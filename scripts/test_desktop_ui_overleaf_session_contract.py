"""Failure detectors for durable native sessions; not real-app interaction proof."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_overleaf_session_journeys import overleaf_session_state, overleaf_offline_saved


class OverleafSessionContracts(unittest.TestCase):
    def document(self):
        source = 'Saved local α.\n'
        root = Path('/fixture/workspace/project-copy')
        return root, source, dict(
            path=str(root / 'main.txt'), source_fingerprint=fingerprint(source),
            source_bytes=len(source.encode('utf-8')), dirty=False, modal_open=False,
            workspace_root_fingerprint=fingerprint(str(root)),
            overleaf=dict(visible=False, connection=True, browser=False, create_browser=False,
                          pending=False, epoch=3, query_fingerprint=fingerprint('project'),
                          browser_url_fingerprint=None, browser_start_url_fingerprint=None,
                          storage_preparing=False, storage_keep_local_copy=True,
                          storage_directory_fingerprint=fingerprint('/fixture/workspace'),
                          storage_controls_visible=True, storage_directory_visible=False,
                          error=None, sync_ready=False, source_locked=False, blocked_count=0,
                          review=dict(open=False, loading=False, left=None, right=None, error=None),
                          online=False, offline_mode=True, restored_session=True,
                          session_saved=True, local_save_pending=False,
                          profile_fingerprint=fingerprint('opaque-profile')))

    def test_offline_save_requires_exact_local_owner_saved_source_and_restored_profile(self):
        root, source, document = self.document()
        profile = document['overleaf']['profile_fingerprint']
        self.assertTrue(overleaf_offline_saved(document, root=root, source=source,
                                              profile=profile, restored=True))
        for change in ({'path': '/fixture/other.txt'}, {'source_bytes': len(source)},
                       {'dirty': True}, {'modal_open': True},
                       {'source_fingerprint': fingerprint('remote source')},
                       {'workspace_root_fingerprint': fingerprint(str(root.parent))}):
            with self.subTest(change=change):
                self.assertFalse(overleaf_offline_saved({**document, **change}, root=root,
                                                       source=source, profile=profile, restored=True))

    def test_incomplete_checkpoint_and_wrong_profile_cannot_pass_as_restored_offline_save(self):
        root, source, document = self.document()
        profile = document['overleaf']['profile_fingerprint']
        for change in ({'session_saved': False}, {'restored_session': False}, {'pending': True},
                       {'source_locked': True}, {'connection': False},
                       {'profile_fingerprint': fingerprint('other-workspace')},
                       {'offline_mode': False}, {'local_save_pending': True, 'session_saved': False}):
            changed = deepcopy(document)
            changed['overleaf'].update(change)
            with self.subTest(change=change):
                self.assertFalse(overleaf_offline_saved(changed, root=root, source=source,
                                                       profile=profile, restored=True))

    def test_missing_or_malformed_persistence_evidence_is_an_error(self):
        _, _, document = self.document()
        fields = ('offline_mode', 'restored_session', 'session_saved',
                  'local_save_pending', 'profile_fingerprint')
        for field in fields:
            changed = deepcopy(document)
            changed['overleaf'].pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_session_state(changed)
        for field, value in (('offline_mode', 1), ('restored_session', 'yes'),
                             ('session_saved', None), ('local_save_pending', 0),
                             ('profile_fingerprint', 'raw-project-address'),
                             ('profile_fingerprint', None), ('online', True)):
            changed = deepcopy(document)
            changed['overleaf'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                overleaf_session_state(changed)

    def test_a_save_still_in_flight_cannot_report_a_finished_checkpoint(self):
        _, _, document = self.document()
        document['overleaf']['local_save_pending'] = True
        with self.assertRaises(AssertionError):
            overleaf_session_state(document)


if __name__ == '__main__':
    unittest.main()
