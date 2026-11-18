"""Overleaf source backups through native Git controls and a local bare remote."""
import json
import math
import os
from pathlib import Path
import re
import subprocess

from desktop_ui_overleaf_session_journeys import overleaf_session_state, overleaf_offline_saved
from desktop_ui_workspace_switch_journeys import workspace_root_is

GIT_SETUP_JOURNEYS = ('git_setup',)
GIT_AUTHOR = 'UI Git Backup'
GIT_EMAIL = 'backup@example.invalid'
GIT_BRANCH = 'backup-main'
GIT_COMMIT = 'Back up the saved offline Overleaf source'
GIT_SOURCE = 'Saved offline Overleaf source for the Git backup. αβ\n'
GIT_NOTE = '@misc{backup, title={Local source backup}}\n'


def _fingerprint(value):
    from desktop_ui_journeys import fingerprint
    return fingerprint(value)


def git_fixture_paths(directory):
    root = Path(directory).resolve()
    return dict(global_config=root / 'isolated-gitconfig',
                remote=root / 'local backup.git', destination=root / 'other-local-workspace')


def git_setup_environment(directory, environment=None):
    """Never inherit Git overrides or read/write the operator's Git configuration."""
    environment = dict(os.environ if environment is None else environment)
    environment = {key: value for key, value in environment.items() if not key.startswith('GIT_')}
    environment.update(GIT_CONFIG_GLOBAL=str(git_fixture_paths(directory)['global_config']),
                       GIT_CONFIG_SYSTEM=os.devnull, GIT_CONFIG_NOSYSTEM='1',
                       GIT_TERMINAL_PROMPT='0')
    return environment


def git_fixture_output(directory, *arguments):
    return subprocess.run(['git', *map(str, arguments)], cwd=Path(directory).resolve(),
                          env=git_setup_environment(directory), check=True,
                          capture_output=True, timeout=15).stdout


def git_fixture_command(directory, *arguments):
    return git_fixture_output(directory, *arguments).decode('utf-8').strip()


def prepare_git_setup_fixture(directory):
    """Prepare the remote only; the app must create/configure the working repository."""
    paths = git_fixture_paths(directory)
    paths['global_config'].write_text('', encoding='utf-8')
    paths['destination'].mkdir()
    git_fixture_command(directory, 'init', '--bare', '--quiet', paths['remote'])
    return paths


def git_setup_state(document):
    state = document.get('git_setup')
    flags = ('visible', 'busy', 'loaded', 'initialized', 'author_set', 'origin_set')
    if (not isinstance(state, dict)
            or any(type(state.get(field)) is not bool for field in flags)):
        raise AssertionError('Git setup inspection is missing or malformed')
    for field in ('name_fingerprint', 'email_fingerprint'):
        value = state.get(field)
        if value is None and not state['visible']:
            continue
        if not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{16}', value) is None:
            raise AssertionError('Git setup author inspection is malformed')
    return state


def git_panel_state(document):
    state = document.get('git')
    if (not isinstance(state, dict)
            or any(type(state.get(field)) is not bool
                   for field in ('initialized', 'busy', 'failed', 'commit_message_set'))
            or any(type(state.get(field)) is not int or state[field] < 0
                   for field in ('changes', 'staged'))):
        raise AssertionError('Git repository inspection is missing or malformed')
    return state


def git_setup_entry_current(snapshot):
    """A retained target from a just-hidden Git section cannot authorize input."""
    document = snapshot.get('document', {})
    target = snapshot.get('targets', {}).get('git.setup.open')
    return (isinstance(target, dict) and target.get('enabled') is True
            and type(document.get('frame')) is int and target.get('frame') == document['frame']
            and isinstance(document.get('viewport'), str)
            and target.get('viewport') == document['viewport']
            and type(target.get('age_ms')) in (int, float) and 0 <= target['age_ms'] <= 500)


def git_setup_ready(document, root, *, initialized, name=None, email=None, origin=None):
    state = git_setup_state(document)
    if state['visible'] and state.get('error'):
        raise AssertionError('Git setup failed: ' + str(state['error']))
    return (state['visible'] and state['loaded'] and not state['busy']
            and state['initialized'] is initialized and state.get('workspace') == str(root)
            and state.get('root') == str(root)
            and (name is None or state['name_fingerprint'] == _fingerprint(name))
            and (email is None or state['email_fingerprint'] == _fingerprint(email))
            and (origin is None or state['origin_set'] is origin))


def assert_git_setup_geometry(snapshot, *, new_repository, github=False):
    """Every single-page form control must fit the real native child viewport."""
    state = git_setup_state(snapshot['document'])
    rect, viewport = state.get('rect'), state.get('viewport')
    if (not state['visible'] or not isinstance(viewport, str) or not viewport
            or not isinstance(rect, list) or len(rect) != 4
            or any(type(value) not in (int, float) or not math.isfinite(value) for value in rect)
            or rect[2] <= rect[0] or rect[3] <= rect[1]):
        raise AssertionError('Git setup child viewport geometry is unavailable')
    names = ['git.setup.name', 'git.setup.email', 'git.setup.submit', 'git.setup.cancel']
    names.extend(['github.name', 'github.private'] if github else ['git.setup.origin'])
    if new_repository:
        names.append('git.setup.branch')
    fields = []
    for name in names:
        target = snapshot.get('targets', {}).get(name)
        if (not isinstance(target, dict) or target.get('viewport') != viewport
                or type(target.get('age_ms')) not in (int, float) or target['age_ms'] > 500
                or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                       for key in ('x', 'y', 'width', 'height'))
                or target['width'] < 20 or target['height'] < 12):
            raise AssertionError('Git setup has a missing, stale or invalid control: ' + name)
        bounds = [target['x'] - target['width'] / 2, target['y'] - target['height'] / 2,
                  target['x'] + target['width'] / 2, target['y'] + target['height'] / 2]
        if any((bounds[0] < rect[0] - 1, bounds[1] < rect[1] - 1,
                bounds[2] > rect[2] + 1, bounds[3] > rect[3] + 1)):
            raise AssertionError('Git setup control is clipped by its native viewport: ' + name)
        if name in ('git.setup.name', 'git.setup.email', 'git.setup.branch',
                    'git.setup.origin', 'github.name'):
            fields.append((name, bounds))
    for index, (name, first) in enumerate(fields):
        for other, second in fields[index + 1:]:
            if (min(first[2], second[2]) > max(first[0], second[0]) + 1
                    and min(first[3], second[3]) > max(first[1], second[1]) + 1):
                raise AssertionError('Git setup fields overlap: ' + name + '/' + other)
    return dict(viewport=viewport, controls=len(names), rect=rect)


def assert_git_control_disabled(snapshot, name):
    target = snapshot.get('targets', {}).get(name)
    if (not isinstance(target, dict) or target.get('enabled') is not False
            or target.get('viewport') != snapshot['document'].get('viewport')
            or type(target.get('age_ms')) not in (int, float) or target['age_ms'] > 500):
        raise AssertionError('Overleaf source ownership did not disable the real control: ' + name)


def assert_git_backup(directory, root, *, pushed, remote=None, origin=None, expected_global_config=b''):
    """Inspect the real repository and bare remote, never infer success from a notice."""
    paths = git_fixture_paths(directory)
    remote_path = paths['remote'] if remote is None else Path(remote)
    command = lambda *args: git_fixture_command(directory, '-C', root, *args)
    if command('rev-parse', '--show-toplevel') != str(root):
        raise AssertionError('Git setup initialized the wrong working repository')
    for key, expected in (('user.name', GIT_AUTHOR), ('user.email', GIT_EMAIL),
                          ('remote.origin.url', str(remote_path) if origin is None else origin)):
        if command('config', '--local', '--get', key) != expected:
            raise AssertionError('Git setup did not persist the requested local ' + key)
    if command('remote', 'get-url', 'origin') != str(remote_path):
        raise AssertionError('The fixture origin does not resolve to its owned local bare repository')
    if command('symbolic-ref', '--short', 'HEAD') != GIT_BRANCH:
        raise AssertionError('Git setup selected a different initial branch')
    tracked = set(command('ls-tree', '-r', '--name-only', 'HEAD').splitlines())
    if tracked != {'main.txt', 'fixture.bin', 'references.bib'}:
        raise AssertionError('The backup omitted real source or included private/runtime files: '
                             + repr(sorted(tracked)))
    if git_fixture_output(directory, '-C', root, 'show', 'HEAD:main.txt') != GIT_SOURCE.encode('utf-8'):
        raise AssertionError('The Git commit did not contain the deliberately saved offline source')
    if git_fixture_output(directory, '-C', root, 'show', 'HEAD:references.bib') != GIT_NOTE.encode('utf-8'):
        raise AssertionError('The Git commit omitted the additional real source file')
    if git_fixture_output(directory, '-C', root, 'show', 'HEAD:fixture.bin') != (root / 'fixture.bin').read_bytes():
        raise AssertionError('The Git commit changed the downloaded Overleaf asset')
    if command('log', '-1', '--format=%an <%ae>') != GIT_AUTHOR + ' <' + GIT_EMAIL + '>':
        raise AssertionError('The first backup used another Git author')
    remote = lambda *args: git_fixture_command(directory, '--git-dir', remote_path, *args)
    if pushed:
        if (command('rev-parse', 'HEAD') != remote('rev-parse', 'refs/heads/' + GIT_BRANCH)
                or command('rev-parse', '--abbrev-ref', '@{upstream}') != 'origin/' + GIT_BRANCH
                or command('config', '--local', '--get', 'branch.' + GIT_BRANCH + '.remote') != 'origin'):
            raise AssertionError('The first Push did not save the commit and set its upstream')
    elif remote('for-each-ref', '--format=%(refname)'):
        raise AssertionError('The app published a commit before explicit Push')
    if paths['global_config'].read_bytes() != expected_global_config:
        raise AssertionError('Workspace Git setup changed global Git configuration')
    return tracked


class GitSetupJourneys:
    def show_git_setup_entry(self):
        # View > Git toggles visibility. The isolated workspace may already
        # show Git, so inspect the current frame before issuing that command.
        if not git_setup_entry_current(self.snapshot()):
            self.key(5, 'cmd+alt')
            self.wait('Git setup entry is rendered in the current document frame', lambda _d:
                      git_setup_entry_current(self.snapshot()))

    def git_setup_type(self, field, value, fingerprint=None):
        target = 'git.setup.' + field
        self.click(target)
        self.wait_target(target, focused=True)
        self.key(0)
        self.native('text', value)
        if fingerprint is not None:
            self.wait('Git setup receives the exact native field input', lambda d:
                      git_setup_state(d)[fingerprint] == _fingerprint(value))

    def git_setup(self):
        from overleaf_storage_fixture import StorageFixture
        paths = git_fixture_paths(self.directory)
        if not paths['remote'].is_dir() or paths['global_config'].read_bytes() != b'':
            raise AssertionError('Git setup requires an isolated prepared remote/configuration')
        initial_auto_save = self.snapshot()['document'].get('auto_save')
        if type(initial_auto_save) is not bool:
            raise AssertionError('Autosave inspection is unavailable')
        recent = '"recent_workspaces": ['
        edits = [(recent, recent + json.dumps(str(paths['destination'])) + ',')]
        if initial_auto_save:
            edits.append(('"auto_save": true', '"auto_save": false'))
        self.tex_settings(edits)
        with StorageFixture(project_name='Git source backup fixture') as fixture:
            fixture.release_assets.set()
            base = self.overleaf_session_create_workspace('git-overleaf-workspace')
            connected = self.overleaf_session_connect(fixture)
            root = Path(connected['path']).parent
            if root.parent != base or (root / '.git').exists():
                raise AssertionError('Git setup must start from its own new Overleaf working copy')
            profile = overleaf_session_state(connected)['profile_fingerprint']
            self.overleaf_session_offline_save(fixture, root, GIT_SOURCE, profile)
            server_source, server_saves = fixture.source, fixture.save_count
            (root / 'references.bib').write_text(GIT_NOTE, encoding='utf-8')
            (root / '.tiptoptyp/private-backup-sentinel.txt').write_text(
                'Synthetic private metadata: never commit this file.\n', encoding='utf-8')
            self.show_git_setup_entry()
            self.click('git.setup.open')
            self.wait('Git setup loads the actual Overleaf storage root', lambda d:
                      git_setup_ready(d, root, initialized=False))
            for field, value, evidence in (
                    ('name', GIT_AUTHOR, 'name_fingerprint'),
                    ('email', GIT_EMAIL, 'email_fingerprint'),
                    ('branch', GIT_BRANCH, None),
                    ('origin', str(paths['remote']), None)):
                self.git_setup_type(field, value, evidence)
            self.wait('Git setup preserves the requested branch and local remote choice', lambda d:
                      git_setup_ready(d, root, initialized=False, name=GIT_AUTHOR,
                                      email=GIT_EMAIL, origin=True)
                      and git_setup_state(d).get('branch') == GIT_BRANCH)
            geometry = assert_git_setup_geometry(self.snapshot(), new_repository=True)
            if getattr(self, 'capture_review', False):
                self.capture_viewport('git-setup')
            self.click('git.setup.submit')
            self.wait('Create repository closes setup without publishing or changing source', lambda d:
                      not git_setup_state(d)['visible'] and git_panel_state(d)['initialized']
                      and not git_panel_state(d)['busy'] and not git_panel_state(d)['failed']
                      and workspace_root_is(d, root) and not d['dirty'], timeout=20)
            if git_fixture_command(self.directory, '--git-dir', paths['remote'],
                                   'for-each-ref', '--format=%(refname)'):
                raise AssertionError('Setup created a remote commit before explicit staging/commit/push')
            assert_git_control_disabled(self.snapshot(), 'git.revert')
            self.click('git.stage_all')
            self.wait('Stage all contains only the three source/asset files', lambda d:
                      not git_panel_state(d)['busy'] and git_panel_state(d)['staged'] == 3)
            self.click('git.commit_message')
            self.wait_target('git.commit_message', focused=True)
            self.native('text', GIT_COMMIT)
            self.wait('The first backup commit message receives real native typing', lambda d:
                      git_panel_state(d).get('commit_message_fingerprint') == _fingerprint(GIT_COMMIT))
            self.click('git.commit')
            self.wait('Commit staged completes the local backup', lambda d:
                      not git_panel_state(d)['busy'] and not git_panel_state(d)['failed']
                      and git_panel_state(d)['staged'] == 0, timeout=20)
            assert_git_backup(self.directory, root, pushed=False)
            commit = git_fixture_command(self.directory, '-C', root, 'rev-parse', 'HEAD')
            self.click('git.push')
            self.wait('The first explicit Push completes against the private bare remote', lambda d:
                      not git_panel_state(d)['busy'] and not git_panel_state(d)['failed']
                      and git_fixture_command(self.directory, '--git-dir', paths['remote'],
                                              'for-each-ref', '--format=%(objectname)',
                                              'refs/heads/' + GIT_BRANCH) == commit, timeout=20)
            tracked = assert_git_backup(self.directory, root, pushed=True)
            assert_git_control_disabled(self.snapshot(), 'git.pull')
            self.wait('Git backup preserves the independently saved offline Overleaf draft', lambda d:
                      overleaf_offline_saved(d, root=root, source=GIT_SOURCE, profile=profile))
            if fixture.source != server_source or fixture.save_count != server_saves:
                raise AssertionError('Git backup unexpectedly published source to Overleaf')
            self.record('git.setup.first_backup', overleaf_offline_save=True,
                        source_files=sorted(tracked), private_metadata_excluded=True,
                        local_author=True, upstream_set=True, explicit_push=True,
                        overleaf_pull_and_revert_disabled=True,
                        overleaf_server_unchanged=True,
                        global_git_config_unchanged=True, geometry=geometry, remote_account=False)

            configuration = (root / '.git/config').read_bytes()
            self.show_git_setup_entry()
            self.click('git.setup.open')
            self.wait('Configure Git prefills the saved workspace author and remote presence', lambda d:
                      git_setup_ready(d, root, initialized=True, name=GIT_AUTHOR,
                                      email=GIT_EMAIL, origin=True)
                      and git_setup_state(d).get('branch') == GIT_BRANCH)
            assert_git_setup_geometry(self.snapshot(), new_repository=False)
            self.git_setup_type('name', 'Cancelled author edit', 'name_fingerprint')
            self.click('git.setup.cancel')
            self.wait('Cancel closes configuration and retains the original workspace', lambda d:
                      not git_setup_state(d)['visible'] and workspace_root_is(d, root))
            if (root / '.git/config').read_bytes() != configuration:
                raise AssertionError('Cancelling Configure Git changed repository settings')
            self.show_git_setup_entry()
            self.click('git.setup.open')
            self.wait('Reopening configuration discards the cancelled form edits', lambda d:
                      git_setup_ready(d, root, initialized=True, name=GIT_AUTHOR,
                                      email=GIT_EMAIL, origin=True))
            self.key(31, 'cmd+shift')
            self.stable_state(lambda d: workspace_root_is(d, root)
                              and git_setup_state(d)['visible']
                              and not d['workspace_chooser_visible'], frames=4)
            self.click('git.setup.cancel')
            self.wait('Cancel releases workspace navigation without saving configuration', lambda d:
                      not git_setup_state(d)['visible'] and workspace_root_is(d, root))
            self.choose_workspace(paths['destination'])
            self.wait('The next workspace opens with no previous setup form', lambda d:
                      workspace_root_is(d, paths['destination']) and not d['modal_open']
                      and not git_setup_state(d)['visible'], timeout=20)
            if ((paths['destination'] / '.git').exists()
                    or (root / '.git/config').read_bytes() != configuration):
                raise AssertionError('A departed Git setup dialog mutated either workspace')
            assert_git_backup(self.directory, root, pushed=True)
            self.record('git.setup.configure_cancel', correct_prefill=True,
                        cancelled_changes_discarded=True, modal_blocks_workspace_navigation=True,
                        workspace_changed_after_cancel=True,
                        source_and_repository_preserved=True)
        # This dedicated journey owns isolated temporary settings. Its final
        # workspace is deliberately empty, so there is no source editor to
        # refocus through the shared settings helper before process cleanup.
