"""Fast evidence guards; native Return behavior is exercised by indentation."""
from copy import deepcopy
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from desktop_ui_indentation_journeys import (
    assert_indentation_result, assert_original_document_restored, indentation_cases,
    indentation_preference_edits, IndentationJourneys,
)
from desktop_ui_journeys import fingerprint


class IndentationContract(unittest.TestCase):
    def test_original_save_uses_real_write_and_idle_owner_not_transient_notice(self):
        class Driver(IndentationJourneys):
            def __init__(self, path, document, *, write=True):
                self.path, self.document, self.write = path, document, write
                self.records = []

            def key(self, code):
                if code != 1:
                    raise AssertionError('Unexpected source input')
                if self.write:
                    temporary = self.path.with_suffix('.replacement')
                    temporary.write_bytes(self.path.read_bytes())
                    temporary.replace(self.path)

            def wait(self, description, predicate, timeout):
                if not predicate(self.document):
                    raise AssertionError(description)

            def record(self, name, **fields):
                self.records.append((name, fields))

        with TemporaryDirectory() as directory:
            path = Path(directory) / 'journey.typ'
            source = 'Original α source.\n'
            path.write_text(source, encoding='utf-8')
            original = source.encode('utf-8')
            before = dict(path=str(path), source_fingerprint=fingerprint(source),
                          source_bytes=len(original), revision='1', dirty=False, tabs=1,
                          active_tab_id=4, preview_tab_id=4, preview_path=str(path))
            completed = dict(before, revision='9', formatting_phase='idle', save_operation=None)
            for notice in ('No formatting changes', 'Saved journey.typ', None):
                with self.subTest(notice=notice):
                    driver = Driver(path, dict(completed, completion_notice=notice))
                    driver.indentation_restore_file_history(path, original, before)
                    self.assertTrue(driver.records[0][1]['durable_write_observed'])
            for change in (dict(save_operation={'id': 7}), dict(formatting_phase='requested'),
                           dict(active_tab_id=8), dict(dirty=True), dict(revision='1'),
                           dict(source_fingerprint=fingerprint('different'))):
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    Driver(path, dict(completed, **change)).indentation_restore_file_history(path, original, before)
            with self.assertRaises(AssertionError):
                Driver(path, completed, write=False).indentation_restore_file_history(path, original, before)

    def test_preference_edits_are_idempotent_use_actual_values_and_restore_custom_indentation(self):
        original = dict(auto_save=False, auto_pair_delimiters=False, indent_spaces=7)
        self.assertEqual(indentation_preference_edits(original, original), [])
        current = dict(auto_save=False, auto_pair_delimiters=True, indent_spaces=0)
        self.assertEqual(indentation_preference_edits(current, original), [
            ('"auto_pair_delimiters": true', '"auto_pair_delimiters": false'),
            ('"indent_spaces": 0', '"indent_spaces": 7')])
        for old, desired in (({}, dict(auto_save=False)),
                             (dict(auto_save=0), dict(auto_save=False)),
                             (dict(indent_spaces=True), dict(indent_spaces=2)),
                             (dict(indent_spaces=2), dict(indent_spaces=17)),
                             (dict(indent_spaces=2), dict(indent_spaces=False)),
                             ({}, dict(unknown=True))):
            with self.subTest(old=old, desired=desired), self.assertRaises(AssertionError):
                indentation_preference_edits(old, desired)

    def test_exact_native_evidence_requires_scalar_caret_bytes_path_dirty_and_focus(self):
        text = ' \t  α\n \t  '
        path = Path('/tmp/indent.typ')
        before = dict(path=str(path), source_fingerprint=fingerprint(text),
                      source_bytes=len(text.encode('utf-8')), cursor=[len(text)] * 2,
                      dirty=True, editor_keyboard_focused=True)
        assert_indentation_result(before, path, text, len(text), True)
        for field, value in (('path', '/tmp/indent.tex'), ('source_fingerprint', 'stale'),
                             ('source_bytes', len(text)), ('cursor', [len(text.encode('utf-8'))] * 2),
                             ('cursor', [0, len(text)]), ('dirty', False),
                             ('editor_keyboard_focused', False)):
            changed = deepcopy(before)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_indentation_result(changed, path, text, len(text), True)

    def test_tab_restoration_preserves_content_and_ownership_with_a_fresh_revision(self):
        from desktop_ui_settings_links_journeys import SOURCE_FIELDS
        before = dict(path='/tmp/journey.txt', source_fingerprint='0123456789abcdef',
                      source_bytes=25, revision='1', dirty=False, tabs=1,
                      active_tab_id=0, preview_tab_id=0, preview_path='/tmp/untitled.typ')
        after = dict(before, revision='99')
        assert_original_document_restored(after, before)
        for key in SOURCE_FIELDS:
            if key == 'revision':
                continue
            changed = dict(after, **{key: 'changed'})
            with self.subTest(key=key), self.assertRaises(AssertionError):
                assert_original_document_restored(changed, before)
        for revision in ('0', '1', '-1', 'invalid', '', None, 99, str(2 ** 64)):
            with self.subTest(revision=revision), self.assertRaises(AssertionError):
                assert_original_document_restored(dict(after, revision=revision), before)

    def test_fixtures_cover_both_document_kinds_and_preserve_literal_tabs_crlf_and_pair_caret(self):
        typst = {case['name']: case for case in indentation_cases('typ')}
        tex = {case['name']: case for case in indentation_cases('tex')}
        self.assertEqual(tex['document']['expected'], '\\begin{document}\n')
        self.assertIn('\r\n', tex['crlf']['expected'])
        self.assertNotEqual(typst['paired-block']['expected_cursor'], len(typst['paired-block']['expected']))
        self.assertTrue(indentation_cases('typ', unit='\t')[0]['expected'].endswith('\t \t'))
        self.assertTrue(indentation_cases('tex', unit='\t')[0]['expected'].endswith('\t \t'))
        for suffix in ('typ', 'tex'):
            for case in indentation_cases(suffix):
                self.assertTrue(0 <= case['cursor'] <= len(case['source']))
                self.assertTrue(0 <= case['expected_cursor'] <= len(case['expected']))
        with self.assertRaises(AssertionError):
            indentation_cases('txt')

    def test_environment_completion_expectations_follow_pairing_mode_and_keep_body_caret(self):
        for unit in ('  ', '\t'):
            disabled = {case['name']: case for case in indentation_cases('tex', unit=unit)}
            enabled = {case['name']: case for case in indentation_cases('tex', unit=unit, pairs=True)}
            for name, base in [('align', '\t '), ('document', '')]:
                with self.subTest(unit=unit, environment=name):
                    self.assertEqual(enabled[name]['source'], disabled[name]['source'])
                    self.assertEqual(enabled[name]['expected'],
                                     disabled[name]['expected'] + '\n' + base + '\\end{' + name + '}')
                    self.assertEqual(enabled[name]['expected_cursor'], len(disabled[name]['expected']))
                    self.assertLess(enabled[name]['expected_cursor'], len(enabled[name]['expected']))
            for name in ('literal-prefix', 'crlf'):
                self.assertEqual(enabled[name], disabled[name])
            self.assertEqual(indentation_cases('typ', unit=unit, pairs=True),
                             indentation_cases('typ', unit=unit))

if __name__ == '__main__':
    unittest.main()
