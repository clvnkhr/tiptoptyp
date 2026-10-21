"""LaTeXdiff native-journey failure detectors; these are not UI interaction evidence."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import EditorJourneys, fingerprint
from desktop_ui_latexdiff_journeys import (
    LATEXDIFF_ARGUMENTS, LATEXDIFF_JOURNEYS, LatexdiffJourneys, latexdiff_artifact,
    latexdiff_complete, latexdiff_draft, latexdiff_inputs_unchanged,
    latexdiff_owner_unchanged, latexdiff_state,
)


class LatexdiffJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, busy=False, step='review', error=None, output_path=None,
                     older_fingerprint=fingerprint('/older.tex'), newer_fingerprint=fingerprint('/newer.tex'),
                     output_fingerprint=fingerprint('/changes.tex'), arguments_fingerprint=fingerprint(LATEXDIFF_ARGUMENTS))
        state.update(changes)
        return {'latexdiff': state}

    def test_missing_malformed_and_stale_looking_observations_never_pass(self):
        for document in ({}, {'latexdiff': None}, {'latexdiff': {}},
                         self.document(visible=1), self.document(step='unknown'),
                         self.document(visible=False, busy=True), self.document(step=None),
                         self.document(older_fingerprint='not-a-fingerprint'),
                         self.document(arguments_fingerprint=None), self.document(error=''),
                         self.document(output_path='relative.tex')):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                latexdiff_state(document)
        closed = dict(visible=False, busy=False, step=None, error=None, output_path=None,
                      older_fingerprint=None, newer_fingerprint=None,
                      output_fingerprint=None, arguments_fingerprint=None)
        self.assertFalse(latexdiff_state({'latexdiff': closed})['visible'])

    def test_draft_requires_exact_paths_options_step_and_idle_state(self):
        def matches(document):
            return latexdiff_draft(document, 'review', Path('/older.tex'), Path('/newer.tex'),
                                   Path('/changes.tex'), LATEXDIFF_ARGUMENTS)
        self.assertTrue(matches(self.document()))
        for changes in ({'step': 'files'}, {'busy': True}, {'visible': False},
                        {'older_fingerprint': fingerprint('/other.tex')},
                        {'newer_fingerprint': fingerprint('/older.tex')},
                        {'output_fingerprint': fingerprint('/older.tex')},
                        {'arguments_fingerprint': fingerprint('--math-markup=0')}):
            with self.subTest(changes=changes):
                self.assertFalse(matches(self.document(**changes)))

    def test_completed_result_requires_idle_review_exact_artifact_and_no_error(self):
        self.assertTrue(latexdiff_complete(self.document(output_path='/changes.tex'), Path('/changes.tex')))
        for changes in ({'busy': True}, {'visible': False}, {'step': 'output'},
                        {'output_path': None}, {'output_path': '/other.tex'}):
            self.assertFalse(latexdiff_complete(self.document(**{'output_path': '/changes.tex', **changes}), Path('/changes.tex')))
        with self.assertRaisesRegex(AssertionError, 'generation failed'):
            latexdiff_complete(self.document(error='Binary failed'), Path('/changes.tex'))

    def test_original_document_identity_and_input_bytes_must_be_preserved(self):
        before = dict(path='/main.typ', source_fingerprint=fingerprint('owner'), source_bytes=5,
                      tabs=1, active_tab_id=8)
        self.assertTrue(latexdiff_owner_unchanged(before, before))
        for change in ({'path': '/other.typ'}, {'source_fingerprint': fingerprint('edited')},
                       {'source_bytes': 6}, {'tabs': 2}, {'active_tab_id': 9}):
            self.assertFalse(latexdiff_owner_unchanged({**before, **change}, before))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'older.tex'
            path.write_bytes(b'original')
            inputs = [(path, b'original')]
            self.assertTrue(latexdiff_inputs_unchanged(inputs))
            path.write_bytes(b'changed')
            with self.assertRaisesRegex(AssertionError, 'modified an original'):
                latexdiff_inputs_unchanged(inputs)
            path.unlink()
            with self.assertRaises(AssertionError):
                latexdiff_inputs_unchanged(inputs)

    def test_generated_body_needs_real_addition_and_deletion_not_preamble_definitions(self):
        good = '\\documentclass{article}\n\\begin{document}\n\\DIFdel{original} \\DIFadd{improved}\n\\end{document}\n'
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'changes.tex'
            with self.assertRaises(AssertionError):
                latexdiff_artifact(path)
            for source in ('', '\\newcommand{\\DIFadd}[1]{#1}\\newcommand{\\DIFdel}[1]{}\n\\begin{document}original improved\\end{document}',
                           good.replace('\\DIFdel{original}', 'original'),
                           good.replace('\\DIFadd{improved}', 'improved')):
                path.write_text(source)
                with self.subTest(source=source), self.assertRaises(AssertionError):
                    latexdiff_artifact(path)
            path.write_text(good)
            self.assertEqual(latexdiff_artifact(path), good)

    def test_journey_is_registered_but_optional_external_tool_is_not_hidden_in_all(self):
        spec = importlib.util.spec_from_file_location('latexdiff_desktop_runner', Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertEqual(LATEXDIFF_JOURNEYS, ('latexdiff_wizard',))
        self.assertTrue(issubclass(EditorJourneys, LatexdiffJourneys))
        self.assertIn('latexdiff_wizard', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('latexdiff_wizard'), ['latexdiff_wizard'])
        self.assertNotIn('latexdiff_wizard', runner.selected_journeys('all'))

    def test_running_cancel_uses_settings_and_checks_process_exit_before_restoring(self):
        owner = self
        class Cancellation(LatexdiffJourneys):
            def __init__(self, folder):
                self.folder, self.events = folder, []
                self.before = dict(path='/main.typ', source_fingerprint=fingerprint('owner'),
                                   source_bytes=5, tabs=1, active_tab_id=8)
                self.document = {**self.before, **owner.document(step='files')}
            def latexdiff_executable_setting(self, value, previous=''):
                self.events.append(('settings', str(previous), str(value)))
            def open_latexdiff_wizard(self):
                self.events.append(('open',))
            def latexdiff_type(self, name, value, key):
                self.document['latexdiff'][key] = fingerprint(str(value))
            def click(self, name):
                self.events.append(('click', name))
                state = self.document['latexdiff']
                if name == 'latexdiff.next':
                    state['step'] = 'output' if state['step'] == 'files' else 'review'
                elif name == 'latexdiff.generate':
                    state['busy'] = True
                    (self.folder / 'worker.pid').write_text('12345')
                elif name == 'latexdiff.cancel':
                    state['busy'] = False
                elif name == 'latexdiff.close':
                    state['visible'] = False
            def wait(self, description, predicate, timeout=6):
                if not predicate(self.document):
                    raise AssertionError(description)
            def record(self, name, **values):
                self.events.append(('record', name, values))
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            older, newer = folder / 'older.tex', folder / 'newer.tex'
            for path in (older, newer):
                path.write_bytes(b'original')
            harness = Cancellation(folder)
            with patch('desktop_ui_latexdiff_journeys.os.kill', side_effect=ProcessLookupError) as process:
                harness.latexdiff_cancel_running(older, newer, folder, harness.before,
                                                 [(older, b'original'), (newer, b'original')])
            process.assert_called_once_with(12345, 0)
            self.assertIn(('click', 'latexdiff.cancel'), harness.events)
            self.assertIn(('record', 'latexdiff.running_cancel', {'process_stopped': True, 'output_exists': False}), harness.events)
            settings = [event[1:] for event in harness.events if event[0] == 'settings']
            self.assertEqual(settings, [('', str(folder / 'controlled latexdiff')),
                                        (str(folder / 'controlled latexdiff'), '')])
            (folder / 'worker.pid').unlink()
            alive = Cancellation(folder)
            with patch('desktop_ui_latexdiff_journeys.os.kill', return_value=None):
                with self.assertRaisesRegex(AssertionError, 'process alive'):
                    alive.latexdiff_cancel_running(older, newer, folder, alive.before,
                                                  [(older, b'original'), (newer, b'original')])
            self.assertEqual(len([event for event in alive.events if event[0] == 'settings']), 1,
                             'a failed cancellation must not be hidden by further UI actions')

    def test_executable_setting_scopes_shared_field_names_and_verifies_selection_before_edit(self):
        import json
        owner = self
        class Settings(LatexdiffJourneys):
            def __init__(self, wrong_line=False):
                self.events = []
                self.query = ''
                self.current = '    "executable": "",'
                self.saved = False
                self.wrong_line = wrong_line
            def settings(self):
                self.events.append(('open',))
            def snapshot(self):
                return {'document': {'settings_json': dict(visible=True, valid=True, saved=self.saved,
                        query=self.query, fingerprint=fingerprint(self.current))}}
            def key(self, code, flags='cmd'):
                self.events.append(('key', code, flags))
            def wait_target(self, target, focused=False):
                owner.assertTrue(focused)
                self.events.append(('focus', target))
            def native(self, command, value):
                owner.assertEqual(command, 'text')
                self.query = value
                self.events.append(('query', value))
            def wait(self, description, predicate, timeout=6):
                owner.assertTrue(predicate(self.snapshot()['document']), description)
            def copy_with_receipt(self, expected, perform):
                if self.query == '"latexdiff": {':
                    owner.assertEqual(self.events[-3:], [('key', 125, 'none'),
                                      ('key', 123, 'cmd'), ('key', 124, 'cmd+shift')])
                    # Cmd+Shift+Right selects only one displayed row in a narrow
                    # Settings window; exact Find must handle the wrapped path.
                    selected = self.current[:60]
                else:
                    owner.assertEqual(self.query, self.current)
                    selected = self.current
                perform()
                actual = '    "flatten": true,' if self.wrong_line else selected
                if actual != expected:
                    raise AssertionError('Copy did not match the selected fixture text')
                self.events.append(('copy', expected))
            def paste_source(self, value, check):
                owner.assertEqual(self.events[-1][0], 'copy')
                self.events.append(('paste', value))
                self.current = value
                check()
            def click(self, name):
                self.events.append(('click', name))
                if name == 'settings.json.save':
                    self.saved = True
            def close_settings(self):
                self.events.append(('close',))
        executable = Path('/private/tmp/path with spaces/controlled latexdiff')
        harness = Settings()
        harness.latexdiff_executable_setting(executable)
        expected = '    "executable": ' + json.dumps(str(executable)) + ','
        self.assertEqual(harness.current, expected)
        self.assertEqual([event[1] for event in harness.events if event[0] == 'query'],
                         ['"latexdiff": {', expected])
        self.assertLess(harness.events.index(('copy', expected)),
                        harness.events.index(('click', 'settings.json.save')))
        harness.events.clear()
        harness.saved = False
        harness.latexdiff_executable_setting('', previous=executable)
        self.assertEqual(harness.current, '    "executable": "",')
        self.assertEqual([event[1] for event in harness.events if event[0] == 'query'],
                         [expected, '"latexdiff": {'])
        stale_schema = Settings(wrong_line=True)
        with self.assertRaisesRegex(AssertionError, 'selected fixture text'):
            stale_schema.latexdiff_executable_setting(executable)
        self.assertFalse(any(event[0] == 'paste' for event in stale_schema.events),
                         'a wrong structural selection must fail before modifying any binary')
        self.assertFalse(stale_schema.saved)

    def test_field_typing_uses_focused_native_controls_and_requires_observed_fingerprint(self):
        owner = self
        class NativeField(LatexdiffJourneys):
            def __init__(self):
                self.events = []
                self.value = ''
            def click(self, target):
                self.events.append(('click', target))
            def wait_target(self, target, focused=False):
                self.events.append(('wait_target', target, focused))
            def key(self, code):
                self.events.append(('key', code))
            def native(self, action, value):
                self.events.append((action, value))
                self.value = value
            def wait(self, description, predicate):
                document = owner.document(older_fingerprint=fingerprint(self.value))
                if not predicate(document):
                    raise AssertionError(description)
        field = NativeField()
        field.latexdiff_type('latexdiff.older', Path('/older draft.tex'), 'older_fingerprint')
        self.assertEqual(field.events, [('click', 'latexdiff.older'),
            ('wait_target', 'latexdiff.older', True), ('key', 0), ('text', '/older draft.tex')])


if __name__ == '__main__':
    unittest.main()
