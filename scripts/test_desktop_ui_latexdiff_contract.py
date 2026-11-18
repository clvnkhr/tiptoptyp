"""LaTeXdiff native-journey failure detectors; these are not UI interaction evidence."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_latexdiff_journeys import (
    LATEXDIFF_ARGUMENTS, LatexdiffJourneys, latexdiff_artifact,
    latexdiff_complete, latexdiff_created, latexdiff_draft, latexdiff_inputs_unchanged,
    latexdiff_pdf_artifact,
    latexdiff_owner_unchanged, latexdiff_state,
)


class LatexdiffJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, busy=False, error=None,
                     older_fingerprint=fingerprint('/older.tex'), newer_fingerprint=fingerprint('/newer.tex'),
                     output_fingerprint=fingerprint('/changes.tex'), arguments_fingerprint=fingerprint(LATEXDIFF_ARGUMENTS))
        state.update(changes)
        return {'latexdiff': state}

    def owner(self):
        return dict(path='/main.typ', source_fingerprint=fingerprint('owner'), source_bytes=5,
                    tabs=1, tab_order=[8], active_tab_id=8,
                    workspace_root_fingerprint=fingerprint('/'))

    def created(self, **changes):
        state = {**self.owner(), **self.document(visible=False), 'path': '/changes.tex',
                 'document_kind': 'Tex', 'dirty': False, 'tabs': 2, 'tab_order': [8, 9],
                 'active_tab_id': 9, 'preview_tab_id': 9, 'preview_path': '/changes.tex',
                 'modal_open': False, 'completion_notice': 'Created changes.tex · Compiling PDF…',
                 'source_fingerprint': fingerprint('diff source'), 'source_bytes': 11,
                 'pdf_output_state': 'Building', 'pdf_output_pending': True,
                 'pdf_output_revision': 3, 'revision': '3', 'compile_indicator_spinning': True,
                 'preview_requested_backend': 'Pdfium', 'preview_backend': 'PDFium',
                 'preview_pdfium_ready': False}
        state.update(changes)
        return state

    def completed(self, **changes):
        return self.created(**{'pdf_output_state': 'Saved', 'pdf_output_pending': False,
                            'compile_indicator_spinning': False, 'preview_pdfium_ready': True,
                            'completion_notice': 'Compiled changes.pdf', **changes})

    def test_missing_malformed_or_old_multistep_inspection_never_passes(self):
        for document in ({}, {'latexdiff': None}, {'latexdiff': {}},
                         self.document(visible=1), self.document(step='files'),
                         self.document(visible=False, busy=True), self.document(step=None),
                         self.document(older_fingerprint='not-a-fingerprint'),
                         self.document(arguments_fingerprint=None), self.document(error='')):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                latexdiff_state(document)
        closed = dict(visible=False, busy=False, error=None,
                      older_fingerprint=None, newer_fingerprint=None,
                      output_fingerprint=None, arguments_fingerprint=None)
        self.assertFalse(latexdiff_state({'latexdiff': closed})['visible'])

    def test_one_form_draft_requires_exact_paths_options_and_idle_state(self):
        def matches(document):
            return latexdiff_draft(document, Path('/older.tex'), Path('/newer.tex'),
                                   Path('/changes.tex'), LATEXDIFF_ARGUMENTS)
        self.assertTrue(matches(self.document()))
        for changes in ({'busy': True}, {'visible': False},
                        {'older_fingerprint': fingerprint('/other.tex')},
                        {'newer_fingerprint': fingerprint('/older.tex')},
                        {'output_fingerprint': fingerprint('/older.tex')},
                        {'arguments_fingerprint': fingerprint('--math-markup=0')}):
            with self.subTest(changes=changes):
                self.assertFalse(matches(self.document(**changes)))

    def test_created_result_needs_automatic_new_source_preview_and_visible_feedback(self):
        output, before = Path('/changes.tex'), self.owner()
        self.assertTrue(latexdiff_created(self.created(), output, before))
        for changes in ({'path': '/other.tex'}, {'document_kind': 'Text'}, {'dirty': True},
                        {'tabs': 1, 'tab_order': [9]}, {'tab_order': [9, 8]},
                        {'active_tab_id': 8}, {'preview_tab_id': 8}, {'preview_path': '/main.typ'},
                        {'workspace_root_fingerprint': fingerprint('/other')},
                        {'modal_open': True}, {'completion_notice': None},
                        {'completion_notice': ''}, {'completion_notice': 'Unrelated completion'}):
            with self.subTest(changes=changes):
                self.assertFalse(latexdiff_created(self.created(**changes), output, before))
        self.assertFalse(latexdiff_created(self.created(**self.document()), output, before))
        with self.assertRaisesRegex(AssertionError, 'generation failed'):
            latexdiff_created(self.created(**self.document(error='Binary failed')), output, before)
        for field in ('source_fingerprint', 'source_bytes', 'tab_order', 'tabs',
                      'active_tab_id', 'workspace_root_fingerprint'):
            current = self.created(); current.pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                latexdiff_created(current, output, before)

    def test_completed_result_needs_saved_current_pdf_and_the_effective_selected_renderer(self):
        output, before, source = Path('/changes.tex'), self.owner(), 'diff source'
        self.assertTrue(latexdiff_complete(self.completed(), output, source, before))
        for changes in ({'pdf_output_state': 'Building'}, {'pdf_output_state': 'Unverified'},
                        {'pdf_output_pending': True}, {'pdf_output_revision': 2},
                        {'compile_indicator_spinning': True}, {'preview_pdfium_ready': False},
                        {'preview_requested_backend': 'Interactive'}, {'preview_backend': 'Tinymist'},
                        {'source_fingerprint': fingerprint('stale')}, {'source_bytes': 6}):
            with self.subTest(changes=changes):
                self.assertFalse(latexdiff_complete(self.completed(**changes), output, source, before))
        with self.assertRaisesRegex(AssertionError, 'automatic PDF compilation failed'):
            latexdiff_complete(self.completed(pdf_output_state='Failed'), output, source, before)

    def test_pdf_artifact_needs_complete_bounded_real_selected_engine_output(self):
        with tempfile.TemporaryDirectory() as directory:
            pdf = Path(directory) / 'changes.pdf'
            with self.assertRaises(AssertionError):
                latexdiff_pdf_artifact(pdf)
            good = b'%PDF-1.7\n/Producer (pdfTeX-1.40.28)\n' + b'content' * 30 + b'\n%%EOF\n'
            for data in (b'', b'%PDF-incomplete' + b'content' * 30,
                         good.replace(b'pdfTeX', b'XeTeX'), good.replace(b'%PDF-', b'wrong')):
                pdf.write_bytes(data)
                with self.subTest(data=data[:50]), self.assertRaises(AssertionError):
                    latexdiff_pdf_artifact(pdf)
            pdf.write_bytes(good)
            self.assertEqual(latexdiff_pdf_artifact(pdf), good)

    def test_original_document_identity_and_input_bytes_must_be_preserved(self):
        before = self.owner()
        self.assertTrue(latexdiff_owner_unchanged(before, before))
        for change in ({'path': '/other.typ'}, {'source_fingerprint': fingerprint('edited')},
                       {'source_bytes': 6}, {'tabs': 2, 'tab_order': [8, 9]},
                       {'active_tab_id': 9, 'tab_order': [9]}):
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

    def test_running_cancel_uses_settings_and_checks_process_exit_before_restoring(self):
        owner = self
        class Cancellation(LatexdiffJourneys):
            def __init__(self, folder):
                self.folder, self.events = folder, []
                self.before = owner.owner()
                self.document = {**self.before, **owner.document()}
            def latexdiff_executable_setting(self, value, previous=''):
                self.events.append(('settings', str(previous), str(value)))
            def open_latexdiff(self):
                self.events.append(('open',))
            def latexdiff_type(self, name, value, key):
                self.document['latexdiff'][key] = fingerprint(str(value))
            def click(self, name):
                self.events.append(('click', name))
                state = self.document['latexdiff']
                if name == 'latexdiff.generate':
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
