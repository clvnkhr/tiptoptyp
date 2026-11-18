"""Overleaf journey failure detectors; these checks are not native evidence."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_overleaf_journeys import (
    LOGIN_URL, INVALID_SCRIPT, OWNER_FIELDS, OverleafJourneys,
    overleaf_browsing, overleaf_browser_at, overleaf_owner_unchanged, overleaf_state,
    overleaf_storage, overleaf_storage_targets, overleaf_storage_draft_ready, overleaf_projectless_storage,
    overleaf_reconcile_state,
    overleaf_source_preserved, overleaf_chooser_state, overleaf_review_pair, overleaf_review_docked,
    overleaf_preview_ready, overleaf_follow_state, overleaf_follow_caret, overleaf_page_departed,
    overleaf_peer_closed,
    overleaf_session_preserved, overleaf_pending_storage_targets,
    overleaf_staged_workspace, overleaf_workspace_reconnected,
    overleaf_authentication_state, overleaf_native_browser_rect, overleaf_header_geometry,
    overleaf_edit_rejection, workspace_creation_state, workspace_creation_options_hidden,
)


class OverleafJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, connection=False, browser=False,
                     create_browser=False, pending=False, epoch=0,
                     query_fingerprint=fingerprint(INVALID_SCRIPT), error='Invalid browser address',
                     browser_url_fingerprint=None, browser_start_url_fingerprint=None,
                     storage_preparing=False, storage_keep_local_copy=True,
                     storage_directory_fingerprint=fingerprint(''), storage_controls_visible=False,
                     storage_directory_visible=False)
        state.update(changes)
        if 'storage_directory_visible' not in changes:
            state['storage_directory_visible'] = (state['visible'] and state['storage_controls_visible']
                                                  and state['storage_keep_local_copy'])
        return dict(path='/private/tmp/fixture.txt', source_fingerprint=fingerprint('original'),
                    source_bytes=8, revision='1', dirty=False, tabs=1,
                    active_tab_id=4, preview_tab_id=4, preview_path='/private/tmp/fixture.txt',
                    overleaf=state)

    def workflow(self, poison_phase=None, poison=lambda document: None, *, initial_tabs=1):
        """Run the real orchestrator; only native phase actions are substituted."""
        original = self.document(visible=False, error=None, online=False, sync_ready=False,
                                 source_locked=False, keep_local_copy=False)
        original['focused'] = True
        original['workspace_root_fingerprint'] = fingerprint('/private/tmp')
        original['tab_order'] = list(range(original['active_tab_id'], original['active_tab_id'] + initial_tabs))
        original['tabs'] = initial_tabs
        case = self

        class Workflow:
            def __init__(self):
                self.directory = Path('/private/tmp')
                self.document, self.events, self.phases, self.boundaries = deepcopy(original), [], [], []

            def snapshot(self):
                return {'document': deepcopy(self.document)}

            def record(self, action, **details):
                self.events.append((action, details))

            def wait(self, description, predicate):
                observed = deepcopy(self.document)
                case.assertTrue(predicate(observed), description)
                self.boundaries.append(observed)
                return {'document': observed}

            def phase(self, label, retained_browser):
                self.phases.append((label, id(self)))
                # Browser-only changes retain owners. Actual workspace changes
                # retire old pages and reopen the saved local file with new IDs.
                self.document['revision'] = label + '-completed'
                if label != 'browser':
                    tab = max(self.document['tab_order']) + 1
                    self.document.update(active_tab_id=tab, preview_tab_id=tab, tab_order=[tab], tabs=1)
                self.document['overleaf'].update(browser=retained_browser, connection=label == 'storage',
                                                keep_local_copy=label == 'storage')
                if poison_phase == label:
                    poison(self.document)

            def overleaf_connection(self):
                self.phase('browser', True)

            def overleaf_files(self):
                self.phase('files', False)

            def overleaf_storage(self):
                self.phase('storage', False)

            def native(self, *args):
                case.fail('an orchestration test attempted native input')

        return Workflow(), original

    def test_workspace_return_does_not_retain_the_departing_aggregate_tab_count(self):
        workflow, before = self.workflow(initial_tabs=3)
        OverleafJourneys.overleaf(workflow)
        self.assertEqual([d['tabs'] for d in workflow.boundaries], [3, 1, 1])
        self.assertEqual(workflow.boundaries[0]['tab_order'], before['tab_order'])
        self.assertFalse(set(workflow.boundaries[1]['tab_order']).intersection(before['tab_order']))

    def test_browser_phase_cannot_replace_inactive_aggregate_owners(self):
        workflow, _ = self.workflow('browser',
                                    lambda d: d.update(tab_order=[d['active_tab_id'], 40, 41]),
                                    initial_tabs=3)
        with self.assertRaises(AssertionError):
            OverleafJourneys.overleaf(workflow)

    def test_combined_workflow_reopens_saved_local_files_and_retires_old_pages(self):
        workflow, before = self.workflow()
        OverleafJourneys.overleaf(workflow)
        self.assertEqual(workflow.phases, [(phase, id(workflow)) for phase in ('browser', 'files', 'storage')])
        self.assertEqual([d['overleaf']['browser'] for d in workflow.boundaries], [True, False, False])
        self.assertEqual([d['overleaf']['connection'] for d in workflow.boundaries], [False, False, True])
        self.assertEqual([d['overleaf']['keep_local_copy'] for d in workflow.boundaries], [False, False, True])
        previous = before
        for index, document in enumerate(workflow.boundaries):
            for field in OWNER_FIELDS:
                if field not in ('revision', 'active_tab_id', 'preview_tab_id'):
                    self.assertEqual(document[field], before[field], field)
            if index == 0:
                self.assertEqual(document['tab_order'], before['tab_order'])
            else:
                self.assertFalse(set(document['tab_order']).intersection(previous['tab_order']))
                self.assertEqual(document['active_tab_id'], document['preview_tab_id'])
                self.assertEqual(document['tab_order'], [document['active_tab_id']])
            self.assertNotEqual(document['revision'], before['revision'])
            self.assertTrue(document['focused'])
            for flag in ('online', 'pending', 'sync_ready', 'source_locked', 'visible'):
                self.assertIs(document['overleaf'][flag], False, flag)
            previous = document
        self.assertEqual([details['phase'] for action, details in workflow.events
                          if action == 'overleaf.phase.passed'], ['browser', 'files', 'storage'])

    def test_any_phase_leak_stops_the_workflow_before_the_next_phase(self):
        phases = ('browser', 'files', 'storage')
        owner_changes = dict(path='/different.typ', source_fingerprint=fingerprint('changed'),
                             source_bytes=9, dirty=True, tabs=2, active_tab_id=8,
                             preview_tab_id=8, preview_path='/different.pdf', tab_order=[8],
                             workspace_root_fingerprint=fingerprint('/different'))
        for index, phase in enumerate(phases):
            poisons = [(field, lambda d, field=field, value=value: d.update({field: value}))
                       for field, value in owner_changes.items()]
            poisons += [('unfocused', lambda d: d.update(focused=False)),
                        ('wrong retained project', lambda d, phase=phase:
                         d['overleaf'].update(connection=phase != 'storage')),
                        ('online', lambda d: d['overleaf'].update(online=True)),
                        ('pending', lambda d: d['overleaf'].update(pending=True)),
                        ('sync ready', lambda d: d['overleaf'].update(sync_ready=True)),
                        ('locked original source', lambda d: d['overleaf'].update(source_locked=True)),
                        ('visible manager', lambda d: d['overleaf'].update(visible=True)),
                        ('wrong browser lifetime', lambda d, phase=phase:
                         d['overleaf'].update(browser=phase != 'browser'))]
            if phase == 'storage':
                poisons += [('temporary offline model', lambda d: d['overleaf'].update(keep_local_copy=False))]
            for leak, poison in poisons:
                with self.subTest(phase=phase, leak=leak):
                    workflow, _ = self.workflow(phase, poison)
                    with self.assertRaises(AssertionError):
                        OverleafJourneys.overleaf(workflow)
                    self.assertEqual(workflow.phases, [(label, id(workflow)) for label in phases[:index + 1]])
                    self.assertEqual([details['phase'] for action, details in workflow.events
                                      if action == 'overleaf.phase.passed'], list(phases[:index]))

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

    def test_following_requires_explicit_peer_head_and_presence_observations(self):
        document = self.document(connection=True, browser=True, following_peer='fixture-peer',
                                 seeking_peer=None, following_head=12, collaborator_count=1, cursor_count=1)
        self.assertEqual(overleaf_follow_state(document)['following_head'], 12)
        stopped = deepcopy(document)
        stopped['overleaf'].update(following_peer=None, following_head=None)
        self.assertIsNone(overleaf_follow_state(stopped)['following_peer'])
        for field in ('following_peer', 'seeking_peer', 'following_head', 'collaborator_count', 'cursor_count'):
            missing = deepcopy(document)
            del missing['overleaf'][field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_follow_state(missing)
        for field, value in (('following_peer', ''), ('following_peer', True), ('following_peer', 'x' * 129),
                             ('seeking_peer', 0), ('following_head', -1), ('following_head', True),
                             ('collaborator_count', -1), ('cursor_count', False)):
            changed = deepcopy(document)
            changed['overleaf'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                overleaf_follow_state(changed)
        stopped['overleaf']['following_head'] = 12
        with self.assertRaises(AssertionError):
            overleaf_follow_state(stopped)

    def peer_close_documents(self):
        from desktop_ui_journeys import PREVIEW_SOURCE
        from overleaf_storage_fixture import SOURCE, SECOND_SOURCE
        main = self.document(visible=False, connection=True, browser=True, following_peer=None,
                             seeking_peer=None, following_head=None, collaborator_count=1, cursor_count=0)
        main.update(path='/private/tmp/project/main.txt', source_fingerprint=fingerprint(SOURCE),
                    source_bytes=len(SOURCE.encode()), dirty=False, cursor=[6, 6], tabs=2,
                    tab_order=[7, 8], active_tab_id=7, preview_tab_id=8,
                    preview_path='/private/tmp/project/preview.typ',
                    workspace_root_fingerprint=fingerprint('/private/tmp/project'))
        peer = deepcopy(main)
        peer.update(path='/private/tmp/project/other.txt', source_fingerprint=fingerprint(SECOND_SOURCE),
                    source_bytes=len(SECOND_SOURCE.encode()), tabs=3, tab_order=[7, 8, 9], active_tab_id=9)
        preview = deepcopy(main)
        preview.update(path=main['preview_path'], source_fingerprint=fingerprint(PREVIEW_SOURCE),
                       source_bytes=len(PREVIEW_SOURCE.encode()), active_tab_id=8)
        return main, peer, preview

    def test_peer_close_allows_adjacent_preview_or_main_and_requires_exact_retired_owner(self):
        main, peer, preview = self.peer_close_documents()
        self.assertTrue(overleaf_peer_closed(preview, peer, main))
        self.assertTrue(overleaf_peer_closed(main, peer, main))
        for fields in (dict(tab_order=[7, 9]), dict(tab_order=[8, 9]), dict(tab_order=[7, 10]),
                       dict(tab_order=[8, 7]), dict(active_tab_id=7), dict(preview_tab_id=7),
                       dict(preview_path='/different.typ'), dict(path=main['path']),
                       dict(workspace_root_fingerprint=fingerprint('/different')),
                       dict(source_fingerprint=fingerprint('changed preview')), dict(source_bytes=0),
                       dict(dirty=True)):
            changed = deepcopy(preview)
            changed.update(fields)
            with self.subTest(fields=fields):
                try:
                    accepted = overleaf_peer_closed(changed, peer, main)
                except AssertionError:
                    accepted = False
                self.assertFalse(accepted)
        following = deepcopy(preview)
        following['overleaf'].update(following_peer='fixture-peer', following_head=3)
        self.assertFalse(overleaf_peer_closed(following, peer, main))

    def test_peer_close_rejects_missing_or_malformed_tab_and_workspace_ownership(self):
        main, peer, preview = self.peer_close_documents()
        for index in range(3):
            for field in ('tab_order', 'tabs', 'active_tab_id', 'preview_tab_id', 'preview_path',
                          'workspace_root_fingerprint', 'source_fingerprint', 'source_bytes'):
                documents = deepcopy([preview, peer, main])
                del documents[index][field]
                with self.subTest(document=index, missing=field), self.assertRaises(AssertionError):
                    overleaf_peer_closed(*documents)
            for field, value in (('tab_order', [7, 7]), ('active_tab_id', True),
                                 ('preview_tab_id', True), ('preview_path', None),
                                 ('workspace_root_fingerprint', 'raw path'), ('tabs', True)):
                documents = deepcopy([preview, peer, main])
                documents[index][field] = value
                with self.subTest(document=index, malformed=field), self.assertRaises(AssertionError):
                    overleaf_peer_closed(*documents)

    def test_follow_caret_requires_current_visible_root_geometry_not_stale_or_child_view(self):
        document = self.document(connection=True, browser=True, following_peer='fixture-peer',
                                 seeking_peer=None, following_head=12, collaborator_count=1, cursor_count=1)
        document.update(viewport='root', frame=42)
        target = dict(x=100, y=200, width=2, height=20, enabled=True,
                      viewport='root', frame=42, age_ms=5)
        snapshot = dict(document=document, targets={'overleaf.cursor.fixture-peer': target})
        self.assertTrue(overleaf_follow_caret(snapshot))
        for field, value in (('viewport', 'child'), ('frame', 41), ('age_ms', 501), ('enabled', False),
                             ('width', 0), ('height', -1), ('x', float('nan')), ('y', None)):
            changed = deepcopy(snapshot)
            changed['targets']['overleaf.cursor.fixture-peer'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                overleaf_follow_caret(changed)
        missing = deepcopy(snapshot)
        missing['targets'] = {}
        with self.assertRaises(AssertionError):
            overleaf_follow_caret(missing)
        stopped = deepcopy(snapshot)
        stopped['document']['overleaf'].update(following_peer=None, following_head=None)
        with self.assertRaises(AssertionError):
            overleaf_follow_caret(stopped)

    def test_native_follow_wait_rejects_early_state_and_stale_caret_until_a_post_scroll_frame(self):
        from unittest.mock import patch
        document = self.document(connection=True, browser=True, following_peer='fixture-peer',
                                 seeking_peer=None, following_head=12, collaborator_count=1, cursor_count=1)
        document.update(viewport='root', frame=42)
        target = dict(x=100, y=200, width=2, height=20, enabled=True,
                      viewport='root', frame=42, age_ms=5)
        visible = dict(document=document, targets={'overleaf.cursor.fixture-peer': target})
        waiting = deepcopy(visible)
        waiting['document']['overleaf'].update(following_peer=None, following_head=None)
        stale = deepcopy(visible)
        stale['targets']['overleaf.cursor.fixture-peer']['frame'] = 41
        class Fake(OverleafJourneys):
            def __init__(self, snapshots):
                self.snapshots = snapshots
                self.observed = 0
            def snapshot(self):
                self.observed += 1
                return self.snapshots[min(self.observed - 1, len(self.snapshots) - 1)]
        ready = lambda d: overleaf_follow_state(d)['following_head'] == 12
        journey = Fake([waiting, stale, visible])
        with patch('time.sleep'):
            self.assertIs(journey.overleaf_wait_follow_visible('follow', ready), visible)
        self.assertEqual(journey.observed, 3)
        journey = Fake([stale])
        with patch('time.sleep'), patch('time.monotonic', side_effect=[0, .01, .1]):
            with self.assertRaisesRegex(AssertionError, 'timed out: follow') as failed:
                journey.overleaf_wait_follow_visible('follow', ready, timeout=.05)
        self.assertIn('not freshly visible', str(failed.exception.__cause__))

    def test_missing_or_malformed_inspection_cannot_pass_as_disconnected(self):
        invalid = ({}, {'overleaf': None}, {'overleaf': {}}, self.document(visible=1),
                   self.document(epoch=True), self.document(epoch=-1),
                   self.document(query_fingerprint='bad'), self.document(error=''),
                   self.document(error=True), self.document(pending=True),
                   self.document(browser_url_fingerprint='not a hash'),
                   self.document(browser_start_url_fingerprint=True),
                   self.document(storage_controls_visible=1),
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
        # Browsing is valid before authentication and does not imply a project.
        browsing = self.document(visible=False, browser=True, browser_url_fingerprint=fingerprint(LOGIN_URL))
        self.assertTrue(overleaf_state(browsing)['browser'])
        creating = self.document(create_browser=True)
        self.assertTrue(overleaf_state(creating)['create_browser'])
        connected = self.document(visible=False, connection=True, browser=True, epoch=2)
        self.assertFalse(overleaf_browsing(connected, self.document(), visible=False))

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

    def test_browsing_proof_requires_a_real_browser_exact_address_and_unchanged_source(self):
        before = self.document()
        browsing = self.document(browser=True, browser_url_fingerprint=fingerprint(LOGIN_URL),
                                 browser_start_url_fingerprint=fingerprint(LOGIN_URL))
        self.assertTrue(overleaf_browsing(browsing, before, visible=True, query=INVALID_SCRIPT, url=LOGIN_URL, error=True))
        self.assertTrue(overleaf_browser_at(browsing, LOGIN_URL))
        self.assertFalse(overleaf_browsing(browsing, before, visible=False, url=LOGIN_URL))
        self.assertFalse(overleaf_browsing(browsing, before, visible=True, query='different'))
        self.assertFalse(overleaf_browsing(browsing, before, visible=True, url='https://other.example/'))
        for changes in ({'connection': True}, {'connection': True, 'pending': True},
                        {'create_browser': True}, {'epoch': 1}, {'storage_preparing': True},
                        {'browser': False}, {'browser_url_fingerprint': None}, {'error': None}):
            changed = deepcopy(browsing)
            changed['overleaf'].update(changes)
            with self.subTest(changes=changes):
                self.assertFalse(overleaf_browsing(changed, before, visible=True,
                                                   query=INVALID_SCRIPT, url=LOGIN_URL, error=True))
        for field in OWNER_FIELDS:
            changed = deepcopy(browsing)
            changed[field] = 'changed'
            with self.subTest(owner=field):
                self.assertFalse(overleaf_browsing(changed, before, visible=True, url=LOGIN_URL))
        for field in ('browser_url_fingerprint', 'browser_start_url_fingerprint', 'storage_controls_visible'):
            missing = deepcopy(browsing)
            del missing['overleaf'][field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_browsing(missing, before, visible=True)

    def test_address_observation_requires_a_constructed_browser_and_never_uses_requested_url_as_current(self):
        document = self.document(browser=True, browser_url_fingerprint=fingerprint(LOGIN_URL),
                                 browser_start_url_fingerprint=fingerprint('https://requested.example/'))
        self.assertTrue(overleaf_browser_at(document, LOGIN_URL))
        self.assertFalse(overleaf_browser_at(document, 'https://requested.example/'))
        document['overleaf']['browser'] = False
        self.assertFalse(overleaf_browser_at(document, LOGIN_URL))
        document['overleaf'].update(browser=True, create_browser=True)
        self.assertFalse(overleaf_browser_at(document, LOGIN_URL))

    def test_storage_proof_requires_exact_folder_and_mode_observations(self):
        document = self.document(storage_directory_fingerprint=fingerprint('/private/copies'))
        self.assertTrue(overleaf_storage(document, keep=True, directory='/private/copies'))
        self.assertFalse(overleaf_storage(document, keep=False, directory='/private/copies'))
        self.assertFalse(overleaf_storage(document, keep=True, directory='/wrong'))
        for field in ('storage_preparing', 'storage_keep_local_copy', 'storage_controls_visible',
                      'storage_directory_visible', 'storage_directory_fingerprint'):
            missing = self.document()
            del missing['overleaf'][field]
            with self.subTest(field=field), self.assertRaises(AssertionError):
                overleaf_state(missing)

    def test_projectless_workspace_storage_requires_visible_folder_and_no_connection_or_root_change(self):
        before = self.document()
        before['workspace_root_fingerprint'] = fingerprint('/private/tmp/local')
        staged = self.document(query_fingerprint=fingerprint(LOGIN_URL), browser=True,
                               browser_url_fingerprint=fingerprint(LOGIN_URL), error=None,
                               storage_controls_visible=True,
                               storage_directory_fingerprint=fingerprint('/private/tmp/new-workspace'))
        staged['workspace_root_fingerprint'] = before['workspace_root_fingerprint']
        options = dict(query=LOGIN_URL, directory='/private/tmp/new-workspace', controls=True)
        self.assertTrue(overleaf_projectless_storage(staged, before, **options))
        project = deepcopy(staged)
        project['overleaf']['query_fingerprint'] = fingerprint('https://overleaf.invalid/project/' + 'a' * 24)
        self.assertTrue(overleaf_projectless_storage(
            project, before, **{**options, 'query': 'https://overleaf.invalid/project/' + 'a' * 24}))
        for field, value in (('connection', True), ('epoch', 1), ('storage_preparing', True),
                             ('storage_directory_fingerprint', fingerprint('/wrong-folder')),
                             ('storage_keep_local_copy', False), ('query_fingerprint', fingerprint('other')),
                             ('error', 'unexpected error')):
            wrong = deepcopy(staged)
            wrong['overleaf'][field] = value
            if field == 'storage_keep_local_copy':
                wrong['overleaf']['storage_directory_visible'] = False
            with self.subTest(field=field):
                self.assertFalse(overleaf_projectless_storage(wrong, before, **options))
        for field, value in (('workspace_root_fingerprint', fingerprint('/other-root')),
                             ('source_fingerprint', fingerprint('changed source'))):
            wrong = deepcopy(staged)
            wrong[field] = value
            with self.subTest(owner=field):
                self.assertFalse(overleaf_projectless_storage(wrong, before, **options))
        reset = deepcopy(staged)
        reset['overleaf'].update(storage_controls_visible=False, storage_directory_visible=False,
                                storage_directory_fingerprint=fingerprint(''))
        self.assertTrue(overleaf_projectless_storage(reset, before, query=LOGIN_URL, directory='', controls=False))
        self.assertFalse(overleaf_projectless_storage(reset, before, **options))
        missing_root = deepcopy(staged)
        del missing_root['workspace_root_fingerprint']
        with self.assertRaisesRegex(AssertionError, 'workspace inspection'):
            overleaf_projectless_storage(missing_root, before, **options)

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

    def test_storage_draft_survives_address_blur_and_rejects_retired_controls_under_500ms(self):
        query = 'http://127.0.0.1:4321/project/' + 'a' * 24
        before = self.document()
        document = self.document(query_fingerprint=fingerprint(query),
                                 storage_controls_visible=True, storage_keep_local_copy=True)
        target = dict(enabled=True, frame=82, age_ms=169, viewport='overleaf-child')
        snapshot = dict(document=document, targets={name: dict(target) for name in
                        ('overleaf.url', 'overleaf.keep_local_copy',
                         'overleaf.directory', 'overleaf.choose_folder')})
        self.assertTrue(overleaf_storage_draft_ready(snapshot, before, query=query, keep=True))
        # A completed login navigation used to overwrite an explicit project
        # draft when its address blurred for the local-copy checkbox. Inspector
        # targets from the vanished storage row can still be less than 500ms old.
        retired = deepcopy(snapshot)
        retired['document']['overleaf'].update(query_fingerprint=fingerprint(LOGIN_URL),
                                             storage_controls_visible=False,
                                             storage_directory_visible=False)
        retired['targets']['overleaf.url']['frame'] = 85
        self.assertTrue(overleaf_storage_targets(retired, keep=True))
        self.assertFalse(overleaf_storage_draft_ready(retired, before, query=query, keep=True))
        for field, value in (('frame', 85), ('viewport', 'another-child'),
                             ('enabled', False), ('age_ms', 501)):
            wrong = deepcopy(snapshot)
            wrong['targets']['overleaf.url'][field] = value
            with self.subTest(address=field):
                self.assertFalse(overleaf_storage_draft_ready(wrong, before, query=query, keep=True))
        changed_source = deepcopy(snapshot)
        changed_source['document']['source_fingerprint'] = fingerprint('changed source')
        self.assertFalse(overleaf_storage_draft_ready(changed_source, before, query=query, keep=True))
        self.assertFalse(overleaf_storage_draft_ready(snapshot, before, query=LOGIN_URL, keep=True))
        self.assertFalse(overleaf_storage_draft_ready(snapshot, before, query=query, keep=False))

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
                self.document = case.document(browser=True, browser_url_fingerprint=fingerprint(LOGIN_URL))
                self.change_owner, self.keys = change_owner, []
            def wait_target(self, name, focused=False):
                case.assertEqual((name, focused), ('overleaf.url', True))
            def key(self, code, flags='cmd'):
                self.keys.append((code, flags))
                self.document['overleaf']['query_fingerprint'] = fingerprint(LOGIN_URL if flags == 'cmd' else INVALID_SCRIPT)
                if self.change_owner:
                    self.document['source_fingerprint'] = fingerprint('wrong editor changed')
            def wait(self, description, predicate):
                if not predicate(self.document):
                    raise AssertionError(description)
        journey = Undo()
        journey.overleaf_undo_initial_query(journey.before, INVALID_SCRIPT)
        self.assertEqual(journey.keys, [(6, 'cmd'), (6, 'cmd+shift')])
        changed = Undo(change_owner=True)
        with self.assertRaises(AssertionError):
            changed.overleaf_undo_initial_query(changed.before, INVALID_SCRIPT)

    def connected_document(self):
        document = self.document(visible=False, connection=True, browser=True, epoch=2, error=None,
                                 sync_ready=True, source_locked=False, blocked_count=0,
                                 review=dict(open=False, loading=False, left=None, right=None, error=None))
        document.update(cursor=[8, 4], dirty=True, workspace_chooser_visible=False,
                        workspace_root_fingerprint=fingerprint('/private/tmp/remote'))
        return document

    def staged_workspace_observation(self):
        query = 'http://127.0.0.1:43000/project/aaaaaaaaaaaaaaaaaaaaaaaa'
        before = self.connected_document()
        before.update(path='/private/tmp/remote/main.txt', modal_open=False,
                      focused=True, editor_keyboard_focused=True,
                      workspace_creation=dict(visible=False, busy=False, use_overleaf=False,
                                              name_fingerprint=None, parent_fingerprint=None, error=None))
        before['overleaf'].update(online=True, keep_local_copy=True,
                                  profile_fingerprint=fingerprint('original-profile'),
                                  query_fingerprint=fingerprint(query),
                                  browser_url_fingerprint=fingerprint(query),
                                  browser_start_url_fingerprint=fingerprint(query),
                                  storage_controls_visible=True,
                                  storage_directory_fingerprint=fingerprint('/private/tmp/current'))
        staged = deepcopy(before)
        staged['overleaf'].update(visible=True, online=False, sync_ready=False, source_locked=True,
                                  profile_fingerprint=fingerprint('staged-profile'),
                                  browser_url_fingerprint=fingerprint(LOGIN_URL),
                                  browser_start_url_fingerprint=fingerprint(LOGIN_URL),
                                  storage_controls_visible=True, storage_directory_visible=True,
                                  storage_directory_fingerprint=fingerprint('/private/tmp/new-workspace'))
        options = dict(query=query, directory_fingerprint=fingerprint('/private/tmp/new-workspace'))
        return before, staged, options

    def test_new_workspace_login_isolates_profile_without_replacing_the_old_source_or_epoch(self):
        before, staged, options = self.staged_workspace_observation()
        self.assertTrue(overleaf_staged_workspace(staged, before, **options))
        profile = staged['overleaf']['profile_fingerprint']
        self.assertTrue(overleaf_staged_workspace(staged, before, profile=profile, **options))
        for field, value in [('profile_fingerprint', before['overleaf']['profile_fingerprint']),
                             ('epoch', before['overleaf']['epoch'] + 1), ('online', True),
                             ('sync_ready', True), ('source_locked', False), ('pending', True),
                             ('browser_url_fingerprint', before['overleaf']['browser_url_fingerprint']),
                             ('browser_start_url_fingerprint', before['overleaf']['browser_start_url_fingerprint']),
                             ('storage_controls_visible', False),
                             ('storage_directory_fingerprint', fingerprint('/wrong-folder'))]:
            wrong = deepcopy(staged)
            wrong['overleaf'][field] = value
            if field == 'storage_controls_visible':
                wrong['overleaf']['storage_directory_visible'] = False
            with self.subTest(state=field):
                self.assertFalse(overleaf_staged_workspace(wrong, before, **options))
        wrong = deepcopy(staged)
        wrong['overleaf']['storage_directory_visible'] = False
        with self.assertRaises(AssertionError):
            overleaf_staged_workspace(wrong, before, **options)
        for field, value in [('workspace_root_fingerprint', fingerprint('/replacement')),
                             ('path', '/replacement/main.txt'), ('source_fingerprint', fingerprint('lost draft')),
                             ('cursor', [0, 0]), ('active_tab_id', 88), ('modal_open', True)]:
            wrong = deepcopy(staged)
            wrong[field] = value
            with self.subTest(owner=field):
                self.assertFalse(overleaf_staged_workspace(wrong, before, **options))
        self.assertFalse(overleaf_staged_workspace(staged, before, profile=fingerprint('another-staged-profile'), **options))
        for value in (None, '', 'profile-id'):
            wrong = deepcopy(staged)
            wrong['overleaf']['profile_fingerprint'] = value
            with self.subTest(missing_profile=value), self.assertRaises(AssertionError):
                overleaf_staged_workspace(wrong, before, **options)

    def test_reset_online_account_requires_original_profile_exact_epoch_and_saved_storage(self):
        before, staged, _ = self.staged_workspace_observation()
        restored = deepcopy(before)
        restored['overleaf'].update(epoch=before['overleaf']['epoch'] + 1)
        self.assertTrue(overleaf_workspace_reconnected(restored, before))
        self.assertFalse(overleaf_workspace_reconnected(staged, before))
        for field, value in [('profile_fingerprint', staged['overleaf']['profile_fingerprint']),
                             ('epoch', before['overleaf']['epoch']), ('epoch', before['overleaf']['epoch'] + 2),
                             ('online', False), ('sync_ready', False), ('source_locked', True),
                             ('pending', True), ('storage_directory_fingerprint', fingerprint('/staged'))]:
            wrong = deepcopy(restored)
            wrong['overleaf'][field] = value
            with self.subTest(state=field, value=value):
                self.assertFalse(overleaf_workspace_reconnected(wrong, before))
        for field, value in [('workspace_root_fingerprint', fingerprint('/wrong-root')),
                             ('source_fingerprint', fingerprint('replacement')), ('cursor', [0, 0]),
                             ('active_tab_id', 88), ('modal_open', True), ('focused', False),
                             ('editor_keyboard_focused', False)]:
            wrong = deepcopy(restored)
            wrong[field] = value
            with self.subTest(owner=field):
                self.assertFalse(overleaf_workspace_reconnected(wrong, before))
        wrong = deepcopy(restored)
        wrong['overleaf'].update(visible=True, storage_directory_visible=True)
        self.assertFalse(overleaf_workspace_reconnected(wrong, before))

    def test_storage_draft_cannot_replace_session_workspace_source_selection_or_tabs(self):
        before = self.connected_document()
        before['overleaf']['keep_local_copy'] = True
        changed = deepcopy(before)
        changed['overleaf'].update(storage_directory_fingerprint=fingerprint('/private/tmp/pending'))
        self.assertTrue(overleaf_session_preserved(changed, before))
        self.assertFalse(overleaf_session_preserved(changed, before, reconnected=True))
        changed['overleaf']['epoch'] += 1
        self.assertTrue(overleaf_session_preserved(changed, before, reconnected=True))
        self.assertFalse(overleaf_session_preserved(changed, before))
        poisons = [('workspace_root_fingerprint', fingerprint('/private/tmp/replacement')),
                   ('path', '/private/tmp/replacement/main.txt'),
                   ('source_fingerprint', fingerprint('lost draft')), ('source_bytes', 0),
                   ('dirty', False), ('cursor', [0, 0]), ('tabs', 8), ('active_tab_id', 42),
                   ('preview_tab_id', 42), ('preview_path', '/private/tmp/replacement/preview.typ')]
        for field, value in poisons:
            broken = deepcopy(changed)
            broken[field] = value
            with self.subTest(owner=field):
                self.assertFalse(overleaf_session_preserved(broken, before, reconnected=True))
        for field, value in (('connection', False), ('browser', False), ('keep_local_copy', False),
                             ('pending', True), ('storage_preparing', True), ('epoch', 1)):
            broken = deepcopy(changed)
            broken['overleaf'][field] = value
            with self.subTest(session=field):
                self.assertFalse(overleaf_session_preserved(broken, before, reconnected=True))
        for field in ('workspace_root_fingerprint', 'keep_local_copy'):
            broken = deepcopy(changed)
            (broken if field == 'workspace_root_fingerprint' else broken['overleaf']).pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                overleaf_session_preserved(broken, before, reconnected=True)

    def test_read_only_feedback_requires_actual_painted_visibility_lock_and_keyboard_owner(self):
        before = self.connected_document()
        before.update(focused=True, editor_keyboard_focused=True)
        for visible in (False, True):
            current = deepcopy(before)
            current['overleaf']['source_locked'] = True
            current['read_only_rejection'] = dict(visible=visible)
            with self.subTest(visible=visible):
                self.assertTrue(overleaf_edit_rejection(current, before, visible=visible))
                self.assertFalse(overleaf_edit_rejection(current, before, visible=not visible))
                for field in ('focused', 'editor_keyboard_focused'):
                    unfocused = deepcopy(current)
                    unfocused[field] = False
                    self.assertFalse(overleaf_edit_rejection(unfocused, before, visible=visible), field)
                unlocked = deepcopy(current)
                unlocked['overleaf']['source_locked'] = False
                self.assertFalse(overleaf_edit_rejection(unlocked, before, visible=visible))
                for field, value in (('path', '/different.txt'), ('source_fingerprint', fingerprint('mutated')),
                                     ('source_bytes', 0), ('cursor', [0, 0]), ('dirty', False), ('tabs', 2),
                                     ('active_tab_id', 42), ('preview_tab_id', 42), ('preview_path', '/different.typ')):
                    changed = deepcopy(current)
                    changed[field] = value
                    self.assertFalse(overleaf_edit_rejection(changed, before, visible=visible), field)

    def test_missing_or_malformed_rejection_observation_cannot_pass_as_hidden(self):
        before = self.connected_document()
        for feedback in (None, False, {}, dict(visible=None), dict(visible=0), dict(visible='false')):
            current = deepcopy(before)
            current['read_only_rejection'] = feedback
            with self.subTest(feedback=feedback), self.assertRaisesRegex(AssertionError, 'Painted read-only'):
                overleaf_edit_rejection(current, before, visible=False)
        with self.assertRaisesRegex(AssertionError, 'Painted read-only'):
            overleaf_edit_rejection(before, before, visible=False)

    def test_authentication_requires_each_real_lock_hydration_and_ready_transition(self):
        before = self.connected_document()
        before['overleaf'].update(online=True, needs_signin=False, keep_local_copy=True)
        urls = dict(signin='http://127.0.0.1:1234/login',
                    hydrating='http://127.0.0.1:1234/project/' + 'a' * 24,
                    ready='http://127.0.0.1:1234/project/' + 'a' * 24)
        phases = dict(signin=dict(needs_signin=True, online=False, sync_ready=False, source_locked=True),
                      hydrating=dict(needs_signin=False, online=True, sync_ready=False, source_locked=True),
                      ready=dict(needs_signin=False, online=True, sync_ready=True, source_locked=False))
        # Initial reconnect replaces the browser once. A real sign-in then
        # replaces it again to reopen the bound project; cookie reuse does not.
        transitions = (('signin', 1), ('hydrating', 2), ('ready', 2),
                       ('hydrating', 1), ('ready', 1))
        for phase, epoch_delta in transitions:
            flags = phases[phase]
            expected_epoch = before['overleaf']['epoch'] + epoch_delta
            current = deepcopy(before)
            current['overleaf'].update(flags, epoch=expected_epoch,
                                       browser_url_fingerprint=fingerprint(urls[phase]))
            options = dict(url=urls[phase], phase=phase, epoch_delta=epoch_delta)
            with self.subTest(phase=phase, epoch_delta=epoch_delta):
                self.assertTrue(overleaf_authentication_state(current, before, **options))
                for field in flags:
                    wrong = deepcopy(current)
                    wrong['overleaf'][field] = not flags[field]
                    self.assertFalse(overleaf_authentication_state(wrong, before, **options), field)
                for epoch in (before['overleaf']['epoch'] - 1, before['overleaf']['epoch'],
                              expected_epoch - 1, expected_epoch + 1, expected_epoch + 100):
                    wrong = deepcopy(current)
                    wrong['overleaf']['epoch'] = epoch
                    with self.subTest(wrong_epoch=epoch):
                        self.assertFalse(overleaf_authentication_state(wrong, before, **options))
                other_delta = 2 if epoch_delta == 1 else 1
                self.assertFalse(overleaf_authentication_state(
                    current, before, **{**options, 'epoch_delta': other_delta}))
                for field, value in (('browser_url_fingerprint', fingerprint(urls[phase] + '/other')),
                                     ('keep_local_copy', False), ('pending', True), ('browser', False)):
                    wrong = deepcopy(current)
                    wrong['overleaf'][field] = value
                    self.assertFalse(overleaf_authentication_state(wrong, before, **options), field)
                for field, value in (('workspace_root_fingerprint', fingerprint('/other')),
                                     ('source_fingerprint', fingerprint('lost draft')), ('dirty', False),
                                     ('cursor', [8, 8]), ('active_tab_id', 9), ('preview_tab_id', 9)):
                    wrong = deepcopy(current)
                    wrong[field] = value
                    self.assertFalse(overleaf_authentication_state(wrong, before, **options), field)
                for field in ('needs_signin', 'online'):
                    for value in (None, 0, 'false'):
                        malformed = deepcopy(current)
                        malformed['overleaf'][field] = value
                        with self.subTest(malformed=field, value=value), self.assertRaisesRegex(
                                AssertionError, 'authentication observation'):
                            overleaf_authentication_state(malformed, before, **options)
                for invalid_delta in (None, True, False, 0, -1, 3, 1.0, 2.0, '1'):
                    with self.subTest(invalid_epoch_delta=invalid_delta), self.assertRaisesRegex(
                            AssertionError, 'epoch'):
                        overleaf_authentication_state(
                            current, before, **{**options, 'epoch_delta': invalid_delta})
                unready = deepcopy(before)
                unready['overleaf']['sync_ready'] = False
                self.assertFalse(overleaf_authentication_state(current, unready, **options))
                if phase == 'ready':
                    poisoned = deepcopy(current)
                    poisoned['overleaf']['error'] = 'Invalid native frame message'
                    self.assertFalse(overleaf_authentication_state(poisoned, before, **options))

    def workspace_header_snapshot(self, scale=1):
        document = self.connected_document()
        document.update(viewport='root', frame=42, interface_scale=scale)
        common = dict(enabled=True, frame=42, age_ms=0, viewport='root',
                      y=40 * scale, height=20 * scale)
        return dict(document=document, targets={
            'explorer.workspace': dict(common, x=128 * scale, width=240 * scale),
            'explorer.overleaf_status': dict(common, x=258 * scale, width=12 * scale)})

    def test_header_column_stays_thin_fixed_and_separate_at_interface_scales_and_name_changes(self):
        for scale in (1, 1.25, 2):
            with self.subTest(scale=scale):
                snapshot = self.workspace_header_snapshot(scale)
                expected = tuple(value * scale for value in (258, 40, 12, 20))
                reference = overleaf_header_geometry(snapshot, present=True)
                self.assertEqual(reference, expected)
                # A shorter name may use less text space, but status changes
                # cannot move or resize the reserved column at the right.
                snapshot['targets']['explorer.workspace'].update(x=118 * scale, width=220 * scale)
                self.assertEqual(overleaf_header_geometry(snapshot, present=True, reference=reference),
                                 reference)
                # Real inspection serializes an opaque egui viewport ID. The
                # document owner identifies it; a literal "root" is not required.
                snapshot['document']['viewport'] = 'ViewportId(0x1234)'
                for target in snapshot['targets'].values():
                    target['viewport'] = snapshot['document']['viewport']
                self.assertEqual(overleaf_header_geometry(snapshot, present=True, reference=reference),
                                 reference)
                for index in (0, 2):
                    moved = list(reference)
                    moved[index] += 4 * scale
                    with self.subTest(changed_reference_field=index), self.assertRaises(AssertionError):
                        overleaf_header_geometry(snapshot, present=True, reference=tuple(moved))

    def test_header_geometry_rejects_missing_stale_wrong_viewport_or_malformed_required_observations(self):
        snapshot = self.workspace_header_snapshot()
        for name in ('explorer.workspace', 'explorer.overleaf_status'):
            missing = deepcopy(snapshot)
            missing['targets'].pop(name)
            with self.subTest(missing=name), self.assertRaises(AssertionError):
                overleaf_header_geometry(missing, present=True)
            for field, value in (('frame', 41), ('frame', True), ('viewport', 'manager'),
                                 ('enabled', False), ('age_ms', 501), ('age_ms', -1),
                                 ('age_ms', '0'), ('age_ms', True), ('age_ms', float('nan')),
                                 ('x', float('nan')), ('y', float('inf')), ('x', True),
                                 ('width', 0), ('height', -1), ('height', '20')):
                malformed = deepcopy(snapshot)
                malformed['targets'][name][field] = value
                with self.subTest(target=name, field=field, value=value), self.assertRaises(AssertionError):
                    overleaf_header_geometry(malformed, present=True)
            for field in ('frame', 'viewport', 'enabled', 'age_ms', 'x', 'y', 'width', 'height'):
                malformed = deepcopy(snapshot)
                malformed['targets'][name].pop(field)
                with self.subTest(target=name, missing=field), self.assertRaises(AssertionError):
                    overleaf_header_geometry(malformed, present=True)
        for field, value in (('frame', -1), ('frame', True), ('frame', 41), ('viewport', None),
                             ('viewport', 'different-owner'),
                             ('interface_scale', 0), ('interface_scale', -1), ('interface_scale', True),
                             ('interface_scale', '1'), ('interface_scale', float('nan'))):
            malformed = deepcopy(snapshot)
            malformed['document'][field] = value
            with self.subTest(document_field=field, value=value), self.assertRaises(AssertionError):
                overleaf_header_geometry(malformed, present=True)
        for field, value in (('x', 250), ('y', 43), ('width', 16), ('height', 24)):
            misaligned = deepcopy(snapshot)
            misaligned['targets']['explorer.overleaf_status'][field] = value
            with self.subTest(column=field), self.assertRaises(AssertionError):
                overleaf_header_geometry(misaligned, present=True)

    def test_local_header_rejects_a_current_status_column_but_accepts_retired_remote_targets(self):
        snapshot = self.workspace_header_snapshot()
        with self.assertRaises(AssertionError):
            overleaf_header_geometry(snapshot, present=False)
        snapshot['targets']['explorer.overleaf_status']['frame'] = 41
        self.assertIsNone(overleaf_header_geometry(snapshot, present=False))
        snapshot['targets'].pop('explorer.overleaf_status')
        self.assertIsNone(overleaf_header_geometry(snapshot, present=False))
        # Local mode still needs a freshly observed root workspace header.
        snapshot['targets']['explorer.workspace']['frame'] = 41
        with self.assertRaises(AssertionError):
            overleaf_header_geometry(snapshot, present=False)

    def test_fixture_buttons_require_visible_key_native_browser_and_finite_containing_bounds(self):
        document = self.connected_document()
        rect = [100, 80, 600, 400]
        document['overleaf'].update(visible=True, show_browser=True, browser_keyboard=dict(
            window_key=True, window_visible=True, web_screen_rect_top_left=rect))
        self.assertEqual(overleaf_native_browser_rect(document), rect)
        for field, value in (('show_browser', False), ('show_browser', None), ('visible', False)):
            malformed = deepcopy(document)
            malformed['overleaf'][field] = value
            with self.subTest(state=field, value=value), self.assertRaisesRegex(AssertionError, 'Native browser bounds'):
                overleaf_native_browser_rect(malformed)
        for field, value in (('window_visible', False), ('window_key', False),
                             ('web_screen_rect_top_left', None), ('web_screen_rect_top_left', [1, 2, 3]),
                             ('web_screen_rect_top_left', [0, 0, 220, 400]),
                             ('web_screen_rect_top_left', [0, 0, 600, 110]),
                             ('web_screen_rect_top_left', [0, float('nan'), 600, 400])):
            malformed = deepcopy(document)
            malformed['overleaf']['browser_keyboard'][field] = value
            with self.subTest(bounds=field, value=value), self.assertRaisesRegex(AssertionError, 'Native browser bounds'):
                overleaf_native_browser_rect(malformed)

    def test_pending_storage_buttons_require_a_current_pair_and_reset_removes_both(self):
        document = self.document(storage_controls_visible=True)
        checkbox = dict(enabled=True, frame=42, age_ms=0, viewport='manager')
        targets = {name: dict(checkbox) for name in ('overleaf.keep_local_copy', 'overleaf.directory',
                   'overleaf.choose_folder', 'overleaf.apply_storage', 'overleaf.reset_storage')}
        snapshot = dict(document=document, targets=targets)
        self.assertTrue(overleaf_pending_storage_targets(snapshot, pending=True))
        self.assertFalse(overleaf_pending_storage_targets(snapshot, pending=False))
        for name in ('overleaf.apply_storage', 'overleaf.reset_storage'):
            for field, value in (('enabled', False), ('frame', 41), ('frame', True),
                                 ('viewport', 'root'), ('age_ms', 501)):
                broken = deepcopy(snapshot)
                broken['targets'][name][field] = value
                with self.subTest(button=name, field=field):
                    self.assertFalse(overleaf_pending_storage_targets(broken, pending=True))
        reset = deepcopy(snapshot)
        for name in ('overleaf.apply_storage', 'overleaf.reset_storage'):
            reset['targets'][name]['frame'] = 41
        self.assertTrue(overleaf_pending_storage_targets(reset, pending=False))
        reset['targets'].pop('overleaf.apply_storage')
        reset['targets'].pop('overleaf.reset_storage')
        self.assertTrue(overleaf_pending_storage_targets(reset, pending=False))
        reset['targets'].pop('overleaf.keep_local_copy')
        self.assertFalse(overleaf_pending_storage_targets(reset, pending=False))
        hidden = deepcopy(snapshot)
        hidden['document']['overleaf'].update(visible=False, storage_directory_visible=False)
        self.assertFalse(overleaf_pending_storage_targets(hidden, pending=True))

    def test_page_departure_requires_actual_offline_lock_and_every_preserved_owner_field(self):
        before = self.connected_document()
        before['overleaf']['online'] = True
        departed = deepcopy(before)
        departed['overleaf'].update(online=False, sync_ready=False, source_locked=True)
        self.assertTrue(overleaf_page_departed(departed, before))
        for field, value in (('online', True), ('sync_ready', True), ('source_locked', False),
                             ('browser', False), ('pending', True), ('epoch', 3)):
            changed = deepcopy(departed)
            changed['overleaf'][field] = value
            with self.subTest(state=field):
                self.assertFalse(overleaf_page_departed(changed, before))
        disconnected = deepcopy(departed)
        disconnected['overleaf'].update(connection=False, browser=False)
        self.assertFalse(overleaf_page_departed(disconnected, before))
        malformed = deepcopy(departed)
        malformed['overleaf']['connection'] = False
        self.assertFalse(overleaf_page_departed(malformed, before))
        for field, value in (('source_fingerprint', fingerprint('mutated')), ('source_bytes', 10),
                             ('dirty', False), ('path', '/private/tmp/other'), ('cursor', [8, 8]),
                             ('tabs', 2), ('active_tab_id', 5), ('preview_tab_id', 5),
                             ('preview_path', '/private/tmp/other.typ')):
            changed = deepcopy(departed)
            changed[field] = value
            with self.subTest(owner=field):
                self.assertFalse(overleaf_page_departed(changed, before))
        for state in (before, departed):
            missing = deepcopy(state)
            missing['overleaf'].pop('online')
            with self.assertRaisesRegex(AssertionError, 'online observation'):
                overleaf_page_departed(missing, before) if state is departed else overleaf_page_departed(departed, missing)
        for value in (None, 0, 'false'):
            malformed = deepcopy(departed)
            malformed['overleaf']['online'] = value
            with self.subTest(online=value), self.assertRaisesRegex(AssertionError, 'online observation'):
                overleaf_page_departed(malformed, before)
        for field, value in (('online', False), ('sync_ready', False), ('source_locked', True)):
            unready = deepcopy(before)
            unready['overleaf'][field] = value
            with self.subTest(initial=field):
                self.assertFalse(overleaf_page_departed(departed, unready))

    def test_native_page_departure_requires_a_real_login_request_before_any_blocked_typing(self):
        from unittest.mock import Mock
        before = self.connected_document()
        before['auto_save'] = False
        before['overleaf']['online'] = True
        class Fake(OverleafJourneys):
            def snapshot(self): return dict(document=before)
            def overleaf_replace_source(self, source): return deepcopy(before)
        fixture = Mock()
        fixture.connection_requested.wait.return_value = True
        fixture.page_requested.wait.return_value = False
        fixture.save_count = 0
        journey = Fake()
        journey.native = Mock()
        with self.assertRaisesRegex(AssertionError, 'real browser never navigated'):
            journey.overleaf_page_departure(fixture, 'baseline')
        fixture.depart_page.assert_called_once_with()
        journey.native.assert_not_called()
        fixture.restore_page.assert_not_called()

    def test_chooser_roundtrip_requires_real_mode_root_connection_and_every_source_field(self):
        before = self.connected_document()
        local = deepcopy(before)
        local['workspace_chooser_visible'] = True
        self.assertTrue(overleaf_chooser_state(local, before, local=True, remote=False,
                                              root=Path('/private/tmp/remote')))
        remote = deepcopy(before)
        remote['overleaf']['visible'] = True
        self.assertTrue(overleaf_chooser_state(remote, before, local=False, remote=True,
                                              root=Path('/private/tmp/remote')))
        selected = deepcopy(before)
        selected['workspace_root_fingerprint'] = fingerprint('/private/tmp/local')
        self.assertFalse(overleaf_chooser_state(selected, before, local=False, remote=False,
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

    def test_workspace_creation_requires_explicit_worker_and_draft_evidence(self):
        document = self.connected_document()
        state = dict(visible=True, busy=False, use_overleaf=False,
                     name_fingerprint=fingerprint('new-workspace'),
                     parent_fingerprint=fingerprint('/private/tmp'), error=None)
        document['workspace_creation'] = state
        self.assertEqual(workspace_creation_state(document), state)
        failed = deepcopy(document)
        failed['workspace_creation'].update(error='The folder already exists.')
        self.assertEqual(workspace_creation_state(failed)['error'], 'The folder already exists.')
        hidden = deepcopy(document)
        hidden['workspace_creation'].update(visible=False, name_fingerprint=None,
                                           parent_fingerprint=None)
        self.assertFalse(workspace_creation_state(hidden)['visible'])
        for field in state:
            missing = deepcopy(document)
            del missing['workspace_creation'][field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                workspace_creation_state(missing)
        for field, value in (('visible', 1), ('busy', None), ('use_overleaf', 'true'),
                             ('name_fingerprint', None), ('parent_fingerprint', '/raw/path'),
                             ('error', ''), ('error', False)):
            broken = deepcopy(document)
            broken['workspace_creation'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                workspace_creation_state(broken)
        with self.assertRaises(AssertionError):
            workspace_creation_state(self.connected_document())

    def test_workspace_overleaf_checkbox_does_not_reveal_a_project_field(self):
        document = self.connected_document()
        document['workspace_creation'] = dict(visible=True, busy=False, use_overleaf=True,
                                              name_fingerprint=fingerprint('new-workspace'),
                                              parent_fingerprint=fingerprint('/private/tmp'), error=None)
        target = dict(enabled=True, frame=42, age_ms=1, viewport='workspace-child')
        snapshot = dict(document=document, targets={'workspace.name': dict(target),
                                                   'workspace.overleaf': dict(target)})
        self.assertTrue(workspace_creation_options_hidden(snapshot))
        forbidden = deepcopy(snapshot)
        forbidden['targets']['workspace.project_url'] = dict(target)
        self.assertFalse(workspace_creation_options_hidden(forbidden))
        forbidden['targets']['workspace.project_url']['frame'] = 41
        self.assertTrue(workspace_creation_options_hidden(forbidden))
        for field, value in (('enabled', False), ('age_ms', 501), ('frame', 41),
                             ('viewport', 'another-child')):
            changed = deepcopy(snapshot)
            changed['targets']['workspace.overleaf'][field] = value
            with self.subTest(field=field):
                self.assertFalse(workspace_creation_options_hidden(changed))
        hidden = deepcopy(snapshot)
        hidden['document']['workspace_creation']['visible'] = False
        self.assertFalse(workspace_creation_options_hidden(hidden))

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
