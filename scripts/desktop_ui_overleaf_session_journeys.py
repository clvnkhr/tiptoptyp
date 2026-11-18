"""Durable Overleaf workspaces through native controls and loopback HTTP only."""
from pathlib import Path
import re

from desktop_ui_overleaf_journeys import (
    overleaf_browser_at, overleaf_reconcile_state, overleaf_review_pair,
    overleaf_review_docked, overleaf_state, workspace_creation_state,
    workspace_creation_options_hidden,
)
from desktop_ui_workspace_switch_journeys import workspace_root_is, workspace_tab_ids

OVERLEAF_SESSION_JOURNEYS = ('overleaf_session',)
SESSION_SEED = 'Opening baseline.\nMiddle baseline.\nClosing baseline.\n'


def _fingerprint(value):
    from desktop_ui_journeys import fingerprint
    return fingerprint(value)


def overleaf_session_state(document):
    """Missing persistence evidence is never treated as a completed checkpoint."""
    state = overleaf_reconcile_state(document)
    flags = ('offline_mode', 'restored_session', 'session_saved', 'local_save_pending')
    if (any(type(state.get(field)) is not bool for field in flags)
            or 'profile_fingerprint' not in state
            or state['profile_fingerprint'] is not None and
               (not isinstance(state['profile_fingerprint'], str)
                or re.fullmatch(r'[0-9a-f]{16}', state['profile_fingerprint']) is None)
            or state['connection'] and state['profile_fingerprint'] is None
            or state['local_save_pending'] and state['session_saved']
            or state['offline_mode'] and state.get('online') is not False):
        raise AssertionError('Overleaf workspace persistence inspection is missing or malformed')
    return state


def overleaf_offline_saved(document, *, root, source, profile, restored=False):
    state = overleaf_session_state(document)
    return (workspace_root_is(document, root) and document.get('path') == str(root / 'main.txt')
            and document.get('source_fingerprint') == _fingerprint(source)
            and document.get('source_bytes') == len(source.encode('utf-8'))
            and document.get('dirty') is False and not document.get('modal_open')
            and state['connection'] and state['offline_mode'] and not state['source_locked']
            and state['profile_fingerprint'] == profile and state['session_saved']
            and not state['local_save_pending'] and not state['pending']
            and (not restored or state['restored_session']))


class OverleafSessionJourneys:
    def overleaf_session_create_workspace(self, name):
        """Create a real storage base using the same small workspace form as users."""
        root = self.directory.resolve()
        base = root / name
        self.focus_document_window()
        self.key(31, 'cmd+shift')
        self.wait('Local folders opens before creating the Overleaf workspace',
                  lambda d: d['workspace_chooser_visible'])
        self.click('chooser.new_workspace')
        self.wait('The real workspace form opens',
                  lambda d: workspace_creation_state(d)['visible'])
        for target, value, field in (
                ('workspace.parent', str(root), 'parent_fingerprint'),
                ('workspace.name', name, 'name_fingerprint')):
            self.click(target)
            self.wait_target(target, focused=True)
            self.key(0)
            self.native('text', value)
            self.wait('Workspace creation receives exact native input', lambda d:
                      workspace_creation_state(d)[field] == _fingerprint(value))
        if not workspace_creation_state(self.snapshot()['document'])['use_overleaf']:
            self.click('workspace.overleaf')
        self.wait('Overleaf project selection stays in the usual connection window', lambda d:
                  workspace_creation_options_hidden(self.snapshot()))
        self.click('workspace.create')
        self.wait('Created workspace becomes the pending offline storage location', lambda d:
                  not workspace_creation_state(d)['visible']
                  and not workspace_creation_state(d)['busy']
                  and overleaf_state(d)['visible']
                  and overleaf_state(d)['storage_keep_local_copy']
                  and overleaf_state(d)['storage_directory_fingerprint'] == _fingerprint(str(base)))
        if not base.is_dir():
            raise AssertionError('Workspace creation did not create its real folder')
        return base

    def overleaf_session_connect(self, fixture, *, signin_count=None, conflict=False):
        self.open_overleaf_connection()
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        self.key(0)
        self.native('text', fixture.url)
        self.wait('The real project address is entered', lambda d:
                  overleaf_state(d)['query_fingerprint'] == _fingerprint(fixture.url))
        self.click('overleaf.connect')
        if signin_count is not None:
            self.wait('This workspace requires its own real HTTP sign-in', lambda d:
                      overleaf_browser_at(d, fixture.login_url)
                      and not overleaf_state(d)['sync_ready'], timeout=15)
            self.overleaf_fixture_button(34)
        ready = self.wait('Authenticated project reaches a durable fresh working copy', lambda d:
                          overleaf_session_state(d)['sync_ready']
                          and overleaf_session_state(d)['session_saved']
                          and (overleaf_state(d)['source_locked'] is conflict)
                          and overleaf_state(d)['blocked_count'] == (1 if conflict else 0)
                          and not overleaf_session_state(d)['offline_mode'], timeout=25)['document']
        if signin_count is not None and fixture.auth_observation()['signins'] != signin_count:
            raise AssertionError('Workspace login did not use exactly one real fixture form submission')
        return ready

    def overleaf_session_offline_save(self, fixture, root, source, profile):
        self.open_overleaf_connection()
        self.click('overleaf.work_offline')
        self.wait('Work offline unlocks only the retained local copy', lambda d:
                  overleaf_session_state(d)['offline_mode']
                  and not overleaf_state(d)['source_locked'])
        if overleaf_state(self.snapshot()['document'])['visible']:
            self.click('overleaf.hide')
            self.wait('Offline editing returns to the native source window', lambda d:
                      not overleaf_state(d)['visible'])
        server, writes = fixture.source, fixture.save_count
        self.overleaf_replace_source(source)
        self.key(1)
        self.wait('Offline Save writes source and the restart checkpoint without publishing', lambda d:
                  overleaf_offline_saved(d, root=root, source=source, profile=profile)
                  and (root / 'main.txt').read_text(encoding='utf-8') == source, timeout=20)
        if fixture.source != server or fixture.save_count != writes:
            raise AssertionError('Offline Save sent a document to the remote server')

    def overleaf_session_reopen(self, base, root, source, profile, *, offline):
        old = workspace_tab_ids(self.snapshot()['document'])
        local = self.directory.resolve()
        self.choose_workspace(local)
        self.wait('Switching workspace retires every old project-owned page', lambda d:
                  workspace_root_is(d, local) and not set(workspace_tab_ids(d)).intersection(old)
                  and not d['modal_open'], timeout=20)
        local_tabs = workspace_tab_ids(self.snapshot()['document'])
        self.choose_workspace(base)
        if offline:
            return self.wait('Reopening the selected storage base restores its saved offline session', lambda d:
                             overleaf_offline_saved(d, root=root, source=source, profile=profile, restored=True)
                             and not set(workspace_tab_ids(d)).intersection(local_tabs), timeout=25)['document']
        return self.wait('Reopening the selected storage base reconnects its own remembered login', lambda d:
                         workspace_root_is(d, root)
                         and d.get('source_fingerprint') == _fingerprint(source)
                         and overleaf_session_state(d)['profile_fingerprint'] == profile
                         and overleaf_session_state(d)['session_saved']
                         and overleaf_state(d)['sync_ready']
                         and not overleaf_session_state(d)['offline_mode']
                         and not set(workspace_tab_ids(d)).intersection(local_tabs), timeout=25)['document']

    def overleaf_session(self):
        """Roundtrip durable local drafts, three-way reviews, and scoped login stores."""
        from overleaf_auth_fixture import AuthStorageFixture
        initial = self.snapshot()['document']
        initial_auto_save = initial.get('auto_save')
        if type(initial_auto_save) is not bool:
            raise AssertionError('Autosave observation is unavailable')
        if initial_auto_save:
            self.tex_settings([('"auto_save": true', '"auto_save": false')])
        with AuthStorageFixture(connection_events=True, multi_sessions=True) as fixture:
            fixture.replace_remote(SESSION_SEED)
            fixture.release_assets.set()
            fixture.enable_auth()
            first_base = self.overleaf_session_create_workspace('overleaf-workspace-a')
            first = self.overleaf_session_connect(fixture, signin_count=1)
            first_root = Path(first['path']).parent
            first_profile = overleaf_session_state(first)['profile_fingerprint']
            if first_root.parent != first_base:
                raise AssertionError('Created workspace was not retained as the selected storage base')
            local = SESSION_SEED.replace('Opening baseline.', 'Opening local edit.')
            self.overleaf_session_offline_save(fixture, first_root, local, first_profile)
            self.overleaf_session_reopen(first_base, first_root, local, first_profile, offline=True)
            remote = SESSION_SEED.replace('Closing baseline.', 'Closing remote edit.')
            merged = local.replace('Closing baseline.', 'Closing remote edit.')
            fixture.replace_remote(remote)
            writes = fixture.save_count
            self.overleaf_session_connect(fixture)
            self.wait('Fresh remote files merge with durable offline edits without a conflict', lambda d:
                      d['source_fingerprint'] == _fingerprint(merged) and d['dirty']
                      and overleaf_reconcile_state(d)['blocked_count'] == 0
                      and not overleaf_state(d)['pending'])
            if fixture.source != remote or fixture.save_count != writes:
                raise AssertionError('Reopening or refreshing published an unapproved merged draft')
            self.overleaf_save_fixture(fixture, merged)
            self.record('overleaf.session_offline_merge', workspace_association_restored=True,
                        local_save_survived_workspace_switch=True, fresh_remote_merge=True,
                        explicit_server_save=True, remote_account=False)

            overlapping_local = merged.replace('Middle baseline.', 'Middle local edit.')
            overlapping_remote = merged.replace('Middle baseline.', 'Middle remote edit.')
            self.overleaf_session_offline_save(fixture, first_root, overlapping_local, first_profile)
            self.overleaf_session_reopen(first_base, first_root, overlapping_local, first_profile, offline=True)
            fixture.replace_remote(overlapping_remote)
            writes = fixture.save_count
            self.overleaf_session_connect(fixture, conflict=True)
            self.wait('Overlapping durable edits require explicit review after reconnect', lambda d:
                      d['source_fingerprint'] == _fingerprint(overlapping_local)
                      and overleaf_reconcile_state(d)['blocked_count'] == 1
                      and overleaf_state(d)['source_locked'])
            for cancel in (True, False):
                self.open_overleaf_connection()
                self.click('overleaf.review')
                self.wait('The diff presents the complete refreshed remote and saved local versions', lambda d:
                          overleaf_review_pair(d, overlapping_remote, overlapping_local))
                overleaf_review_docked(self.snapshot())
                self.click('overleaf.review.cancel' if cancel else 'overleaf.review.use_local')
                self.wait('Review closes while preserving the explicit resolution state', lambda d:
                          not overleaf_reconcile_state(d)['review']['open']
                          and overleaf_state(d)['blocked_count'] == (1 if cancel else 0)
                          and d['source_fingerprint'] == _fingerprint(overlapping_local))
                if fixture.source != overlapping_remote or fixture.save_count != writes:
                    raise AssertionError('Review cancellation or local choice silently published source')
            self.overleaf_save_fixture(fixture, overlapping_local)
            self.wait('The resolved session checkpoint is durable', lambda d:
                      overleaf_session_state(d)['session_saved'])
            self.record('overleaf.session_conflict', exact_diff_pair=True, cancel_retains_conflict=True,
                        explicit_resolution=True, explicit_server_save=True, remote_account=False)

            # Same origin and project, different workspace: browser cookies must
            # be distinct, and signing in here must leave the first session valid.
            self.choose_workspace(self.directory.resolve())
            self.wait('A plain workspace leaves the completed project', lambda d:
                      workspace_root_is(d, self.directory.resolve()) and not d['modal_open'])
            second_base = self.overleaf_session_create_workspace('overleaf-workspace-b')
            second = self.overleaf_session_connect(fixture, signin_count=2)
            second_profile = overleaf_session_state(second)['profile_fingerprint']
            if first_profile == second_profile or Path(second['path']).parent.parent != second_base:
                raise AssertionError('Distinct workspaces shared a browser profile or local storage root')
            counts = fixture.session_project_counts()
            if set(counts) != {1, 2}:
                raise AssertionError('The real WebViews never used two distinct synthetic authenticated sessions')
            self.overleaf_session_reopen(first_base, first_root, overlapping_local, first_profile, offline=False)
            after = fixture.session_project_counts()
            if (fixture.auth_observation()['signins'] != 2 or after.get(1, 0) <= counts[1]
                    or after.get(2) != counts[2]):
                raise AssertionError('Reopening the first workspace lost or borrowed another workspace login')
            self.record('overleaf.session_profile_isolation', separate_profile_ids=True,
                        two_real_http_signins=True, original_login_reused=True,
                        cookie_material_observed=False, remote_account=False)
            # Leave only fixture-owned local files active and return preferences.
            self.choose_workspace(self.directory.resolve())
            self.wait('The completed session journey returns to its original local workspace', lambda d:
                      workspace_root_is(d, self.directory.resolve()) and not d['modal_open'])
            if initial_auto_save:
                self.tex_settings([('"auto_save": false', '"auto_save": true')])
