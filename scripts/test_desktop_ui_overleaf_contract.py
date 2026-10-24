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
)


class OverleafJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, connection=False, browser=False,
                     create_browser=False, pending=False, epoch=0,
                     query_fingerprint=fingerprint(INVALID_HTTP), error='HTTPS required')
        state.update(changes)
        return dict(path='/private/tmp/fixture.txt', source_fingerprint=fingerprint('original'),
                    source_bytes=8, revision='1', dirty=False, tabs=1,
                    active_tab_id=4, preview_tab_id=4, preview_path='/private/tmp/fixture.txt',
                    overleaf=state)

    def test_missing_or_malformed_inspection_cannot_pass_as_disconnected(self):
        invalid = ({}, {'overleaf': None}, {'overleaf': {}}, self.document(visible=1),
                   self.document(epoch=True), self.document(epoch=-1),
                   self.document(query_fingerprint='bad'), self.document(error=''),
                   self.document(error=True), self.document(browser=True),
                   self.document(create_browser=True), self.document(pending=True))
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
                        {'connection': True, 'pending': True}, {'epoch': 1}):
            with self.subTest(changes=changes):
                self.assertFalse(overleaf_offline(self.document(**changes), before,
                                                  visible=True, query=INVALID_HTTP, error=True))

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
        self.assertEqual(OVERLEAF_JOURNEYS, ('overleaf_connection',))
        self.assertTrue(issubclass(EditorJourneys, OverleafJourneys))
        self.assertIn('overleaf_connection', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('overleaf_connection'), ['overleaf_connection'])
        self.assertIn('overleaf_connection', runner.selected_journeys('all'))
