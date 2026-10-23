"""Fast evidence guards; native Return behavior is exercised by indentation."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import unittest

from desktop_ui_indentation_journeys import (
    assert_indentation_result, assert_original_document_restored, indentation_cases,
    indentation_preference_edits,
)
from desktop_ui_journeys import fingerprint


class IndentationContract(unittest.TestCase):
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

    def test_native_journey_is_registered_once_without_optional_models(self):
        spec = importlib.util.spec_from_file_location('indentation_runner',
                                                      Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn('indentation', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('indentation'), ['indentation'])
        self.assertEqual(runner.selected_journeys('all').count('indentation'), 1)
        self.assertTrue(callable(runner.Journey.indentation))
        self.assertNotIn('indentation', runner.LIQUID_JOURNEYS)
        self.assertNotIn('indentation', runner.LTEX_JOURNEYS)


if __name__ == '__main__':
    unittest.main()
