"""Deterministic protocol and measurement contracts; no JVMs are launched."""
from copy import deepcopy
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location('benchmark_ltex_jvm', Path(__file__).with_name('benchmark-ltex-jvm.py'))
bench = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = bench
SPEC.loader.exec_module(bench)


def diagnostic(message='This are a test.', character=0):
    return {'range': {'start': {'line': 2, 'character': character},
                      'end': {'line': 2, 'character': character + 4}},
            'code': 'AGREEMENT', 'severity': 2, 'source': 'ltex', 'message': message}


def publication(uri, diagnostics):
    return {'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics',
            'params': {'uri': uri, 'diagnostics': diagnostics}}


class FramingTests(unittest.TestCase):
    def test_utf8_content_length_and_multiple_frames_survive_fragmented_reads(self):
        message = {'jsonrpc': '2.0', 'method': 'test', 'params': 'α。'}
        frame = bench.encode_message(message)
        self.assertEqual(int(frame.split(b'\r\n', 1)[0].split(b': ')[1]), len(frame.split(b'\r\n\r\n', 1)[1]))

        class Fragmented(io.BytesIO):
            def read(self, size=-1):
                return super().read(min(size, 3))

        stream = Fragmented(frame + frame)
        self.assertEqual(bench.read_message(stream), message)
        self.assertEqual(bench.read_message(stream), message)
        self.assertIsNone(bench.read_message(stream))

    def test_invalid_frames_fail_instead_of_hanging_or_truncating(self):
        for frame in (b'\r\n', b'Content-Length: -1\r\n\r\n',
                      b'Content-Length: 1\r\nContent-Length: 1\r\n\r\n0',
                      b'Content-Length: 4\r\n\r\n{}', b'Invalid header\r\n\r\n',
                      b'Content-Length: 2\r\n\r\n[]',
                      f'Content-Length: {bench.MAX_BODY + 1}\r\n\r\n'.encode()):
            with self.subTest(frame=frame[:70]), self.assertRaises(bench.BenchmarkError):
                bench.read_message(io.BytesIO(frame))


class ProtocolTests(unittest.TestCase):
    def test_both_configuration_requests_preserve_nested_values_and_workspace(self):
        params = {'items': [{'section': name} for name in ('', 'ltex', 'ltex.language',
                  'ltex.additionalRules.enablePickyRules', 'ltex.missing', 'other')]}
        expected = [{'ltex': bench.CONFIGURATION}, bench.CONFIGURATION, 'en-GB', False, None, None]
        for method in ('workspace/configuration', 'ltex/workspaceSpecificConfiguration'):
            reply = bench.server_request_response({'id': 'request', 'method': method, 'params': params}, 'file:///root')
            self.assertEqual(reply['id'], 'request')
            self.assertEqual(reply['result'], expected)
        reply = bench.server_request_response({'id': 2, 'method': 'workspace/workspaceFolders'}, 'file:///root')
        self.assertEqual(reply['result'][0]['uri'], 'file:///root')
        for method in ('client/registerCapability', 'client/unregisterCapability', 'window/workDoneProgress/create'):
            self.assertIsNone(bench.server_request_response({'id': 3, 'method': method}, 'file:///root')['result'])
        self.assertEqual(bench.server_request_response({'id': 4, 'method': 'unknown'}, 'file:///root')['error']['code'], -32601)
        self.assertIsNone(bench.server_request_response({'method': 'notification'}, 'file:///root'))

    def test_check_waits_for_both_matching_publication_and_success_response(self):
        pending = bench.PendingCheck(7, 'file:///paper.tex')
        self.assertFalse(pending.observe(publication('file:///wrong.tex', [diagnostic()])))
        self.assertFalse(pending.observe({'id': 8, 'result': {'success': True}}))
        self.assertFalse(pending.observe({'id': 7, 'result': {'success': True}}))
        self.assertTrue(pending.observe(publication('file:///paper.tex', [])))
        self.assertEqual(pending.diagnostics, [])
        pending = bench.PendingCheck(7, 'file:///paper.tex')
        self.assertFalse(pending.observe(publication('file:///paper.tex', [diagnostic()])))
        self.assertTrue(pending.observe({'id': 7, 'result': {'success': True}}))
        for response in ({'id': 7, 'error': {'message': 'private prose'}},
                         {'id': 7, 'result': {'success': False}}):
            with self.assertRaises(bench.BenchmarkError) as caught:
                bench.PendingCheck(7, 'file:///paper.tex').observe(response)
            self.assertNotIn('private prose', str(caught.exception))

    def test_explicit_check_precedes_did_open_and_actions_then_changes_on_next_check(self):
        session = bench.Session.__new__(bench.Session)
        session.started, session.opened, session.version = time.perf_counter_ns(), False, 0
        sent, identifiers = [], iter(range(2, 20))
        replies = []

        def request(method, params):
            request_id = next(identifiers)
            sent.append((method, params))
            if method == 'workspace/executeCommand':
                replies.extend([publication(params['arguments'][0]['uri'], [diagnostic()]),
                                {'id': request_id, 'result': {'success': True}}])
            else:
                replies.append({'id': request_id, 'result': []})
            return request_id

        session.request = request
        session.notify = lambda method, params: sent.append((method, params))
        session.receive = lambda deadline: replies.pop(0)
        for source in ('A 😀\nThis are a test.', 'A 😀\nThis are a new test.'):
            result = session.check('file:///paper.tex', 'latex', source, 1)
            self.assertEqual(len(result[0]), 1)
        self.assertEqual([method for method, _ in sent], [
            'workspace/executeCommand', 'textDocument/didOpen', 'textDocument/codeAction',
            'workspace/executeCommand', 'textDocument/didChange', 'textDocument/codeAction'])
        self.assertGreaterEqual(session.last_suggestions_from_process_ms, session.last_diagnostics_from_process_ms)
        self.assertEqual(sent[2][1]['range']['end'], {'line': 1, 'character': 16})
        self.assertEqual(sent[4][1]['textDocument']['version'], 2)


class ParityTests(unittest.TestCase):
    def test_diagnostic_bag_ignores_order_but_keeps_duplicates_locations_and_messages(self):
        a, b = diagnostic(), diagnostic('Other private sentence', 7)
        identity = bench.diagnostic_identity([a, b], 1, ['AGREEMENT'])
        self.assertEqual(identity, bench.diagnostic_identity([b, a], 1, ['AGREEMENT']))
        self.assertNotEqual(identity['fingerprint'], bench.diagnostic_identity([a, a, b], 1, [])['fingerprint'])
        for key, value in [('message', 'changed'), ('code', 'OTHER'), ('severity', 1), ('source', 'other')]:
            changed = deepcopy(a)
            changed[key] = value
            self.assertNotEqual(identity['fingerprint'], bench.diagnostic_identity([changed, b], 1, [])['fingerprint'])
        self.assertNotIn('private sentence', bench.canonical(identity))
        for diagnostics, required in [([], []), ([a], ['MISSING'])]:
            with self.assertRaises(bench.BenchmarkError):
                bench.diagnostic_identity(diagnostics, 1, required)

    def test_front_only_publication_cannot_pass_middle_and_end_coverage_gate(self):
        front = diagnostic()
        middle, end = deepcopy(front), deepcopy(front)
        for item, line in ((middle, 50), (end, 100)):
            item['range']['start']['line'] = item['range']['end']['line'] = line
        for incomplete in ([front], [front, middle], [front, end]):
            with self.subTest(count=len(incomplete)), self.assertRaises(bench.BenchmarkError):
                bench.diagnostic_identity(incomplete, 1, ['AGREEMENT'], [2, 50, 100])
        result = bench.diagnostic_identity([end, front, middle], 1, ['AGREEMENT'], [2, 50, 100])
        self.assertEqual(result['covered_required_lines'], [2, 50, 100])
        self.assertEqual(result['count'], 3)

    def test_suggestion_bags_and_ordered_per_range_choices_detect_different_regressions(self):
        uri = 'file:///paper.tex'
        actions = [{'title': text, 'kind': 'quickfix', 'edit': {'changes': {
            uri: [{'range': diagnostic()['range'], 'newText': text}]}}} for text in ('is', 'was')]
        original = bench.action_identity(actions, uri)
        reordered = bench.action_identity(list(reversed(actions)), uri)
        self.assertEqual(original['fingerprint'], reordered['fingerprint'])
        self.assertNotEqual(original['preferred_fingerprint'], reordered['preferred_fingerprint'])
        self.assertNotIn('newText', bench.canonical(original))
        moved = deepcopy(actions)
        for action in moved:
            action['edit']['changes']['file:///other.tex'] = action['edit']['changes'].pop(uri)
        self.assertNotEqual(original['fingerprint'], bench.action_identity(moved, uri)['fingerprint'])


    def test_actual_corrections_must_be_same_file_and_use_editor_workspace_edit_precedence(self):
        uri, other = 'file:///paper.tex', 'file:///other.tex'
        replacement = {'range': diagnostic()['range'], 'newText': 'is'}
        foreign = {'range': diagnostic()['range'], 'newText': 'was'}
        mixed = {'title': 'mixed', 'edit': {'changes': {uri: [replacement], other: [foreign]}}}
        self.assertEqual(bench.same_file_edits(mixed, uri), [])
        mixed = {'title': 'mixed', 'edit': {'documentChanges': [
            {'textDocument': {'uri': uri}, 'edits': [replacement]},
            {'textDocument': {'uri': other}, 'edits': [foreign]}]}}
        self.assertEqual(bench.same_file_edits(mixed, uri), [])
        admitted = {'title': 'local', 'edit': {'documentChanges': [
            {'textDocument': {'uri': uri}, 'edits': [replacement]}],
            'changes': {uri: [foreign]}}}
        self.assertEqual(bench.same_file_edits(admitted, uri), [replacement])
        self.assertEqual(bench.action_identity([admitted], uri, 1)['same_file_replacements'], 1)
        for actions in ([], [{'title': 'command only', 'command': 'ltex.ignore'}], [mixed]):
            with self.assertRaises(bench.BenchmarkError):
                bench.action_identity(actions, uri, 1)


class WorkloadTests(unittest.TestCase):
    def fixture(self, directory, edits=3):
        (directory / 'base.typ').write_text('This are the base result.')
        for index in range(edits):
            (directory / f'edit-{index}.typ').write_text(f'This are result {index}.')
        manifest = {'version': 1, 'fixtures': [{'name': 'short-typ', 'language': 'typst',
            'source': 'base.typ', 'edits': [f'edit-{index}.typ' for index in range(edits)],
            'min_diagnostics': 1, 'required_rules': ['AGREEMENT']}]}
        path = directory / 'fixtures.json'
        path.write_text(json.dumps(manifest))
        return path

    def test_warmup_edits_are_never_reused_as_measurements_and_shortages_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = self.fixture(directory)
            fixtures, _ = bench.load_fixtures(path, 1, 2)
            items = list(bench.workload(fixtures[0], 1, 2))
            edits = [item for item in items if item[0].startswith('edit')]
            self.assertEqual([item[0] for item in edits], ['edit_warmup', 'edit', 'edit'])
            self.assertEqual(len({item[3]['sha256'] for item in edits}), 3)
            with self.assertRaises(bench.BenchmarkError):
                bench.load_fixtures(path, 2, 2)
            (directory / 'edit-2.typ').write_text((directory / 'edit-0.typ').read_text())
            with self.assertRaises(bench.BenchmarkError):
                bench.load_fixtures(path, 1, 2)

    def test_manifest_diagnostic_lines_are_optional_validated_and_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = self.fixture(directory)
            fixtures, _ = bench.load_fixtures(path, 1, 2)
            self.assertEqual(fixtures[0]['required_lines'], [])
            manifest = json.loads(path.read_text())
            manifest['fixtures'][0]['required_diagnostic_lines'] = [0]
            path.write_text(json.dumps(manifest))
            fixtures, _ = bench.load_fixtures(path, 1, 2)
            self.assertEqual(fixtures[0]['required_lines'], [0])
            for invalid in ([True], [-1], [0, 0], [1], '0', [None]):
                with self.subTest(invalid=invalid), self.assertRaises(bench.BenchmarkError):
                    manifest['fixtures'][0]['required_diagnostic_lines'] = invalid
                    path.write_text(json.dumps(manifest))
                    bench.load_fixtures(path, 1, 2)

    def test_balanced_process_blocks_and_summary_report_actual_samples(self):
        self.assertEqual(bench.balanced_order(['A', 'B'], 2), ['A', 'B', 'B', 'A'] * 2)
        with self.assertRaises(bench.BenchmarkError):
            bench.balanced_order(['A', 'A'], 1)
        result = bench.summary([1, 9, 3, 7])
        self.assertEqual(result, {'samples': 4, 'median_ms': 5, 'mean_ms': 5,
                                 'min_ms': 1, 'max_ms': 9, 'p95_ms': 9})
        for samples in ([], [float('nan')], [float('inf')], [-1]):
            with self.assertRaises(bench.BenchmarkError):
                bench.summary(samples)

    def test_environment_has_no_inherited_heap_agent_or_gc_flags(self):
        with patch.dict(os.environ, {name: 'private override' for name in bench.JAVA_OVERRIDES}):
            environment = bench.benchmark_environment(Path('/candidate home'))
        self.assertTrue(all(name not in environment for name in bench.JAVA_OVERRIDES))
        self.assertEqual(environment['JAVA_HOME'], '/candidate home')
        self.assertEqual(environment['JAVACMD'], '/candidate home/bin/java')
        self.assertEqual(environment['TZ'], 'UTC')


class CleanupTests(unittest.TestCase):
    @unittest.skipUnless(os.name == 'posix', 'process groups are a POSIX contract')
    def test_timeout_kills_the_owned_process_group_and_reaps_with_bounded_waits(self):
        session = bench.Session.__new__(bench.Session)
        process = Mock(pid=1234, stdin=io.BytesIO(), stdout=io.BytesIO(), stderr=io.BytesIO())
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired('java', 1), 0]
        session.process, session.stop = process, threading.Event()
        session.threads, session.stderr = [], []
        session.send = lambda message: None
        session.receive = Mock(side_effect=bench.BenchmarkError('timeout'))
        with patch.object(bench.os, 'killpg') as kill:
            session.close()
        self.assertEqual(kill.call_args_list[0].args, (1234, signal.SIGTERM))
        self.assertEqual(kill.call_args_list[1].args, (1234, signal.SIGKILL))
        self.assertEqual([call.kwargs['timeout'] for call in process.wait.call_args_list], [1, 2])
        self.assertTrue(session.stop.is_set())

    @unittest.skipUnless(os.name == 'posix', 'process groups are a POSIX contract')
    def test_descendants_are_retired_even_after_the_launcher_has_exited(self):
        session = bench.Session.__new__(bench.Session)
        session.process = Mock(pid=1234, stdin=io.BytesIO(), stdout=io.BytesIO(), stderr=io.BytesIO())
        session.process.poll.return_value = 0
        session.stop, session.threads, session.stderr = threading.Event(), [], []
        with patch.object(bench.os, 'killpg') as kill:
            session.close()
        self.assertEqual([call.args for call in kill.call_args_list],
                         [(1234, signal.SIGTERM), (1234, signal.SIGKILL)])


    @unittest.skipUnless(os.name == 'posix' and hasattr(os, 'fork'), 'requires owned POSIX process groups')
    def test_real_exited_launcher_with_term_ignoring_descendant_cannot_block_pipe_close(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            marker = workspace / 'ready'
            launcher = workspace / 'fake-ltex'
            launcher.write_text('#!' + sys.executable + '\n' +
                'import os, signal, time\nfrom pathlib import Path\n' +
                'if os.fork() == 0:\n' +
                '    signal.signal(signal.SIGTERM, signal.SIG_IGN)\n' +
                '    Path(' + repr(str(marker)) + ').write_text(str(os.getpid()))\n' +
                '    while True: time.sleep(1)\n' +
                'os._exit(0)\n')
            launcher.chmod(0o700)
            session = bench.Session(launcher, workspace, workspace)
            closer = None
            try:
                deadline = time.monotonic() + 5
                while not marker.exists() or session.process.poll() is None:
                    if time.monotonic() >= deadline:
                        self.fail('Controlled descendant did not become ready')
                    time.sleep(.01)
                closer = threading.Thread(target=session.close, daemon=True)
                closer.start()
                closer.join(timeout=5)
                self.assertFalse(closer.is_alive(), 'Buffered pipe close blocked after the launcher exited')
            finally:
                try:
                    os.killpg(session.process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                if closer is not None:
                    closer.join(timeout=1)
                else:
                    session.close()


    @unittest.skipUnless(os.name == 'posix', 'process groups are a POSIX contract')
    def test_broken_stdin_close_does_not_skip_other_pipes_or_reader_joins(self):
        session = bench.Session.__new__(bench.Session)
        stdin = Mock()
        stdin.close.side_effect = BrokenPipeError('closed child input')
        session.process = Mock(pid=1234, stdin=stdin, stdout=io.BytesIO(), stderr=io.BytesIO())
        session.process.poll.return_value = 0
        reader = Mock()
        session.stop, session.threads, session.stderr = threading.Event(), [reader], []
        with patch.object(bench.os, 'killpg'):
            session.close()
        self.assertTrue(session.process.stdout.closed)
        self.assertTrue(session.process.stderr.closed)
        reader.join.assert_called_once_with(timeout=.2)


if __name__ == '__main__':
    unittest.main()
