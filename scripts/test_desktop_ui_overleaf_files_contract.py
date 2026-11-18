"""Failure detectors require explicit file-operation evidence, never defaults."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_overleaf_files_journeys import overleaf_files_state, overleaf_file_rows_visible


class FileJourneyContracts(unittest.TestCase):
    def document(self):
        return dict(overleaf=dict(visible=True, connection=True, browser=True, create_browser=False,
                    pending=False, epoch=1, query_fingerprint=fingerprint('project'), error=None,
                    browser_url_fingerprint=fingerprint('project'), browser_start_url_fingerprint=fingerprint('project'),
                    storage_preparing=False, storage_keep_local_copy=False, storage_directory_fingerprint=fingerprint(''),
                    storage_controls_visible=True, storage_directory_visible=False,
                    files=dict(dialog=True, pending=False, writable=True, action='duplicate',
                               name_fingerprint=fingerprint('copy.tex'), parent_fingerprint=fingerprint(''), error=None)))

    def test_hydration_cannot_pass_for_missing_stale_or_clipped_explorer_rows(self):
        paths = ('main.txt', 'fixture.bin')
        state = dict(document=dict(frame=7, viewport='root'), targets={
            'explorer.file.' + path: dict(frame=7, viewport='root', enabled=True,
                                         age_ms=0, width=100, height=22) for path in paths})
        self.assertTrue(overleaf_file_rows_visible(state, paths))
        for mutation in ('missing', 'stale', 'other viewport', 'disabled', 'expired', 'clipped', 'invalid'):
            changed = deepcopy(state)
            row = changed['targets']['explorer.file.fixture.bin']
            if mutation == 'missing':
                del changed['targets']['explorer.file.fixture.bin']
            elif mutation == 'stale':
                row['frame'] -= 1
            elif mutation == 'other viewport':
                row['viewport'] = 'manager'
            elif mutation == 'disabled':
                row['enabled'] = False
            elif mutation == 'expired':
                row['age_ms'] = 501
            elif mutation == 'clipped':
                row['height'] = 0
            else:
                row['width'] = float('nan')
            with self.subTest(mutation=mutation):
                self.assertFalse(overleaf_file_rows_visible(changed, paths))

    def test_file_dialog_requires_every_actual_observation(self):
        document = self.document()
        self.assertEqual(overleaf_files_state(document)['action'], 'duplicate')
        for field in document['overleaf']['files']:
            missing = deepcopy(document)
            del missing['overleaf']['files'][field]
            with self.subTest(field=field), self.assertRaises(AssertionError):
                overleaf_files_state(missing)
        del document['overleaf']['files']
        with self.assertRaises(AssertionError):
            overleaf_files_state(document)

    def test_pending_and_closed_states_cannot_hide_a_dialog_or_malformed_name(self):
        for field, value in [('pending', 1), ('writable', 'yes'), ('action', 'overwrite'),
                             ('name_fingerprint', 'copy.tex'), ('parent_fingerprint', 'notes'),
                             ('action', None), ('name_fingerprint', None)]:
            document = self.document()
            document['overleaf']['files'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                overleaf_files_state(document)
        document = self.document()
        document['overleaf']['files'].update(dialog=False, pending=True)
        with self.assertRaises(AssertionError):
            overleaf_files_state(document)
        document['overleaf']['files'].update(pending=False, action=None, name_fingerprint=None,
                                            parent_fingerprint=None, error=None)
        self.assertFalse(overleaf_files_state(document)['dialog'])


if __name__ == '__main__':
    unittest.main()
