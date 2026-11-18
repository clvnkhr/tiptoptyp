"""Explorer native-journey failure detectors; these are not interaction evidence."""
from copy import deepcopy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_native_scroll import native_scroll_advance
from desktop_ui_explorer_journeys import (
    EXPLORER_FOLDER, EXPLORER_NESTED, EXPLORER_SEED,
    OWNER_FIELDS, ExplorerJourneys, current_explorer_target, explorer_owner_unchanged,
    file_bytes, file_dialog, prepare_explorer_files, tree_bytes,
)


class NativeScrollSetupContracts(unittest.TestCase):
    class Journey:
        def __init__(self, successful_step=None, offset=120, stalled=False):
            self.frame, self.wheels, self.events = 0, 0, []
            self.successful_step, self.offset, self.stalled = successful_step, offset, stalled

        def snapshot(self):
            if not self.stalled:
                self.frame += 1
            offset = self.offset if self.successful_step is not None and self.wheels >= self.successful_step else 20
            return dict(document=dict(frame=self.frame), scrolls=dict(Files=dict(
                frame=self.frame, x=100, y=200, offset=offset, content_height=2000, viewport_height=200)))

        def native(self, command, *arguments):
            self.events.append((command, arguments))
            if command == 'wheel':
                self.wheels += 1

        def record(self, action, **details):
            self.events.append((action, details))

    def run_setup(self, journey):
        clock = iter(index / 10 for index in range(500))
        with patch('desktop_ui_native_scroll.time.monotonic', side_effect=lambda: next(clock)), \
                patch('desktop_ui_native_scroll.time.sleep'):
            return native_scroll_advance(journey, 'Files', lines=-24,
                                         minimum_advance=0, minimum_offset=80)

    def test_lost_step_requires_fresh_frames_and_actual_movement_before_success(self):
        journey = self.Journey(successful_step=2)
        self.assertEqual(self.run_setup(journey), (20, 120, 2))
        self.assertEqual(journey.wheels, 2)
        receipts = [value for action, value in journey.events if action == 'native.scroll.attempt']
        self.assertEqual([(r['attempt'], r['advanced']) for r in receipts], [(1, False), (2, True)])

    def test_no_movement_or_threshold_equality_fails_after_exactly_four_steps(self):
        for journey in (self.Journey(), self.Journey(successful_step=1, offset=80)):
            with self.subTest(offset=journey.offset), self.assertRaisesRegex(AssertionError, 'after 4 native wheel steps'):
                self.run_setup(journey)
            self.assertEqual(journey.wheels, 4)
            self.assertEqual(sum(action == 'native.scroll.attempt' for action, _ in journey.events), 4)

    def test_stalled_viewport_cannot_authorize_wheel_input(self):
        journey = self.Journey(stalled=True)
        with self.assertRaisesRegex(AssertionError, 'fresh frame'):
            self.run_setup(journey)
        self.assertEqual(journey.wheels, 0)


class ExplorerFileJourneyContracts(unittest.TestCase):
    def owner(self):
        return dict(path='/private/fixture/main.txt', active_tab_id=7, tabs=1,
                    source_fingerprint=fingerprint('original'), source_bytes=8,
                    revision='Revision(1)', dirty=False,
                    preview_path='/private/fixture/main.txt', preview_tab_id=7, cursor=[0, 0])

    def dialog(self):
        return dict(create=True, path='/private/fixture/folder',
                    name_fingerprint=fingerprint('untitled.typ'), error=None)

    def test_missing_or_malformed_dialog_observations_cannot_pass_as_closed(self):
        self.assertIsNone(file_dialog({'file_dialog': None}))
        good = self.dialog()
        self.assertEqual(file_dialog({'file_dialog': good}), good)
        invalid = ({}, {'file_dialog': {}}, {'file_dialog': False})
        for state in invalid:
            with self.subTest(state=state), self.assertRaises(AssertionError): file_dialog(state)
        for key, bad in [('create', 1), ('path', 'relative'), ('path', None),
                         ('name_fingerprint', 'raw name'), ('name_fingerprint', None),
                         ('error', ''), ('error', True)]:
            changed = {**good, key: bad}
            with self.subTest(field=key, value=bad), self.assertRaises(AssertionError):
                file_dialog({'file_dialog': changed})
        for key in good:
            changed = dict(good); del changed[key]
            with self.subTest(missing=key), self.assertRaises(AssertionError):
                file_dialog({'file_dialog': changed})

    def test_every_owner_field_is_required_and_real_source_changes_are_detected(self):
        before = self.owner()
        self.assertTrue(explorer_owner_unchanged(before, before))
        for key in OWNER_FIELDS:
            with self.subTest(changed=key):
                self.assertFalse(explorer_owner_unchanged({**before, key: 'changed'}, before))
            missing = dict(before); del missing[key]
            with self.subTest(missing=key), self.assertRaises(AssertionError):
                explorer_owner_unchanged(missing, before)

    def test_selected_folder_visibility_requires_current_enabled_owner_targets(self):
        name = 'explorer.file.explorer-actions/seed.txt'
        target = dict(enabled=True, frame=10, age_ms=1, viewport='ROOT')
        state = {'document': dict(frame=10, viewport='ROOT'), 'targets': {name: target}}
        self.assertTrue(current_explorer_target(state, name))
        for key, bad in [('enabled', False), ('enabled', 1), ('frame', 9), ('frame', True),
                         ('age_ms', 501), ('age_ms', -1), ('age_ms', True),
                         ('viewport', 'other')]:
            changed = deepcopy(state); changed['targets'][name][key] = bad
            with self.subTest(field=key, value=bad):
                self.assertFalse(current_explorer_target(changed, name))
        self.assertFalse(current_explorer_target({'document': state['document'], 'targets': {}}, name))

    def test_fixture_and_recursive_copy_proof_retain_empty_directories_and_binary_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            prepare_explorer_files(directory)
            folder = Path(directory) / EXPLORER_FOLDER
            file_bytes(folder / 'seed.txt', EXPLORER_SEED)
            file_bytes(folder / 'nested' / 'payload.bin', EXPLORER_NESTED)
            self.assertEqual(tree_bytes(folder), {
                'seed.txt': EXPLORER_SEED, 'nested': None, 'nested/payload.bin': EXPLORER_NESTED})
            (folder / 'empty').mkdir()
            self.assertIsNone(tree_bytes(folder)['empty'])
            for bad in (b'', EXPLORER_SEED + b'changed'):
                with self.subTest(expected=bad), self.assertRaises(AssertionError):
                    file_bytes(folder / 'seed.txt', bad)
            (folder / 'link').symlink_to(folder / 'seed.txt')
            with self.assertRaises(AssertionError): file_bytes(folder / 'link', EXPLORER_SEED)
            with self.assertRaises(AssertionError): tree_bytes(folder)

    def test_name_typing_uses_native_field_input_and_rejects_owner_mutation(self):
        case = self
        class Input:
            def __init__(self, corrupt=False):
                self.document = case.owner(); self.document['file_dialog'] = case.dialog()
                self.actions = []; self.corrupt = corrupt
            def click(self, name): self.actions.append(('click', name))
            def wait_target(self, name, focused): case.assertEqual((name, focused), ('rename.name', True))
            def key(self, key): case.assertEqual(key, 0)
            def native(self, command, text):
                self.actions.append((command, text))
                self.document['file_dialog']['name_fingerprint'] = fingerprint(text)
                if self.corrupt: self.document['source_fingerprint'] = fingerprint('wrong')
            def wait(self, description, predicate):
                if not predicate(self.document): raise AssertionError('source owner changed')
        good = Input()
        ExplorerJourneys.explorer_type_name(good, 'created.typ', self.owner())
        self.assertEqual(good.actions, [('click', 'rename.name'), ('text', 'created.typ')])
        with self.assertRaises(AssertionError):
            ExplorerJourneys.explorer_type_name(Input(corrupt=True), 'created.typ', self.owner())

if __name__ == '__main__': unittest.main()
