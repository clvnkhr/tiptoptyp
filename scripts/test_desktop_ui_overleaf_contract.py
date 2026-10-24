"""Overleaf journey failure detectors; these checks are not native evidence."""
import importlib.util
import ast
from types import SimpleNamespace
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import EditorJourneys, fingerprint
from desktop_ui_overleaf_journeys import (
    INVALID_HTTP, OVERLEAF_JOURNEYS, OWNER_FIELDS, OverleafJourneys,
    overleaf_offline, overleaf_owner_unchanged, overleaf_state,
    overleaf_storage, overleaf_storage_targets, overleaf_reconcile_state,
    overleaf_source_preserved, overleaf_chooser_state, overleaf_review_pair, overleaf_review_docked,
    overleaf_preview_ready,
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

    def test_storage_fixture_requires_actual_visible_tinymist_readiness_and_unchanged_source(self):
        from desktop_ui_journeys import PREVIEW_SOURCE
        fixture = Path('/private/tmp/journey.typ')
        document = dict(path=str(fixture), document_kind='Typst', source_fingerprint=fingerprint(PREVIEW_SOURCE),
                        source_bytes=len(PREVIEW_SOURCE.encode()), active_tab_id=0, preview_tab_id=0,
                        preview_path=str(fixture), preview_backend='Tinymist',
                        preview_requested_backend='Interactive', preview_native_ready=True,
                        preview_surface_visible=True, preview_pages=3, compile_indicator_spinning=False)
        self.assertTrue(overleaf_preview_ready(document, fixture))
        partial = {**document, 'preview_requested_backend': 'InteractivePartial', 'preview_backend': 'Tinymist partial'}
        self.assertTrue(overleaf_preview_ready(partial, fixture))
        for changed in ({**document, 'preview_requested_backend': 'InteractivePartial'},
                        {**partial, 'preview_requested_backend': 'Interactive'},
                        {**document, 'preview_requested_backend': 'Pdfium'}):
            self.assertFalse(overleaf_preview_ready(changed, fixture))
        for field, value in (('path', '/private/tmp/other.typ'), ('document_kind', 'Text'),
                             ('source_fingerprint', fingerprint('changed')), ('source_bytes', 0),
                             ('preview_tab_id', 2), ('preview_path', '/private/tmp/.tiptoptyp/untitled.txt'),
                             ('preview_backend', 'PDFium'), ('preview_native_ready', False),
                             ('preview_surface_visible', False), ('preview_pages', 0),
                             ('compile_indicator_spinning', True)):
            with self.subTest(field=field):
                self.assertFalse(overleaf_preview_ready({**document, field: value}, fixture))
        for field in document:
            missing = dict(document)
            del missing[field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_preview_ready(missing, fixture)
        with self.assertRaises(AssertionError):
            overleaf_preview_ready({**document, 'preview_native_ready': 1}, fixture)

    def test_storage_runner_selects_the_real_typst_preview_fixture(self):
        script = Path(__file__).with_name('test-desktop-ui.py')
        spec = importlib.util.spec_from_file_location('overleaf_fixture_desktop_runner', script)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        main = next(node for node in ast.parse(script.read_text()).body
                    if isinstance(node, ast.FunctionDef) and node.name == 'main')
        assignments = [node for node in ast.walk(main) if isinstance(node, ast.Assign)
                       and any(isinstance(target, ast.Name) and target.id == 'fixture' for target in node.targets)
                       and isinstance(node.value, ast.BinOp) and isinstance(node.value.op, ast.Div)
                       and isinstance(node.value.right, ast.IfExp)]
        self.assertEqual(len(assignments), 1)
        selection = compile(ast.Expression(assignments[0].value.right), str(script), 'eval')
        name = eval(selection, {**vars(runner), 'args': SimpleNamespace(journey='overleaf_storage')})
        self.assertEqual(name, 'journey.typ')

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

    def connected_document(self):
        document = self.document(visible=False, connection=True, browser=True, epoch=2, error=None,
                                 sync_ready=True, source_locked=False, blocked_count=0,
                                 review=dict(open=False, loading=False, left=None, right=None, error=None))
        document.update(cursor=[8, 4], dirty=True, workspace_chooser_visible=False,
                        workspace_root_fingerprint=fingerprint('/private/tmp/remote'))
        return document

    def test_chooser_roundtrip_requires_real_mode_root_connection_and_every_source_field(self):
        before = self.connected_document()
        local = deepcopy(before)
        local['workspace_chooser_visible'] = True
        self.assertTrue(overleaf_chooser_state(local, before, local=True, remote=False,
                                              root=Path('/private/tmp/remote')))
        remote = deepcopy(before)
        remote['overleaf']['visible'] = remote['overleaf']['storage_directory_visible'] = True
        self.assertTrue(overleaf_chooser_state(remote, before, local=False, remote=True,
                                              root=Path('/private/tmp/remote')))
        selected = deepcopy(before)
        selected['workspace_root_fingerprint'] = fingerprint('/private/tmp/local')
        self.assertTrue(overleaf_chooser_state(selected, before, local=False, remote=False,
                                              root=Path('/private/tmp/local')))
        for field, value in (('path', '/private/tmp/other.txt'), ('source_fingerprint', fingerprint('other')),
                             ('source_bytes', 9), ('dirty', False), ('tabs', 2), ('active_tab_id', 5),
                             ('preview_tab_id', 5), ('preview_path', '/private/tmp/other.txt'),
                             ('cursor', [4, 8])):
            changed = deepcopy(local)
            changed[field] = value
            with self.subTest(field=field):
                self.assertFalse(overleaf_chooser_state(changed, before, local=True, remote=False,
                                                       root=Path('/private/tmp/remote')))
        for field, value in (('epoch', 3), ('pending', True), ('sync_ready', False), ('browser', False)):
            changed = deepcopy(local)
            changed['overleaf'][field] = value
            with self.subTest(connection=field):
                self.assertFalse(overleaf_chooser_state(changed, before, local=True, remote=False,
                                                       root=Path('/private/tmp/remote')))
        self.assertFalse(overleaf_chooser_state(local, before, local=False, remote=False,
                                               root=Path('/private/tmp/remote')))
        self.assertFalse(overleaf_chooser_state(local, before, local=True, remote=False,
                                               root=Path('/private/tmp/wrong')))
        self.assertEqual(before, self.connected_document())

    def test_service_rekey_is_allowed_but_missing_or_malformed_chooser_ownership_is_not(self):
        before = self.connected_document()
        changed = deepcopy(before)
        changed['revision'] = 'new provider generation'
        self.assertTrue(overleaf_source_preserved(changed, before))
        for field in ('path', 'source_fingerprint', 'source_bytes', 'dirty', 'tabs',
                      'active_tab_id', 'preview_tab_id', 'preview_path', 'cursor'):
            missing = deepcopy(before)
            del missing[field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_source_preserved(missing, before)
        for field, value in (('dirty', 1), ('tabs', True), ('active_tab_id', True),
                             ('source_fingerprint', 'not a hash'), ('cursor', None),
                             ('cursor', [True, 0]), ('path', None)):
            malformed = deepcopy(before)
            malformed[field] = value
            with self.subTest(malformed=field), self.assertRaises(AssertionError):
                overleaf_source_preserved(malformed, before)
        for field, value in (('workspace_chooser_visible', 1), ('workspace_root_fingerprint', 'raw path')):
            invalid = deepcopy(before)
            invalid[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                overleaf_chooser_state(invalid, before, local=False, remote=False,
                                       root=Path('/private/tmp/remote'))

    def test_review_pair_requires_exact_complete_sides_and_current_conflict_readiness(self):
        document = self.connected_document()
        state = document['overleaf']
        state.update(blocked_count=1, source_locked=True)
        state['review'].update(open=True, left=fingerprint('whole remote'), right=fingerprint('whole local'))
        self.assertTrue(overleaf_review_pair(document, 'whole remote', 'whole local'))
        self.assertFalse(overleaf_review_pair(document, 'remote fragment', 'whole local'))
        self.assertFalse(overleaf_review_pair(document, 'whole remote', 'local fragment'))
        for field, value in (('blocked_count', 0), ('sync_ready', False), ('source_locked', False), ('pending', True)):
            changed = deepcopy(document)
            changed['overleaf'][field] = value
            with self.subTest(field=field):
                self.assertFalse(overleaf_review_pair(changed, 'whole remote', 'whole local'))
        for field, value in (('open', False), ('loading', True), ('left', None), ('right', None), ('error', 'failed')):
            changed = deepcopy(document)
            changed['overleaf']['review'][field] = value
            with self.subTest(review=field):
                self.assertFalse(overleaf_review_pair(changed, 'whole remote', 'whole local'))

    def test_review_geometry_requires_a_current_root_comparison_inside_its_code_panel(self):
        document = self.connected_document()
        document.update(frame=42, viewport='root', preview_surface_visible=True)
        document['overleaf']['review'].update(open=True, left=fingerprint('remote'), right=fingerprint('local'))
        def target(x, y, width, height):
            return dict(frame=42, viewport='root', age_ms=0, x=x, y=y, width=width, height=height)
        state = {'document': document, 'targets': {
            'overleaf.review.panel': target(200, 250, 300, 400),
            'overleaf.review.diff': target(200, 280, 280, 300),
            'preview.page': target(500, 250, 200, 400)}}
        self.assertTrue(overleaf_review_docked(state))
        missing_preview = deepcopy(state)
        del missing_preview['targets']['preview.page']
        with self.assertRaisesRegex(AssertionError, 'preview lacks'):
            overleaf_review_docked(missing_preview)
        for name in ('overleaf.review.panel', 'overleaf.review.diff', 'preview.page'):
            for field, value in (('frame', 41), ('viewport', 'child'), ('age_ms', 501),
                                 ('width', 0), ('x', float('nan'))):
                changed = deepcopy(state)
                changed['targets'][name][field] = value
                with self.subTest(target=name, field=field), self.assertRaises(AssertionError):
                    overleaf_review_docked(changed)
        for field, value in (('x', 400), ('y', 500)):
            changed = deepcopy(state)
            changed['targets']['overleaf.review.diff'][field] = value
            with self.subTest(escaped=field), self.assertRaises(AssertionError):
                overleaf_review_docked(changed)
        covered = deepcopy(state)
        covered['targets']['overleaf.review.panel']['width'] = 600
        with self.assertRaisesRegex(AssertionError, 'covered'):
            overleaf_review_docked(covered)
        state['document']['overleaf']['review']['loading'] = True
        with self.assertRaisesRegex(AssertionError, 'not ready'):
            overleaf_review_docked(state)

    def test_reconciliation_cannot_pass_without_explicit_valid_observation(self):
        document = self.connected_document()
        for field in ('sync_ready', 'source_locked', 'blocked_count', 'review'):
            missing = deepcopy(document)
            del missing['overleaf'][field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_reconcile_state(missing)
        for field, value in (('sync_ready', 1), ('source_locked', 0), ('blocked_count', True),
                             ('blocked_count', -1), ('review', None)):
            changed = deepcopy(document)
            changed['overleaf'][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                overleaf_reconcile_state(changed)
        for field, value in (('open', 1), ('loading', True), ('left', 'not a hash'), ('error', '')):
            changed = deepcopy(document)
            changed['overleaf']['review'][field] = value
            with self.subTest(review=field), self.assertRaises(AssertionError):
                overleaf_reconcile_state(changed)

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
