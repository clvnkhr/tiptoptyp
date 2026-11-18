"""Native GitHub setup UI against an explicitly synthetic local provider."""
import json
from pathlib import Path
import re

from desktop_ui_git_journeys import (
    GIT_AUTHOR, GIT_EMAIL, GIT_BRANCH, GIT_SOURCE, GIT_NOTE, GIT_COMMIT,
    git_fixture_paths, git_fixture_command, git_setup_state, git_setup_ready,
    git_panel_state, assert_git_setup_geometry, assert_git_backup,
)
from desktop_ui_overleaf_session_journeys import overleaf_session_state, overleaf_offline_saved
from desktop_ui_workspace_switch_journeys import workspace_root_is
from github_service_fixture import GitHubFixture, DEVICE_CODE, FIRST_ACCOUNT, SECOND_ACCOUNT

GITHUB_JOURNEYS = ('git_github',)


def _fingerprint(value):
    from desktop_ui_journeys import fingerprint
    return fingerprint(value)


def github_state(document):
    state = git_setup_state(document).get('github')
    flags = ('selected', 'signed_in', 'private', 'code_ready', 'busy',
             'open_requested', 'attach_failed', 'uncertain')
    if (not isinstance(state, dict)
            or any(type(state.get(field)) is not bool for field in flags)
            or not isinstance(state.get('stage'), str) or not state['stage']):
        raise AssertionError('GitHub setup inspection is missing or malformed')
    if state['signed_in'] and (type(state.get('account_id')) is not int or state['account_id'] < 1
            or not isinstance(state.get('account_login'), str)
            or re.fullmatch(r'[A-Za-z0-9-]{1,39}', state['account_login']) is None):
        raise AssertionError('GitHub account inspection is missing its public identity')
    for field in ('name_fingerprint', 'opened_url_fingerprint'):
        value = state.get(field)
        if field == 'name_fingerprint' and state['selected'] and value is None:
            raise AssertionError('GitHub repository name inspection is missing')
        if value is not None and (not isinstance(value, str)
                or re.fullmatch(r'[0-9a-f]{16}', value) is None):
            raise AssertionError('GitHub setup fingerprint is malformed')
    if state['open_requested'] and state.get('opened_url_fingerprint') is None:
        raise AssertionError('GitHub Open action has no verified destination evidence')
    return state


def github_account_ready(document, account):
    state = github_state(document)
    return (state['selected'] and state['signed_in'] and not state['busy']
            and state.get('account_id') == account['id']
            and state.get('account_login') == account['login'])


def github_no_remote(directory, root):
    if not (root / '.git').exists():
        return True
    config = git_fixture_command(directory, '-C', root, 'config', '--local', '--list')
    if any(line.startswith(('remote.origin.', 'credential.')) for line in config.splitlines()):
        raise AssertionError('Cancelled GitHub creation attached a remote or credential helper')
    return True


def github_departed_outcome(document, root, expected_url):
    outcome = git_setup_state(document).get('last_github_outcome')
    if outcome is None:
        return False
    if (not isinstance(outcome, dict)
            or type(outcome.get('confirmed')) is not bool
            or type(outcome.get('attach_failed')) is not bool):
        raise AssertionError('The departed GitHub outcome is missing its confirmation state')
    field = 'created_url' if outcome['confirmed'] else 'expected_url'
    return (outcome.get('workspace') == str(root) and outcome['attach_failed']
            and outcome.get(field) == expected_url)


def github_remote_empty(directory, repository):
    if repository.get('private') is not True:
        raise AssertionError('Synthetic provider did not create the requested private repository')
    owner, name = repository.get('owner', {}).get('login'), repository.get('name')
    if (repository.get('html_url') != f'https://github.com/{owner}/{name}'
            or repository.get('clone_url') != f'https://github.com/{owner}/{name}.git'):
        raise AssertionError('Synthetic API did not preserve the strict canonical repository URLs')
    remote = Path(repository['fixture_remote'])
    if not remote.resolve().is_relative_to(Path(directory).resolve()):
        raise AssertionError('Synthetic GitHub remote escaped the private fixture')
    if git_fixture_command(directory, '--git-dir', remote, 'for-each-ref', '--format=%(refname)'):
        raise AssertionError('GitHub setup published source before explicit Push')
    return remote


class GitHubJourneys:
    def github_use_saved_account(self, account):
        # Merely opening Git setup performs no account/network request. The
        # explicit Sign in action may reuse this app's existing secure login.
        if not github_account_ready(self.snapshot()['document'], account):
            self.click('github.sign_in')
            self.wait('Explicit Sign in reuses the saved synthetic account', lambda d:
                      github_account_ready(d, account), timeout=20)

    def github_open_form(self, root, name):
        self.show_git_setup_entry()
        self.click('git.setup.open')
        loaded = self.wait('GitHub setup owns the intended local storage workspace', lambda d:
                           git_setup_state(d)['visible'] and git_setup_state(d)['loaded']
                           and not git_setup_state(d)['busy']
                           and git_setup_state(d).get('workspace') == str(root))['document']
        recovered = github_state(loaded)
        if recovered.get('created_url') is not None or recovered['uncertain']:
            self.wait('Recovered creation retains its original repository name and author', lambda d:
                      github_state(d)['selected']
                      and github_state(d)['name_fingerprint'] == _fingerprint(name)
                      and git_setup_state(d)['name_fingerprint'] == _fingerprint(GIT_AUTHOR)
                      and git_setup_state(d)['email_fingerprint'] == _fingerprint(GIT_EMAIL))
            return
        self.git_setup_type('name', GIT_AUTHOR, 'name_fingerprint')
        self.git_setup_type('email', GIT_EMAIL, 'email_fingerprint')
        if not git_setup_state(loaded)['initialized']:
            self.git_setup_type('branch', GIT_BRANCH)
        if not github_state(self.snapshot()['document'])['selected']:
            self.click('git.setup.remote_github')
        self.wait('GitHub mode is ready for an explicit account action', lambda d:
                  github_state(d)['selected'] and not github_state(d)['busy'], timeout=20)
        inspected = github_state(self.snapshot()['document'])
        if inspected.get('created_url') is not None or inspected['uncertain']:
            self.wait('Recovered creation retains its original repository name', lambda d:
                      github_state(d)['name_fingerprint'] == _fingerprint(name))
            return
        self.click('github.name')
        self.wait_target('github.name', focused=True)
        self.key(0)
        self.native('text', name)
        self.wait('The GitHub repository name receives exact native typing', lambda d:
                  github_state(d)['name_fingerprint'] == _fingerprint(name)
                  and github_state(d)['private'])

    def github_workspace(self, fixture, name):
        base = self.overleaf_session_create_workspace(name)
        connected = self.overleaf_session_connect(fixture)
        root = Path(connected['path']).parent
        profile = overleaf_session_state(connected)['profile_fingerprint']
        self.overleaf_session_offline_save(fixture, root, GIT_SOURCE, profile)
        (root / 'references.bib').write_text(GIT_NOTE, encoding='utf-8')
        (root / '.tiptoptyp/private-github-sentinel.txt').write_text(
            'Synthetic private GitHub setup metadata.\n', encoding='utf-8')
        self.show_git_setup_entry()
        return base, root, profile

    def git_github(self):
        from overleaf_storage_fixture import StorageFixture
        paths = git_fixture_paths(self.directory)
        global_configuration = paths['global_config'].read_bytes()
        initial_auto_save = self.snapshot()['document'].get('auto_save')
        if type(initial_auto_save) is not bool:
            raise AssertionError('Autosave inspection is unavailable')
        recent = '"recent_workspaces": ['
        edits = [(recent, recent + json.dumps(str(paths['destination'])) + ',')]
        if initial_auto_save:
            edits.append(('"auto_save": true', '"auto_save": false'))
        self.tex_settings(edits)
        with GitHubFixture(self.directory) as provider, StorageFixture() as overleaf:
            overleaf.release_assets.set()
            _, root, profile = self.github_workspace(overleaf, 'github-backup-workspace')
            server_source, server_saves = overleaf.source, overleaf.save_count
            self.github_open_form(root, 'private-native')
            self.click('github.sign_in')
            self.wait('Native sign-in displays a synthetic device code without exposing credentials', lambda d:
                      github_state(d)['busy'] and github_state(d)['code_ready'], timeout=20)
            self.copy_with_receipt(DEVICE_CODE, lambda: self.click('github.code_copy'))
            self.click('github.open')
            self.wait('Open GitHub records only the validated loopback destination', lambda d:
                      github_state(d)['open_requested']
                      and github_state(d)['opened_url_fingerprint'] == _fingerprint(provider.verification_uri))
            provider.approve()  # Explicit provider simulation, not real browser consent.
            self.wait('The native form receives the synthetic signed-in public identity', lambda d:
                      github_account_ready(d, FIRST_ACCOUNT), timeout=20)
            self.record('git.github.synthetic_authentication', native_sign_in=True,
                        native_code_copy=True, verified_open_request=True,
                        simulated_provider_approval=True, external_browser_opened=False,
                        real_github_authentication=False, token_material_observed=False)

            provider.switch_account(SECOND_ACCOUNT)
            self.click('git.setup.submit')
            self.wait('A changed account is refused before creating a repository', lambda d:
                      git_setup_state(d)['visible'] and not git_setup_state(d)['busy']
                      and isinstance(git_setup_state(d).get('error'), str)
                      and 'account' in git_setup_state(d)['error'].lower(), timeout=20)
            if provider.observation()['create_requests'] != 0:
                raise AssertionError('A stale reviewed account reached the create-repository API')
            self.click('git.setup.cancel')
            self.wait('Closing stale account setup preserves the local workspace', lambda d:
                      not git_setup_state(d)['visible'] and workspace_root_is(d, root))
            self.github_open_form(root, 'private-native')
            self.github_use_saved_account(SECOND_ACCOUNT)
            geometry = assert_git_setup_geometry(self.snapshot(),
                new_repository=not git_setup_state(self.snapshot()['document'])['initialized'], github=True)
            if getattr(self, 'capture_review', False):
                self.capture_viewport('git-setup')
            self.click('git.setup.submit')
            self.wait('Explicit Create on GitHub confirms and connects a private empty repository', lambda d:
                      not git_setup_state(d)['busy']
                      and github_state(d).get('created_url') is not None
                      and not github_state(d)['attach_failed'], timeout=25)
            observations = provider.observation()
            if observations['create_requests'] != 1 or observations['create_count'] != 1:
                raise AssertionError('Explicit GitHub creation did not make exactly one API request')
            repository = observations['repositories'][0]
            remote = github_remote_empty(self.directory, repository)
            self.click('git.setup.cancel')
            self.wait('Closing the creation receipt returns to the source backup controls', lambda d:
                      not git_setup_state(d)['visible'] and git_panel_state(d)['initialized']
                      and not git_panel_state(d)['busy'], timeout=15)
            self.click('git.stage_all')
            self.wait('GitHub backup stages saved source and asset files', lambda d:
                      not git_panel_state(d)['busy'] and git_panel_state(d)['staged'] == 3)
            self.click('git.commit_message')
            self.wait_target('git.commit_message', focused=True)
            self.native('text', GIT_COMMIT)
            self.wait('GitHub backup commit message receives native input', lambda d:
                      git_panel_state(d).get('commit_message_fingerprint') == _fingerprint(GIT_COMMIT))
            self.click('git.commit')
            self.wait('Commit stays local until Push is explicitly clicked', lambda d:
                      not git_panel_state(d)['busy'] and not git_panel_state(d)['failed']
                      and git_panel_state(d)['staged'] == 0, timeout=20)
            assert_git_backup(self.directory, root, pushed=False, remote=remote,
                              origin=repository['clone_url'], expected_global_config=global_configuration)
            commit = git_fixture_command(self.directory, '-C', root, 'rev-parse', 'HEAD')
            self.click('git.push')
            self.wait('Explicit Push sends the first backup only to the private bare fixture', lambda d:
                      not git_panel_state(d)['busy'] and not git_panel_state(d)['failed']
                      and git_fixture_command(self.directory, '--git-dir', remote, 'for-each-ref',
                                              '--format=%(objectname)', 'refs/heads/' + GIT_BRANCH) == commit,
                      timeout=20)
            assert_git_backup(self.directory, root, pushed=True, remote=remote,
                              origin=repository['clone_url'], expected_global_config=global_configuration)
            self.wait('GitHub setup preserves saved offline Overleaf source ownership', lambda d:
                      overleaf_offline_saved(d, root=root, source=GIT_SOURCE, profile=profile))
            if overleaf.source != server_source or overleaf.save_count != server_saves:
                raise AssertionError('GitHub backup unexpectedly published source to Overleaf')
            self.record('git.github.private_create_backup', account_change_refused=True,
                        explicit_private_creation=True, creation_requests=1,
                        source_private_until_push=True, native_stage_commit_push=True,
                        private_metadata_excluded=True, local_remote=True,
                        canonical_origin=True, private_transport_rewrite=True, geometry=geometry,
                        overleaf_server_unchanged=True,
                        real_github_repository_created=False)

            self.choose_workspace(self.directory.resolve())
            self.wait('A separate workspace can own the cancellation scenario', lambda d:
                      workspace_root_is(d, self.directory.resolve()) and not d['modal_open'])
            departed_base, departed_root, departed_profile = self.github_workspace(
                overleaf, 'github-departed-workspace')
            self.github_open_form(departed_root, 'departed-private')
            self.github_use_saved_account(SECOND_ACCOUNT)
            provider.pause_next_creation()
            self.click('git.setup.submit')
            self.wait('The provider creates once while its acknowledgement is deliberately delayed', lambda d:
                      git_setup_state(d)['busy'] and provider.creation_started.is_set(), timeout=20)
            self.click('git.setup.cancel')
            self.wait('Closing the GitHub operation cancels the waiting helper without forgetting its origin', lambda d:
                      not git_setup_state(d)['visible'])
            self.choose_workspace(paths['destination'])
            self.wait('A newer workspace receives no stale setup dialog or repository', lambda d:
                      workspace_root_is(d, paths['destination']) and not git_setup_state(d)['visible']
                      and not d['modal_open'], timeout=20)
            provider.release_creation.set()
            departed_repository = provider.observation()['repositories'][1]
            self.wait('The departed helper finishes without taking the new workspace', lambda d:
                      not git_setup_state(d)['busy'] and workspace_root_is(d, paths['destination'])
                      and github_departed_outcome(d, departed_root, departed_repository['html_url']),
                      timeout=20)
            if (paths['destination'] / '.git').exists():
                raise AssertionError('A stale GitHub operation initialized the new workspace')
            github_no_remote(self.directory, departed_root)
            observations = provider.observation()
            if observations['create_requests'] != 2 or observations['create_count'] != 2:
                raise AssertionError('Cancellation retried creation or forgot the single provider outcome')
            github_remote_empty(self.directory, departed_repository)
            repository_reads = observations['recovery_reads']
            self.choose_workspace(departed_base)
            self.wait('Reopening restores the original offline workspace and saved source', lambda d:
                      overleaf_offline_saved(d, root=departed_root, source=GIT_SOURCE,
                                             profile=departed_profile, restored=True), timeout=25)
            self.github_open_form(departed_root, 'departed-private')
            self.github_use_saved_account(SECOND_ACCOUNT)
            self.wait('Reopening restores the durable uncertain creation without an automatic request', lambda d:
                      not git_setup_state(d)['busy']
                      and github_state(d).get('expected_url') == departed_repository['html_url']
                      and github_state(d)['uncertain']
                      and github_state(d)['attach_failed'], timeout=25)
            github_no_remote(self.directory, departed_root)
            self.click('github.retry_attach')
            self.wait('Explicit recovery looks up and attaches the known repository without another creation', lambda d:
                      not git_setup_state(d)['busy']
                      and github_state(d).get('created_url') == departed_repository['html_url']
                      and not github_state(d)['attach_failed'], timeout=20)
            if (provider.observation()['create_requests'] != 2
                    or provider.observation()['recovery_reads'] <= repository_reads):
                raise AssertionError('Recovery repeated POST instead of looking up the existing repository')
            if git_fixture_command(self.directory, '-C', departed_root, 'config', '--local',
                                   '--get', 'remote.origin.url') != departed_repository['clone_url']:
                raise AssertionError('Recovery did not attach to its original workspace')
            github_remote_empty(self.directory, departed_repository)
            self.click('git.setup.cancel')
            self.wait('Recovered setup closes without publishing files', lambda d:
                      not git_setup_state(d)['visible'])
            if overleaf.source != server_source or overleaf.save_count != server_saves:
                raise AssertionError('Recovering GitHub setup unexpectedly published source to Overleaf')
            self.record('git.github.cancelled_creation_recovered', single_post_per_request=True,
                        no_stale_workspace_attach=True, no_cancelled_remote_attach=True,
                        durable_reopen_lookup=True, explicit_retry_attach=True,
                        retry_did_not_create_repository=True, source_not_pushed=True,
                        real_github_repository_created=False)
            self.choose_workspace(self.directory.resolve())
            self.wait('The synthetic GitHub journey returns to its initial local workspace', lambda d:
                      workspace_root_is(d, self.directory.resolve()) and not d['modal_open'])
        # Dedicated fixture settings disappear with the isolated app. Restoring
        # a preference here must not require opening a file in an empty workspace.
