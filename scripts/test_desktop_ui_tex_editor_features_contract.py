"""Deterministic evidence guards for the native TeX editor-parity helper."""
from copy import deepcopy
import inspect
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from desktop_ui_journeys import fingerprint
from desktop_ui_tex_editor_features import (
    TEX_EDITOR_PREFERENCES, TEX_EDITOR_SOURCE, TexEditorFeaturesJourneys, editor_pair_result,
    tex_editor_folds, tex_editor_match, tex_editor_owner_restored,
    tex_editor_preference_edits, tex_editor_render_ready, tex_backslash_edit_result,
)


class TexEditorFeatureContract(unittest.TestCase):
    def test_backslash_edit_receipt_rejects_lost_closer_wrong_scalar_caret_and_foreign_owner(self):
        source = 'α \\(\\a\\)'
        owner = dict(path='/private/tmp/closer.tex', tabs=2, active_tab_id=3,
                     preview_tab_id=1, preview_path='/main.tex', workspace_root_fingerprint=fingerprint('/workspace'))
        document = dict(owner, document_kind='Tex', source_fingerprint=fingerprint(source),
                        source_bytes=len(source.encode()), cursor=[6, 6],
                        editor_keyboard_focused=True, revision='12')
        self.assertTrue(tex_backslash_edit_result(document, owner, source, 6, '12'))
        for field, value in [('source_fingerprint', fingerprint('α \\(\\a)')),
                             ('source_bytes', len(source)), ('cursor', [5, 5]),
                             ('cursor', [6, True]), ('document_kind', 'Typst'),
                             ('editor_keyboard_focused', False), ('revision', '13'),
                             ('path', '/foreign.tex'), ('active_tab_id', 1),
                             ('preview_tab_id', 3), ('tabs', 1),
                             ('workspace_root_fingerprint', fingerprint('/foreign'))]:
            with self.subTest(field=field):
                self.assertFalse(tex_backslash_edit_result(dict(document, **{field: value}), owner, source, 6, '12'))
        for field in owner:
            partial = dict(document)
            partial.pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, 'ownership inspection'):
                tex_backslash_edit_result(partial, owner, source, 6)

    def test_preference_edits_use_actual_values_and_preserve_custom_incoming_choices(self):
        before = dict(auto_save=True, auto_pair_delimiters=False,
                      line_numbers=False, line_wrap=True, sticky_context=False)
        self.assertEqual(tex_editor_preference_edits(before, before), [])
        desired = dict(auto_save=False, auto_pair_delimiters=True,
                       line_numbers=True, line_wrap=False, sticky_context=True)
        edits = tex_editor_preference_edits(before, desired)
        self.assertEqual(len(edits), len(TEX_EDITOR_PREFERENCES))
        self.assertIn(('"sticky_context_rows": false', '"sticky_context_rows": true'), edits)
        self.assertEqual(tex_editor_preference_edits(desired, before), [(new, old) for old, new in edits])
        for document, changes in [({}, dict(auto_save=False)),
                                  (dict(auto_save=1), dict(auto_save=False)),
                                  (before, dict(auto_save=0)),
                                  (before, dict(unknown=True))]:
            with self.subTest(document=document, changes=changes), self.assertRaises(AssertionError):
                tex_editor_preference_edits(document, changes)

    def test_cached_match_requires_current_scalar_ranges_not_bytes_or_guesses(self):
        source = 'α{}'
        pair = [[1, 2], [2, 3]]
        self.assertEqual(tex_editor_match(dict(matching_delimiters=pair), source), pair)
        self.assertIsNone(tex_editor_match(dict(matching_delimiters=None), source))
        with self.assertRaises(AssertionError):
            tex_editor_match({}, source)
        for ranges in ([], [[1, 2]], [[1, 2], [2, 4]], [[2, 3], [1, 2]],
                       [[True, 2], [2, 3]], [[-1, 0], [2, 3]], [[1, 1], [2, 3]],
                       [[0, 3], [2, 3]], 'pair'):
            with self.subTest(ranges=ranges), self.assertRaises(AssertionError):
                tex_editor_match(dict(matching_delimiters=ranges), source)

    def test_render_evidence_requires_actual_tex_grammar_and_observed_rainbow_preference(self):
        for syntax in ('TeX', 'LaTeX'):
            self.assertTrue(tex_editor_render_ready(dict(document_kind='Tex', highlight_syntax=syntax, rainbow_brackets=True)))
        for syntax in (None, 'Plain Text', 'Typst'):
            self.assertFalse(tex_editor_render_ready(dict(document_kind='Tex', highlight_syntax=syntax, rainbow_brackets=True)))
        self.assertFalse(tex_editor_render_ready(dict(document_kind='Typst', highlight_syntax='TeX', rainbow_brackets=True)))
        for change in ({}, dict(rainbow_brackets=1), dict(rainbow_brackets=True, highlight_syntax=[])):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                tex_editor_render_ready(change)

    def pair_observation(self):
        source = 'α\\[x + y\\]'
        pair = [[1, 3], [len(source) - 2, len(source)]]
        document = dict(document_kind='Tex', source_fingerprint=fingerprint(source),
                        source_bytes=len(source.encode('utf-8')), cursor=[5, 5],
                        editor_keyboard_focused=True, matching_delimiters=pair,
                        path='/private/tmp/pairs.tex', active_tab_id=2, revision='7')
        return source, pair, document

    def test_pair_receipt_requires_exact_source_scalar_caret_language_and_current_query(self):
        source, pair, document = self.pair_observation()
        self.assertTrue(editor_pair_result(document, source, 5, pair, 'Tex'))
        for field, value in [('document_kind', 'Typst'), ('source_fingerprint', fingerprint('old')),
                             ('source_bytes', len(source)), ('source_bytes', True),
                             ('cursor', [4, 5]), ('cursor', [6, 6]),
                             ('editor_keyboard_focused', False), ('matching_delimiters', None),
                             ('matching_delimiters', [[1, 2], [len(source) - 2, len(source)]])]:
            with self.subTest(field=field, value=value):
                self.assertFalse(editor_pair_result(dict(document, **{field: value}), source, 5, pair, 'Tex'))
        del document['matching_delimiters']
        with self.assertRaises(AssertionError):
            editor_pair_result(document, source, 5, pair, 'Tex')

    def test_native_caret_query_cannot_succeed_after_owner_or_revision_replacement(self):
        source, pair, document = self.pair_observation()

        class Driver(TexEditorFeaturesJourneys):
            def __init__(self, changes):
                self.before = dict(document, cursor=[0, 0])
                self.after = dict(document, **changes)
                self.frames = None

            def snapshot(self):
                return dict(document=deepcopy(self.before))

            def tex_editor_caret(self, actual_source, position):
                if actual_source != source or position != 5:
                    raise AssertionError('Wrong native caret request')

            def wait(self, description, predicate):
                if not predicate(self.after):
                    raise AssertionError(description)

            def stable_state(self, predicate, frames):
                self.frames = frames
                if not predicate(self.after):
                    raise AssertionError('Unstable current pair')

        current = Driver({})
        current.tex_editor_expect_pair(source, 5, pair, 'Tex')
        self.assertEqual(current.frames, 4)
        for change in [dict(path='/private/tmp/replacement.tex'), dict(active_tab_id=3), dict(revision='8')]:
            with self.subTest(change=change), self.assertRaises(AssertionError):
                Driver(change).tex_editor_expect_pair(source, 5, pair, 'Tex')

    def test_fold_evidence_has_unique_headers_and_collapsed_subset(self):
        self.assertEqual(tex_editor_folds(dict(fold_headers=[0, 1, 4], fold_collapsed=[0, 4])), ([0, 1, 4], [0, 4]))
        for document in ({}, dict(fold_headers=[], fold_collapsed=[0]),
                         dict(fold_headers=[0, 0], fold_collapsed=[]),
                         dict(fold_headers=[True], fold_collapsed=[]),
                         dict(fold_headers=[0], fold_collapsed=[0, 0]),
                         dict(fold_headers=[-1], fold_collapsed=[])):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                tex_editor_folds(document)

    def test_original_owner_includes_source_caret_dirty_layout_and_pinned_preview(self):
        before = dict(path='/tmp/journey.tex', tabs=2, active_tab_id=1,
                      preview_tab_id=0, preview_path='/tmp/pinned.typ',
                      source_fingerprint=fingerprint('α'), source_bytes=2,
                      cursor=[1, 1], dirty=False, view_mode='Split', revision='2')
        after = dict(before, revision='3')
        self.assertTrue(tex_editor_owner_restored(after, before))
        self.assertFalse(tex_editor_owner_restored(before, before))
        changes = dict(path='/tmp/other.tex', tabs=3, active_tab_id=2,
                       preview_tab_id=1, preview_path='/tmp/journey.tex',
                       source_fingerprint=fingerprint('β'), source_bytes=3,
                       cursor=[0, 1], dirty=True, view_mode='Code')
        for field, value in changes.items():
            with self.subTest(field=field):
                self.assertFalse(tex_editor_owner_restored(dict(after, **{field: value}), before))
        for field, value in [('cursor', [True, 1]), ('source_bytes', True),
                             ('tabs', False), ('active_tab_id', None),
                             ('source_fingerprint', 'partial'), ('revision', 'bad')]:
            with self.subTest(field=field), self.assertRaises(AssertionError):
                tex_editor_owner_restored(dict(after, **{field: value}), before)
        incomplete = deepcopy(after)
        del incomplete['preview_path']
        with self.assertRaises(AssertionError):
            tex_editor_owner_restored(incomplete, before)

    def test_fixture_and_native_path_exercise_real_input_without_opening_a_browser(self):
        self.assertIn('α+(β)', TEX_EDITOR_SOURCE)
        self.assertIn('\\newcommand', TEX_EDITOR_SOURCE)
        self.assertIn('\\begin{align}', TEX_EDITOR_SOURCE)
        self.assertTrue(TEX_EDITOR_SOURCE.endswith('\n'))
        source = inspect.getsource(TexEditorFeaturesJourneys.tex_editor_features)
        self.assertIn("self.native('text', '{')", source)
        self.assertIn("self.native('text', '}')", source)
        self.assertIn("self.key(51, 'none')", source)
        self.assertIn("self.key(6, 'cmd+shift')", source)
        self.assertIn("self.wait_target('menu.Open Link in Browser')", source)
        self.assertNotIn("self.click('menu.Open Link in Browser')", source)
        self.assertIn('assert_preferences_preserved', source)
        self.assertIn('tex_editor_owner_restored', source)

    def test_unchanged_original_save_observes_disk_receipt_despite_replaced_notice(self):
        class Driver(TexEditorFeaturesJourneys):
            def __init__(self, path, document):
                self.path, self.document = path, document
                self.keys, self.records = [], []

            def key(self, code):
                self.keys.append(code)
                replacement = self.path.with_suffix('.replacement')
                replacement.write_bytes(self.path.read_bytes())
                replacement.replace(self.path)

            def wait(self, label, predicate, timeout):
                self.asserted = predicate(self.document)
                if not self.asserted:
                    raise AssertionError(label)
                return dict(document=self.document)

            def record(self, name, **fields):
                self.records.append((name, fields))

        with TemporaryDirectory() as directory:
            path = Path(directory) / 'journey.tex'
            source = '\\section{Original}\n'
            path.write_text(source, encoding='utf-8')
            before = dict(path=str(path), dirty=False,
                          source_fingerprint=fingerprint(source), source_bytes=len(source))
            # None, a reused Saved notice, and the formatter's actual response
            # all carry the same durable Save result.
            for notice in (None, 'Saved journey.tex',
                           'The selected tool did not provide a formatter for this document'):
                with self.subTest(notice=notice):
                    driver = Driver(path, dict(before, completion_notice=notice))
                    driver.tex_editor_restore_file_history(path, before)
                    self.assertEqual(driver.keys, [1])
                    self.assertEqual(path.read_text(encoding='utf-8'), source)
                    self.assertTrue(driver.records[0][1]['durable_write_observed'])
            badness = dict(before, tex_formatter='badness')
            noop = dict(badness, formatting_phase='idle', save_operation=None,
                        completion_notice='No formatting changes')
            driver = Driver(path, noop)
            driver.tex_editor_restore_file_history(path, badness)
            self.assertEqual(driver.records[1][0], 'tex.editor.format_noop')
            for change in (dict(completion_notice='Saved journey.tex'),
                           dict(completion_notice='The selected tool did not provide a formatter'),
                           dict(formatting_phase='requested'),
                           dict(save_operation={'id': 2})):
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    Driver(path, dict(noop, **change)).tex_editor_restore_file_history(path, badness)
            for change in (dict(dirty=True), dict(path=str(path.with_name('other.tex'))),
                           dict(source_fingerprint=fingerprint('changed')),
                           dict(source_bytes=0)):
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    Driver(path, dict(before, **change)).tex_editor_restore_file_history(path, before)
            no_save = Driver(path, dict(before))
            no_save.key = lambda code: no_save.keys.append(code)
            with self.assertRaises(AssertionError):
                no_save.tex_editor_restore_file_history(path, before)


if __name__ == '__main__':
    unittest.main()
