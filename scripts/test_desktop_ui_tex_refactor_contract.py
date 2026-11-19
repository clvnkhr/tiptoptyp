"""Refactoring observation failure detectors; no native interaction is claimed."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_tex_refactor_journeys import (
    REFACTOR_OWNER_FIELDS, REFACTOR_SOURCE, REFACTOR_WORKING, REFACTOR_INLINE,
    REFACTOR_ALL, REFACTOR_BOUNDARY, REFACTOR_EXTRACTED,
    refactor_state, refactor_source, refactor_review, refactor_controls,
    refactor_selection_point,
)


class TexRefactorObservationContracts(unittest.TestCase):
    def document(self, phase='reviewing'):
        return dict(path='/private/tmp/refactor.tex', document_kind='Tex', tabs=2,
                    active_tab_id=7, preview_tab_id=4, preview_path='/private/tmp/main.typ',
                    workspace_root_fingerprint=fingerprint('/private/tmp/workspace'),
                    source_fingerprint=fingerprint(REFACTOR_WORKING),
                    source_bytes=len(REFACTOR_WORKING.encode('utf-8')), dirty=True,
                    revision='Revision(8)', viewport='document-viewport', frame=77,
                    tex_refactor=dict(phase=phase, visible=phase != 'closed',
                                      current=phase not in ('closed', 'stale'),
                                      comparison=(dict(before=fingerprint(REFACTOR_WORKING),
                                                       after=fingerprint(REFACTOR_INLINE),
                                                       occurrences=1)
                                                  if phase == 'reviewing' else None)))

    def snapshot(self, accepted=True):
        target = dict(x=200., y=100., width=100., height=30., age_ms=10,
                      enabled=True, frame=20, viewport='refactor-child')
        targets = {name: dict(target) for name in
                   ('tex_refactor.accept', 'tex_refactor.cancel', 'tex_refactor.diff')}
        targets['tex_refactor.accept']['enabled'] = accepted
        if not accepted:
            targets.pop('tex_refactor.diff')
        return dict(document=self.document('reviewing' if accepted else 'rejected'), targets=targets)

    def test_exact_full_document_preview_count_and_source_owner_are_required(self):
        owner = self.document()
        self.assertTrue(refactor_review(self.document(), owner,
                                        REFACTOR_WORKING, REFACTOR_INLINE, 1))
        for field, value in [('path', '/different.tex'), ('document_kind', 'Typst'),
                             ('tabs', 1), ('active_tab_id', 9), ('preview_tab_id', 7),
                             ('preview_path', '/different.typ'),
                             ('workspace_root_fingerprint', fingerprint('/different')),
                             ('source_fingerprint', fingerprint(REFACTOR_INLINE)),
                             ('source_bytes', len(REFACTOR_WORKING)),
                             ('revision', 'Revision(9)'), ('dirty', False)]:
            with self.subTest(field=field):
                self.assertFalse(refactor_review({**self.document(), field: value}, owner,
                                                 REFACTOR_WORKING, REFACTOR_INLINE, 1))
        for field, value in [('before', fingerprint(REFACTOR_INLINE)),
                             ('after', fingerprint(REFACTOR_ALL)), ('occurrences', 2)]:
            document = self.document()
            document['tex_refactor']['comparison'][field] = value
            with self.subTest(comparison=field):
                self.assertFalse(refactor_review(document, owner,
                                                 REFACTOR_WORKING, REFACTOR_INLINE, 1))

    def test_owner_receipt_must_be_complete_and_preserve_field_types(self):
        for field in REFACTOR_OWNER_FIELDS:
            for missing in ('owner', 'document'):
                owner, document = self.document(), self.document()
                (owner if missing == 'owner' else document).pop(field)
                with self.subTest(field=field, missing=missing), self.assertRaisesRegex(AssertionError, 'missing ' + field):
                    refactor_source(document, owner, REFACTOR_WORKING)
        self.assertFalse(refactor_source({**self.document(), 'active_tab_id': True},
                                         {**self.document(), 'active_tab_id': 1}, REFACTOR_WORKING))
        for field in ('revision', 'dirty'):
            document = self.document()
            document.pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, 'revision or dirty'):
                refactor_review(document, self.document(), REFACTOR_WORKING, REFACTOR_INLINE, 1)

    def test_lifecycle_visibility_and_ownership_cannot_disagree(self):
        for phase in ('closed', 'naming', 'reviewing', 'rejected', 'stale'):
            self.assertEqual(refactor_state(self.document(phase))['phase'], phase)
        for state in (None, {}, dict(phase='unknown', visible=True, current=True),
                      dict(phase='closed', visible=True, current=False, comparison=None),
                      dict(phase='stale', visible=True, current=True, comparison=None),
                      dict(phase='reviewing', visible=True, current=False, comparison=None),
                      dict(phase='naming', visible=1, current=True, comparison=None)):
            with self.subTest(state=state), self.assertRaises(AssertionError):
                refactor_state({**self.document(), 'tex_refactor': state})
        for comparison in (None, {}, dict(before='a', after='b', occurrences=True),
                           dict(before='a', after='b', occurrences=0),
                           dict(before='', after='b', occurrences=1)):
            document = self.document()
            document['tex_refactor']['comparison'] = comparison
            with self.subTest(comparison=comparison), self.assertRaisesRegex(AssertionError, 'comparison inspection'):
                refactor_state(document)
        document = self.document('naming')
        document['tex_refactor']['comparison'] = {'before': 'a', 'after': 'b', 'occurrences': 1}
        with self.assertRaisesRegex(AssertionError, 'Only a TeX refactoring review'):
            refactor_state(document)

    def test_diff_and_footer_must_be_fresh_enabled_and_owned_by_one_child_frame(self):
        self.assertTrue(refactor_controls(self.snapshot(), accepted=True))
        self.assertTrue(refactor_controls(self.snapshot(False), accepted=False))
        for name, field, value in [
                ('tex_refactor.accept', 'enabled', False),
                ('tex_refactor.cancel', 'enabled', False),
                ('tex_refactor.diff', 'enabled', False),
                ('tex_refactor.diff', 'frame', 19),
                ('tex_refactor.cancel', 'viewport', 'other-child'),
                ('tex_refactor.accept', 'age_ms', 501),
                ('tex_refactor.accept', 'height', -212.0),
                ('tex_refactor.cancel', 'age_ms', True),
                ('tex_refactor.diff', 'width', 0),
                ('tex_refactor.diff', 'height', float('nan'))]:
            state = self.snapshot()
            state['targets'][name][field] = value
            with self.subTest(name=name, field=field):
                self.assertFalse(refactor_controls(state, accepted=True))
        for name in ('tex_refactor.accept', 'tex_refactor.cancel', 'tex_refactor.diff'):
            state = self.snapshot()
            state['targets'].pop(name)
            self.assertFalse(refactor_controls(state, accepted=True))
        state = self.snapshot()
        for target in state['targets'].values():
            target['viewport'] = state['document']['viewport']
        self.assertFalse(refactor_controls(state, accepted=True))

    def test_selection_menu_hit_belongs_to_the_exact_visible_keyboard_selection(self):
        state = self.snapshot()
        state['document'].update(cursor=[30, 20], editor_keyboard_focused=True)
        target = dict(x=200., y=150., width=300., height=100., age_ms=10,
                      enabled=True, frame=77, viewport='document-viewport')
        state['targets'].update({'editor.source': dict(target),
                                 'editor.ime_cursor': {**target, 'x': 180.}})
        self.assertEqual(refactor_selection_point(state, 20, 30), (175., 150.))
        for name, field, value in [('editor.source', 'frame', 76),
                                   ('editor.ime_cursor', 'viewport', 'other-editor'),
                                   ('editor.ime_cursor', 'age_ms', 501),
                                   ('editor.source', 'width', float('nan')),
                                   ('editor.ime_cursor', 'x', 900.)]:
            changed = deepcopy(state)
            changed['targets'][name][field] = value
            with self.subTest(name=name, field=field), self.assertRaises(AssertionError):
                refactor_selection_point(changed, 20, 30)
        state['document']['cursor'] = [31, 20]
        with self.assertRaisesRegex(AssertionError, 'exact focused selection'):
            refactor_selection_point(state, 20, 30)

    def test_native_expected_outputs_preserve_scripts_tokens_and_protected_text(self):
        self.assertIn('$ab^2_i + a^\\sq{xy}$', REFACTOR_INLINE)
        self.assertIn('$ab^2_i + a^xy^2$', REFACTOR_ALL)
        self.assertIn('$\\alpha x_i$', REFACTOR_BOUNDARY)
        for expected in (REFACTOR_INLINE, REFACTOR_ALL, REFACTOR_BOUNDARY, REFACTOR_EXTRACTED):
            self.assertIn('% Protected \\sq{comment}', expected)
            self.assertIn('\\verb|\\sq{raw}|', expected)
            self.assertTrue(expected.endswith('% native refactor undo sentinel\n'))
        self.assertIn('\\newcommand{\\energy}{x_i+\\alpha}%\n\\begin{document}', REFACTOR_EXTRACTED)
        self.assertIn('$a^{\\energy}+z$', REFACTOR_EXTRACTED)
        self.assertNotEqual(REFACTOR_SOURCE, REFACTOR_WORKING)


if __name__ == '__main__':
    unittest.main()
