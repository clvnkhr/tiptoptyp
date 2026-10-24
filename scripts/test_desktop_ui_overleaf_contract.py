"""Overleaf journey failure detectors; these checks are not native evidence."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import EditorJourneys, fingerprint
from desktop_ui_overleaf_journeys import (
    INVALID_HTTP, OVERLEAF_JOURNEYS, OWNER_FIELDS, OverleafJourneys,
    overleaf_offline, overleaf_owner_unchanged, overleaf_state,
    overleaf_storage, overleaf_storage_targets,
)


class OverleafJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, connection=False, browser=False,
                     create_browser=False, pending=False, epoch=0,
                     query_fingerprint=fingerprint(INVALID_HTTP), error='HTTPS required',
                     storage_preparing=False, storage_keep_local_copy=True,
                     storage_directory_fingerprint=fingerprint(''), storage_directory_visible=True)
        state.update(changes)
        if 'storage_directory_visible' not in changes:
            state['storage_directory_visible'] = state['visible'] and state['storage_keep_local_copy']
        return dict(path='/private/tmp/fixture.txt', source_fingerprint=fingerprint('original'),
                    source_bytes=8, revision='1', dirty=False, tabs=1,
                    active_tab_id=4, preview_tab_id=4, preview_path='/private/tmp/fixture.txt',
                    overleaf=state)

    def test_missing_or_malformed_inspection_cannot_pass_as_disconnected(self):
        invalid = ({}, {'overleaf': None}, {'overleaf': {}}, self.document(visible=1),
                   self.document(epoch=True), self.document(epoch=-1),
                   self.document(query_fingerprint='bad'), self.document(error=''),
                   self.document(error=True), self.document(browser=True),
                   self.document(create_browser=True), self.document(pending=True),
                   self.document(storage_keep_local_copy=1),
                   self.document(storage_preparing=1),
                   self.document(storage_directory_fingerprint='raw folder'),
                   self.document(storage_directory_fingerprint=None),
                   self.document(storage_directory_visible=1),
                   self.document(visible=False, storage_directory_visible=True),
                   self.document(storage_keep_local_copy=False, storage_directory_visible=True))
        for document in invalid:
            with self.subTest(document=document), self.assertRaises(AssertionError):
                overleaf_state(document)
        self.assertFalse(overleaf_state(self.document(visible=False))['visible'])
        # Hidden authenticated connections are valid in general, but not an
        # offline journey proving that an invalid URL never reached a browser.
        connected = self.document(visible=False, connection=True, browser=True, epoch=2)
        self.assertTrue(overleaf_state(connected)['browser'])
        self.assertFalse(overleaf_offline(connected, self.document(), visible=False))

    def test_every_owner_field_is_required_and_changes_are_detected(self):
        before = self.document()
        self.assertTrue(overleaf_owner_unchanged(before, before))
        for field in OWNER_FIELDS:
            changed = {**before, field: 'different'}
            with self.subTest(field=field):
                self.assertFalse(overleaf_owner_unchanged(changed, before))
            absent = dict(before)
            absent.pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_owner_unchanged(absent, before)

    def test_invalid_connection_proof_requires_field_fingerprint_error_and_no_allocations(self):
        before = self.document()
        self.assertTrue(overleaf_offline(before, before, visible=True, query=INVALID_HTTP, error=True))
        self.assertFalse(overleaf_offline(before, before, visible=False, query=INVALID_HTTP))
        self.assertFalse(overleaf_offline(before, before, visible=True, query='different'))
        self.assertFalse(overleaf_offline(self.document(error=None), before,
                                          visible=True, query=INVALID_HTTP, error=True))
        for changes in ({'connection': True}, {'connection': True, 'browser': True},
                        {'connection': True, 'create_browser': True},
                        {'connection': True, 'pending': True}, {'epoch': 1},
                        {'storage_preparing': True}):
            with self.subTest(changes=changes):
                self.assertFalse(overleaf_offline(self.document(**changes), before,
                                                  visible=True, query=INVALID_HTTP, error=True))

    def test_storage_proof_requires_exact_folder_and_mode_observations(self):
        document = self.document(storage_directory_fingerprint=fingerprint('/private/copies'))
        self.assertTrue(overleaf_storage(document, keep=True, directory='/private/copies'))
        self.assertFalse(overleaf_storage(document, keep=False, directory='/private/copies'))
        self.assertFalse(overleaf_storage(document, keep=True, directory='/wrong'))
        for field in ('storage_preparing', 'storage_keep_local_copy',
                      'storage_directory_visible', 'storage_directory_fingerprint'):
            missing = self.document()
            del missing['overleaf'][field]
            with self.subTest(field=field), self.assertRaises(AssertionError):
                overleaf_state(missing)

    def test_storage_hit_targets_must_be_current_and_hidden_targets_may_only_be_stale(self):
        checkbox = dict(enabled=True, frame=20, age_ms=1, viewport='child')
        directory = dict(checkbox)
        folder = dict(checkbox)
        state = {'targets': {'overleaf.keep_local_copy': checkbox,
                             'overleaf.directory': directory,
                             'overleaf.choose_folder': folder}}
        self.assertTrue(overleaf_storage_targets(state, keep=True))
        self.assertFalse(overleaf_storage_targets(state, keep=False))
        for field, bad in (('enabled', False), ('age_ms', 501), ('frame', 19),
                           ('viewport', 'other')):
            previous = directory[field]
            directory[field] = bad
            with self.subTest(field=field):
                self.assertFalse(overleaf_storage_targets(state, keep=True))
            directory[field] = previous
        directory['frame'] = folder['frame'] = 19
        self.assertTrue(overleaf_storage_targets(state, keep=False))
        directory['frame'] = 20
        self.assertFalse(overleaf_storage_targets(state, keep=False))
        state['targets'] = {'overleaf.keep_local_copy': checkbox}
        self.assertTrue(overleaf_storage_targets(state, keep=False))
        self.assertFalse(overleaf_storage_targets(state, keep=True))
        self.assertFalse(overleaf_storage_targets({'targets': {}}, keep=False))
        for field, bad in (('age_ms', 501), ('frame', -1), ('frame', True),
                           ('viewport', None), ('viewport', '')):
            previous = checkbox[field]
            checkbox[field] = bad
            with self.subTest(checkbox=field, value=bad):
                self.assertFalse(overleaf_storage_targets(state, keep=False))
            checkbox[field] = previous

    def test_native_opener_rejects_wrong_or_disabled_palette_action_before_enter(self):
        class Fake(OverleafJourneys):
            def __init__(self, selected, enabled):
                self.selected, self.enabled, self.keys = selected, enabled, []
            def focus_document_window(self): pass
            def palette_open(self): pass
            def palette_query(self, query):
                return {'palette_selected': self.selected, 'palette_enabled': self.enabled}
            def key(self, code, flags='cmd'):
                self.keys.append((code, flags))
        for selected, enabled in [('file.open', ['file.open']), ('file.overleaf', []), (None, [])]:
            fake = Fake(selected, enabled)
            with self.subTest(selected=selected), self.assertRaises(AssertionError):
                fake.open_overleaf_connection()
            self.assertEqual(fake.keys, [])

    def test_undo_uses_the_known_first_field_boundary_and_keeps_owner_checks(self):
        case = self
        class Undo(OverleafJourneys):
            def __init__(self, change_owner=False):
                self.before = case.document()
                self.document = case.document()
                self.change_owner, self.keys = change_owner, []
            def wait_target(self, name, focused=False):
                case.assertEqual((name, focused), ('overleaf.url', True))
            def key(self, code, flags='cmd'):
                self.keys.append((code, flags))
                self.document['overleaf']['query_fingerprint'] = fingerprint('' if flags == 'cmd' else INVALID_HTTP)
                if self.change_owner:
                    self.document['source_fingerprint'] = fingerprint('wrong editor changed')
            def wait(self, description, predicate):
                if not predicate(self.document):
                    raise AssertionError(description)
        journey = Undo()
        journey.overleaf_undo_initial_query(journey.before, INVALID_HTTP)
        self.assertEqual(journey.keys, [(6, 'cmd'), (6, 'cmd+shift')])
        changed = Undo(change_owner=True)
        with self.assertRaises(AssertionError):
            changed.overleaf_undo_initial_query(changed.before, INVALID_HTTP)

    def test_offline_journey_is_registered_in_existing_runner_and_aggregate(self):
        spec = importlib.util.spec_from_file_location('overleaf_desktop_runner', Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertEqual(OVERLEAF_JOURNEYS, ('overleaf_connection', 'overleaf_storage'))
        self.assertTrue(issubclass(EditorJourneys, OverleafJourneys))
        self.assertIn('overleaf_connection', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('overleaf_connection'), ['overleaf_connection'])
        self.assertIn('overleaf_connection', runner.selected_journeys('all'))
        self.assertIn('overleaf_storage', runner.JOURNEY_NAMES)
        self.assertIn('overleaf_storage', runner.selected_journeys('all'))
