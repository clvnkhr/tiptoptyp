"""Autofix native-journey failure detectors; these are not interaction evidence."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_autofix_journeys import AUTOFIX_OWNER_FIELDS, autofix_saved
from desktop_ui_journeys import fingerprint


class AutofixSaveContracts(unittest.TestCase):
    source = 'γ Figure~\\ref{fig:a}.\n'

    def owner(self):
        return dict(path='/private/tmp/autofix.tex', document_kind='Tex', tabs=2,
                    active_tab_id=9, preview_tab_id=4, preview_path='/main.typ',
                    workspace_root_fingerprint=fingerprint('/private/tmp/workspace'))

    def document(self):
        return {**self.owner(), 'autofix_on_save': True, 'autofix': {'phase': 'idle'},
                'source_fingerprint': fingerprint(self.source),
                'source_bytes': len(self.source.encode('utf-8')), 'dirty': False,
                'modal_open': False, 'save_operation': None, 'formatting_phase': 'idle'}

    def test_completed_save_requires_exact_source_owner_bytes_and_preference(self):
        good = self.document()
        self.assertTrue(autofix_saved(good, self.owner(), self.source, True))
        for field, value in [('path', '/private/tmp/other.tex'), ('document_kind', 'Typst'),
                             ('tabs', 1), ('active_tab_id', 4), ('preview_tab_id', 9),
                             ('preview_path', '/private/tmp/autofix.tex'),
                             ('workspace_root_fingerprint', fingerprint('/other')),
                             ('source_fingerprint', fingerprint('uncorrected')),
                             ('source_bytes', len(self.source)), ('autofix_on_save', False),
                             ('dirty', True), ('dirty', 0), ('modal_open', True)]:
            state = {**good, field: value}
            with self.subTest(field=field):
                self.assertFalse(autofix_saved(state, self.owner(), self.source, True))
        disabled = {**good, 'autofix_on_save': False}
        self.assertTrue(autofix_saved(disabled, self.owner(), self.source, False))

    def test_pending_scan_save_or_formatter_never_passes_as_clean_completion(self):
        for change in ({'autofix': {'phase': 'running'}}, {'autofix': {'phase': 'cancelling'}},
                       {'save_operation': {'id': 3}}, {'formatting_phase': 'requested'}):
            with self.subTest(change=change):
                self.assertFalse(autofix_saved({**self.document(), **change}, self.owner(), self.source, True))
        with self.assertRaisesRegex(AssertionError, 'failed before saving'):
            autofix_saved({**self.document(), 'autofix': {'phase': 'failed'}}, self.owner(), self.source, True)

    def test_missing_or_malformed_inspection_is_unavailable(self):
        for value in (None, {}, {'phase': 'unknown'}, {'phase': True}):
            state = {**self.document(), 'autofix': value}
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, 'lifecycle inspection'):
                autofix_saved(state, self.owner(), self.source, True)
        for value in (None, 'true', 1, 0):
            state = {**self.document(), 'autofix_on_save': value}
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, 'on-save inspection'):
                autofix_saved(state, self.owner(), self.source, True)
        for field in (*AUTOFIX_OWNER_FIELDS, 'save_operation', 'formatting_phase', 'dirty'):
            state = self.document()
            state.pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, 'missing ' + field):
                autofix_saved(state, self.owner(), self.source, True)

    def test_owner_receipt_cannot_be_partial_or_use_a_different_field_type(self):
        for field in AUTOFIX_OWNER_FIELDS:
            owner = deepcopy(self.owner())
            owner.pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, 'missing ' + field):
                autofix_saved(self.document(), owner, self.source, True)
        owner = self.owner()
        owner['active_tab_id'] = 1
        state = {**self.document(), 'active_tab_id': True}
        self.assertFalse(autofix_saved(state, owner, self.source, True))


if __name__ == '__main__':
    unittest.main()
