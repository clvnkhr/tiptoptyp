"""Failure detectors for native workspace replacement, not interaction evidence."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_workspace_switch_journeys import (
    workspace_tab_ids, workspace_owner_unchanged, workspace_root_is,
    workspace_has_only_destination,
)


class WorkspaceSwitchContracts(unittest.TestCase):
    def before(self):
        return {'tabs': 2, 'tab_order': [1, 2], 'active_tab_id': 2,
                'preview_tab_id': 1, 'path': '/fixture/second.txt',
                'preview_path': '/fixture/first.txt', 'source_fingerprint': fingerprint('source'),
                'source_bytes': 6, 'cursor': [3, 3], 'dirty': False,
                'workspace_root_fingerprint': fingerprint('/fixture')}

    def test_cancel_requires_the_exact_owner_tabs_preview_caret_and_source(self):
        before = self.before()
        self.assertTrue(workspace_owner_unchanged(deepcopy(before), before))
        for field, value in (
                ('tab_order', [2, 1]), ('active_tab_id', 1), ('preview_tab_id', 2),
                ('preview_path', '/elsewhere/source.txt'), ('path', '/fixture/other.txt'),
                ('source_fingerprint', fingerprint('edited')), ('source_bytes', 8),
                ('cursor', [0, 0]), ('dirty', True),
                ('workspace_root_fingerprint', fingerprint('/fixture/next'))):
            with self.subTest(field=field):
                self.assertFalse(workspace_owner_unchanged({**before, field: value}, before))

    def test_cancel_cannot_pass_when_ownership_inspection_is_missing_or_malformed(self):
        before = self.before()
        for field in before:
            current = dict(before); current.pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                workspace_owner_unchanged(current, before)
        for field, value in (('source_bytes', False), ('dirty', 0),
                             ('source_fingerprint', None), ('workspace_root_fingerprint', None)):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                workspace_owner_unchanged({**before, field: value}, before)

    def test_tab_identity_rejects_duplicates_bool_ids_and_partial_counts(self):
        before = self.before()
        for change in ({'tab_order': [1, 1]}, {'tab_order': [True, 2]},
                       {'tabs': 1}, {'tabs': True}, {'tab_order': None}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                workspace_tab_ids({**before, **change})
        self.assertEqual(workspace_tab_ids({'tabs': 0, 'tab_order': []}), [])

    def destination(self):
        root = Path('/fixture/destination')
        path = root / 'source.txt'
        source = 'Saved destination α.\n'
        state = {'tabs': 1, 'tab_order': [9], 'active_tab_id': 9,
                 'path': str(path), 'source_fingerprint': fingerprint(source),
                 'source_bytes': len(source.encode()), 'dirty': False, 'modal_open': False,
                 'workspace_root_fingerprint': fingerprint(str(root))}
        return root, path, source, state

    def test_revisit_requires_only_a_fresh_saved_destination_tab(self):
        root, path, source, state = self.destination()
        self.assertTrue(workspace_has_only_destination(state, root, path, source, [1, 2, 3]))
        for change in (
                {'tab_order': [2], 'active_tab_id': 2},
                {'tabs': 2, 'tab_order': [9, 10]}, {'active_tab_id': 10},
                {'path': '/fixture/old.txt'}, {'source_fingerprint': fingerprint('unsaved draft')},
                {'source_bytes': 1}, {'dirty': True}, {'modal_open': True},
                {'workspace_root_fingerprint': fingerprint('/fixture')}):
            with self.subTest(change=change):
                self.assertFalse(workspace_has_only_destination(
                    {**state, **change}, root, path, source, [1, 2, 3]))

    def test_root_identity_does_not_treat_parent_containment_as_same_workspace(self):
        root, _, _, state = self.destination()
        self.assertTrue(workspace_root_is(state, root))
        self.assertFalse(workspace_root_is(state, root.parent))
        with self.assertRaises(AssertionError):
            workspace_root_is({}, root)


if __name__ == '__main__':
    unittest.main()
