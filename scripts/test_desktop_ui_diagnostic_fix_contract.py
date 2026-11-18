"""Failure detectors for real-model diagnostic-fix journeys; no UI pass claim."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_diagnostic_fix_journeys import (
    DIAGNOSTIC_FIX_MODEL_ID, diagnostic_fix_comparison, diagnostic_fix_fixture,
    diagnostic_fix_model_directory, diagnostic_fix_model_files, diagnostic_fix_native_target,
    diagnostic_fix_ready, diagnostic_fix_row, diagnostic_fix_source, diagnostic_fix_state,
    diagnostic_fix_preview_owner, DiagnosticFixJourneys, copy_diagnostic_fix_model,
    diagnostic_fix_setup_defaults, assert_diagnostic_fix_setup_busy,
    diagnostic_fix_setup_cancelled,
)


class DiagnosticFixContracts(unittest.TestCase):
    def setUp(self):
        self.catalog_patch = patch('desktop_ui_diagnostic_fix_journeys.diagnostic_fix_catalog', return_value={
            'model_id': DIAGNOSTIC_FIX_MODEL_ID, 'model_revision': 'test reference revision',
            'runtime_version': 'test reference runtime', 'catalog_sha256': 'a' * 64,
            'model': {'bytes': len(b'tiny test artifact; the app validates real pins'), 'sha256': 'b' * 64},
            'runtimes': [{'target': diagnostic_fix_native_target(), 'files': [
                {'name': 'llama-server', 'bytes': len(b'tiny test helper')},
                {'name': 'libllama.dylib', 'bytes': len(b'tiny test library')},
            ]}],
        })
        self.catalog_patch.start()
        self.addCleanup(self.catalog_patch.stop)

    def fixture(self):
        return diagnostic_fix_fixture()

    def review(self):
        fixture = self.fixture()
        return {'diagnostic_fix': {'visible': True, 'busy': False, 'current': True,
                'error': None, 'line': fixture['line'],
                'diagnostic_fingerprint': fingerprint(fixture['diagnostic']),
                'comparison': {'before': fingerprint(fixture['original_line']),
                               'after': fingerprint(fixture['replacement']),
                               'before_chars': len(fixture['original_line']),
                               'after_chars': len(fixture['replacement']), 'style': 'Unified'}}}

    def test_review_requires_a_completed_current_actual_change(self):
        self.assertTrue(diagnostic_fix_ready(self.review()))
        for change in ({'visible': False}, {'busy': True}, {'current': False}, {'comparison': None}):
            document = self.review(); document['diagnostic_fix'].update(change)
            with self.subTest(change=change):
                self.assertFalse(diagnostic_fix_ready(document))
        document = self.review()
        document['diagnostic_fix']['comparison']['after'] = document['diagnostic_fix']['comparison']['before']
        self.assertFalse(diagnostic_fix_ready(document))
        document = self.review(); document['diagnostic_fix']['error'] = 'The model did not suggest a change.'
        with self.assertRaisesRegex(AssertionError, 'Diagnostic suggestion failed'):
            diagnostic_fix_ready(document)

    def test_missing_or_malformed_state_never_passes(self):
        for value in (None, {}, {'visible': True, 'busy': 1, 'current': True, 'error': None},
                      {'visible': True, 'busy': False, 'current': True, 'error': 7}):
            with self.subTest(value=value), self.assertRaises(AssertionError):
                diagnostic_fix_state({'diagnostic_fix': value})
        for change in ({'before': None}, {'after_chars': True}, {'style': 'Unknown'}):
            document = self.review(); document['diagnostic_fix']['comparison'].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                diagnostic_fix_ready(document)

    def test_comparison_requires_exact_diagnostic_and_complete_line_in_effective_style(self):
        fixture = self.fixture()
        self.assertTrue(diagnostic_fix_comparison(self.review(), fixture, 'Unified'))
        for field, value in (('line', fixture['line'] + 1), ('diagnostic_fingerprint', fingerprint('other diagnostic'))):
            document = self.review(); document['diagnostic_fix'][field] = value
            with self.subTest(field=field):
                self.assertFalse(diagnostic_fix_comparison(document, fixture, 'Unified'))
        for field, value in (('before', fingerprint(fixture['working'])), ('after', fingerprint('blue')),
                             ('before_chars', len(fixture['working'])), ('after_chars', 4),
                             ('style', 'SideBySide')):
            document = self.review(); document['diagnostic_fix']['comparison'][field] = value
            with self.subTest(field=field):
                self.assertFalse(diagnostic_fix_comparison(document, fixture, 'Unified'))

    def test_source_guard_preserves_exact_owner_and_bytes(self):
        fixture = self.fixture()
        owner = {'path': '/private/tmp/diagnostic-fix.typ', 'tabs': 2, 'active_tab_id': 4}
        document = {**owner, 'source_fingerprint': fingerprint(fixture['working']),
                    'source_bytes': len(fixture['working'].encode())}
        self.assertTrue(diagnostic_fix_source(document, fixture['working'], owner))
        for field, value in (('path', '/other.typ'), ('tabs', 1), ('active_tab_id', 5),
                             ('source_fingerprint', fingerprint(fixture['corrected'])), ('source_bytes', 0)):
            with self.subTest(field=field):
                self.assertFalse(diagnostic_fix_source({**document, field: value}, fixture['working'], owner))

    def test_error_fixture_must_own_preview_before_waiting_for_its_compiler_diagnostic(self):
        path = Path('/private/tmp/diagnostic-fix.typ')
        owner = dict(path=str(path), preview_path=str(path), active_tab_id=3, preview_tab_id=3)
        self.assertTrue(diagnostic_fix_preview_owner(owner, path))
        for field, value in (('preview_path', '/private/tmp/journey.typ'), ('preview_tab_id', 1),
                             ('path', '/private/tmp/journey.typ'), ('active_tab_id', None)):
            with self.subTest(field=field):
                self.assertFalse(diagnostic_fix_preview_owner({**owner, field: value}, path))

        class Recorder(DiagnosticFixJourneys):
            def __init__(self, document):
                self.document = dict(document)
                self.keys = []
            def snapshot(self):
                return {'document': self.document}
            def key(self, code, modifiers):
                self.keys.append((code, modifiers))
                self.document.update(preview_path=self.document['path'],
                                     preview_tab_id=self.document['active_tab_id'])
            def wait(self, description, predicate, timeout):
                if not predicate(self.document):
                    raise AssertionError('Preview action did not establish the observed owner')
                return self.snapshot()
        fresh = Recorder(owner)
        fresh.use_diagnostic_fix_preview(path)
        self.assertEqual(fresh.keys, [], 'an already pinned fixture must not be toggled off')
        old_preview = Recorder({**owner, 'preview_path': '/private/tmp/journey.typ', 'preview_tab_id': 1})
        old_preview.use_diagnostic_fix_preview(path)
        self.assertEqual(old_preview.keys, [(35, 'cmd+alt')])

    def test_row_requires_unsuppressed_exact_line_and_message(self):
        fixture = self.fixture()
        row = dict(index=3, line=fixture['line'], message_fingerprint=fingerprint(fixture['diagnostic']),
                   suppressed=False)
        self.assertEqual(diagnostic_fix_row({'diagnostic_rows': [row]}, fixture), 3)
        for change in ({'line': fixture['line'] + 1}, {'message_fingerprint': fingerprint('different')},
                       {'suppressed': True}):
            with self.subTest(change=change):
                self.assertIsNone(diagnostic_fix_row({'diagnostic_rows': [{**row, **change}]}, fixture))
        self.assertEqual(diagnostic_fix_row({'diagnostic_rows': [{**row, 'index': 4}, row]}, fixture), 3)
        for change in ({'index': True}, {'line': True}, {'suppressed': 0}, {'message_fingerprint': None}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                diagnostic_fix_row({'diagnostic_rows': [{**row, **change}]}, fixture)

    def test_evaluation_fixtures_preserve_every_character_outside_target_line(self):
        catalog = json.loads((Path(__file__).with_name('fixtures') / 'diagnostic-fixes.json').read_text())
        self.assertEqual({case['language'] for case in catalog['cases']}, {'typst', 'tex'})
        self.assertTrue(any(case.get('expect_no_fix') for case in catalog['cases']))
        self.assertEqual(len({case['id'] for case in catalog['cases']}), len(catalog['cases']))
        for case in catalog['cases']:
            fixture = diagnostic_fix_fixture(case['id'])
            before = fixture['working'].splitlines(keepends=True)
            after = fixture['corrected'].splitlines(keepends=True)
            line = fixture['line'] - 1
            with self.subTest(case=case['id']):
                self.assertEqual(before[:line], after[:line])
                self.assertEqual(before[line+1:], after[line+1:])
                self.assertEqual(after[line].rstrip('\r\n'), fixture['replacement'])
                self.assertEqual(fixture['working'] == fixture['corrected'], bool(case.get('expect_no_fix')))

    def cache(self, root):
        base = Path(root) / 'foundation' / DIAGNOSTIC_FIX_MODEL_ID
        runtime = base / 'runtime' / diagnostic_fix_native_target()
        runtime.mkdir(parents=True)
        (base / 'model.gguf').write_bytes(b'tiny test artifact; the app validates real pins')
        (base / 'LICENSE-model.txt').write_text('Model license')
        (base / 'LICENSE-runtime.txt').write_text('Runtime license')
        (runtime / 'llama-server').write_bytes(b'tiny test helper')
        (runtime / 'llama-server').chmod(0o700)
        (runtime / 'libllama.dylib').write_bytes(b'tiny test library')
        return base

    def test_private_copy_is_independent_and_does_not_modify_reference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'reference'; self.cache(root)
            private = Path(directory) / 'private'
            metadata = copy_diagnostic_fix_model(root, private)
            self.assertTrue(metadata['reference_read_only'])
            self.assertTrue(metadata['private_regular_copies'])
            self.assertEqual(diagnostic_fix_model_directory(private), private.resolve())
            old, files = diagnostic_fix_model_files(root)
            new, copied_files = diagnostic_fix_model_files(private)
            self.assertEqual(files, copied_files)
            for relative in files:
                self.assertEqual((old / relative).read_bytes(), (new / relative).read_bytes())
                self.assertNotEqual((old / relative).stat().st_ino, (new / relative).stat().st_ino)
            original = (old / 'model.gguf').read_bytes()
            (new / 'model.gguf').write_bytes(b'changed private copy')
            self.assertEqual((old / 'model.gguf').read_bytes(), original)
            with self.assertRaisesRegex(AssertionError, 'must initially be absent'):
                copy_diagnostic_fix_model(root, private)

    def test_reference_check_rejects_missing_files_nonexecutables_and_symlinks(self):
        for part in ('model.gguf', 'LICENSE-model.txt', 'LICENSE-runtime.txt',
                     'runtime/' + diagnostic_fix_native_target() + '/libllama.dylib'):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory); base = self.cache(root)
                path = base / part
                real = Path(directory) / 'outside'; path.rename(real)
                path.symlink_to(real)
                with self.subTest(part=part), self.assertRaises(ValueError):
                    diagnostic_fix_model_directory(root)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); base = self.cache(root)
            helper = base / 'runtime' / diagnostic_fix_native_target() / 'llama-server'
            helper.chmod(0o600)
            with self.assertRaisesRegex(ValueError, 'server is unavailable'):
                diagnostic_fix_model_directory(root)
        with self.assertRaises(ValueError):
            diagnostic_fix_model_directory(None)
        with self.assertRaises(ValueError):
            diagnostic_fix_model_directory(Path('relative-cache'))

    def test_new_model_journey_remains_explicitly_opt_in(self):
        spec = importlib.util.spec_from_file_location('diagnostic_fix_runner', Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec); spec.loader.exec_module(runner)
        self.assertIn('diagnostic_fix', runner.JOURNEY_NAMES)
        self.assertNotIn('diagnostic_fix', runner.selected_journeys('all'))
        self.assertEqual(runner.selected_journeys('diagnostic_fix'), ['diagnostic_fix'])
        self.assertIn('diagnostic_fix_setup', runner.JOURNEY_NAMES)
        self.assertNotIn('diagnostic_fix_setup', runner.selected_journeys('all'))
        self.assertEqual(runner.selected_journeys('diagnostic_fix_setup'), ['diagnostic_fix_setup'])

    def test_setup_requires_exact_fresh_provider_defaults(self):
        document = {'harper_enabled': False, 'ltex': {'enabled': True}}
        self.assertTrue(diagnostic_fix_setup_defaults(document))
        for harper, ltex in ((True, True), (False, False), (True, False)):
            self.assertFalse(diagnostic_fix_setup_defaults({'harper_enabled': harper,
                                                           'ltex': {'enabled': ltex}}))
        for document in ({}, {'harper_enabled': 0, 'ltex': {'enabled': True}},
                         {'harper_enabled': False, 'ltex': {'enabled': 1}}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                diagnostic_fix_setup_defaults(document)

    def setup_busy(self):
        return {'document': {'resource_repair': {'busy': True,
                                                'status': 'Downloading Liquid fix model…'}},
                'targets': {'settings.diagnostic_fix.download': {'enabled': False, 'age_ms': 0},
                            'settings.diagnostic_fix.cancel_download': {'enabled': True, 'age_ms': 0}}}

    def test_setup_busy_requires_its_own_fresh_controls(self):
        assert_diagnostic_fix_setup_busy(self.setup_busy())
        for change in ({'busy': False}, {'status': 'Downloading Liquid…'}, {'status': None}):
            snapshot = self.setup_busy(); snapshot['document']['resource_repair'].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                assert_diagnostic_fix_setup_busy(snapshot)
        for field, value in (('enabled', False), ('age_ms', 501), ('age_ms', -1), ('age_ms', True)):
            snapshot = self.setup_busy()
            snapshot['targets']['settings.diagnostic_fix.cancel_download'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                assert_diagnostic_fix_setup_busy(snapshot)
        snapshot = self.setup_busy()
        del snapshot['targets']['settings.diagnostic_fix.download']
        with self.assertRaises(AssertionError):
            assert_diagnostic_fix_setup_busy(snapshot)
        snapshot = self.setup_busy()
        snapshot['targets']['settings.liquid.cancel_download'] = {'enabled': True, 'age_ms': 0}
        with self.assertRaisesRegex(AssertionError, 'selection grammar'):
            assert_diagnostic_fix_setup_busy(snapshot)

    def test_setup_cancellation_requires_terminal_cancel_instead_of_success_or_network_failure(self):
        self.assertFalse(diagnostic_fix_setup_cancelled({'resource_repair': {'busy': True}}))
        self.assertTrue(diagnostic_fix_setup_cancelled({'resource_repair': {
            'busy': False, 'status': 'Fix suggestion canceled.'}}))
        for state in ({}, {'busy': 0}, {'busy': False, 'status': None},
                      {'busy': False, 'status': 'Liquid fix model downloaded'},
                      {'busy': False, 'status': 'Connection failed'}):
            with self.subTest(state=state), self.assertRaises(AssertionError):
                diagnostic_fix_setup_cancelled({'resource_repair': state})


if __name__ == '__main__':
    unittest.main()
