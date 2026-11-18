"""Failure detectors for Git backups; these tests are not native interaction evidence."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_git_journeys import (
    GIT_AUTHOR, GIT_EMAIL, GIT_BRANCH, GIT_COMMIT, GIT_SOURCE, GIT_NOTE,
    assert_git_backup, assert_git_setup_geometry, assert_git_control_disabled, git_fixture_command,
    git_fixture_paths, git_setup_environment, git_setup_ready, git_setup_state,
    git_setup_entry_current, GitSetupJourneys,
    prepare_git_setup_fixture,
)


class GitSetupContracts(unittest.TestCase):
    def entry_snapshot(self, *, frame=9):
        return dict(document=dict(frame=9, viewport='owner'), targets={
            'git.setup.open':dict(frame=frame, viewport='owner', age_ms=30, enabled=True)})

    def test_git_entry_requires_the_current_frame_even_when_a_hidden_hit_is_recent(self):
        self.assertTrue(git_setup_entry_current(self.entry_snapshot()))
        self.assertFalse(git_setup_entry_current(self.entry_snapshot(frame=8)))
        for change in ({'enabled':False}, {'viewport':'other'}, {'age_ms':501}):
            with self.subTest(change=change):
                snapshot = self.entry_snapshot()
                snapshot['targets']['git.setup.open'].update(change)
                self.assertFalse(git_setup_entry_current(snapshot))

    def test_show_git_does_not_toggle_an_already_visible_section_off(self):
        current = self.entry_snapshot()
        hidden = self.entry_snapshot(frame=8)
        class Harness(GitSetupJourneys):
            def __init__(self, snapshot):
                self.current, self.keys = snapshot, []
            def snapshot(self):
                return self.current
            def key(self, code, modifiers):
                self.keys.append((code, modifiers))
                self.current = current
            def wait(self, description, predicate):
                if not predicate(self.current['document']):
                    raise AssertionError(description)
        visible = Harness(current)
        visible.show_git_setup_entry()
        self.assertEqual(visible.keys, [])
        absent = Harness(hidden)
        absent.show_git_setup_entry()
        self.assertEqual(absent.keys, [(5, 'cmd+alt')])
        self.assertTrue(git_setup_entry_current(absent.current))

    def snapshot(self):
        root = '/fixture/working-copy'
        state = dict(visible=True, busy=False, loaded=True, initialized=False,
                     author_set=True, origin_set=True, workspace=root, root=root,
                     branch=GIT_BRANCH, name_fingerprint=fingerprint(GIT_AUTHOR),
                     email_fingerprint=fingerprint(GIT_EMAIL), error=None,
                     viewport='child', rect=[100, 100, 700, 600])
        targets = {}
        for index, field in enumerate(('name', 'email', 'branch', 'origin')):
            targets['git.setup.' + field] = dict(x=400, y=180 + 50 * index,
                width=340, height=25, age_ms=0, viewport='child', enabled=True)
        for field, x in (('submit', 500), ('cancel', 250)):
            targets['git.setup.' + field] = dict(x=x, y=520, width=130, height=30,
                                                age_ms=0, viewport='child', enabled=True)
        return dict(document={'git_setup': state}, targets=targets)

    def test_setup_ready_requires_loaded_exact_workspace_and_author(self):
        snapshot = self.snapshot()
        root = Path('/fixture/working-copy')
        self.assertTrue(git_setup_ready(snapshot['document'], root, initialized=False,
                                       name=GIT_AUTHOR, email=GIT_EMAIL, origin=True))
        for change in ({'loaded': False}, {'busy': True}, {'workspace': '/fixture/other'},
                       {'root': '/fixture/parent'}, {'initialized': True},
                       {'name_fingerprint': fingerprint('Wrong author')},
                       {'email_fingerprint': fingerprint('other@example.invalid')},
                       {'origin_set': False}):
            with self.subTest(change=change):
                document = deepcopy(snapshot['document'])
                document['git_setup'].update(change)
                self.assertFalse(git_setup_ready(document, root, initialized=False,
                                                 name=GIT_AUTHOR, email=GIT_EMAIL, origin=True))

    def test_setup_journey_is_registered_without_changing_the_shared_all_fixture(self):
        path = Path(__file__).with_name('test-desktop-ui.py')
        spec = importlib.util.spec_from_file_location('git_setup_test_runner', path)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn('git_setup', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('git_setup'), ['git_setup'])
        self.assertNotIn('git_setup', runner.selected_journeys('all'))
        self.assertTrue(callable(getattr(runner.Journey, 'git_setup')))

    def test_missing_or_malformed_inspection_cannot_pass_as_ready(self):
        snapshot = self.snapshot()
        for field in ('visible', 'busy', 'loaded', 'initialized', 'author_set', 'origin_set',
                      'name_fingerprint', 'email_fingerprint'):
            with self.subTest(field=field):
                document = deepcopy(snapshot['document'])
                document['git_setup'].pop(field)
                with self.assertRaises(AssertionError):
                    git_setup_state(document)
        document = deepcopy(snapshot['document'])
        document['git_setup']['error'] = 'Configuration failed'
        with self.assertRaisesRegex(AssertionError, 'Configuration failed'):
            git_setup_ready(document, Path('/fixture/working-copy'), initialized=False)

    def test_closing_the_form_does_not_report_an_authorized_worker_as_cancelled(self):
        document = dict(git_setup=dict(visible=False, busy=True, loaded=False,
                        initialized=False, author_set=False, origin_set=False,
                        name_fingerprint=None, email_fingerprint=None))
        self.assertTrue(git_setup_state(document)['busy'])
        self.assertFalse(git_setup_ready(document, Path('/fixture/working-copy'),
                                        initialized=False))

    def test_single_page_geometry_rejects_stale_wrong_owner_missing_and_clipped_fields(self):
        snapshot = self.snapshot()
        self.assertEqual(assert_git_setup_geometry(snapshot, new_repository=True)['controls'], 6)
        for change in ({'viewport': 'parent'}, {'age_ms': 501}, {'x': 720},
                       {'height': 0}, {'width': float('nan')}):
            with self.subTest(change=change):
                altered = deepcopy(snapshot)
                altered['targets']['git.setup.origin'].update(change)
                with self.assertRaises(AssertionError):
                    assert_git_setup_geometry(altered, new_repository=True)
        altered = deepcopy(snapshot)
        altered['targets'].pop('git.setup.branch')
        with self.assertRaises(AssertionError):
            assert_git_setup_geometry(altered, new_repository=True)
        self.assertEqual(assert_git_setup_geometry(altered, new_repository=False)['controls'], 5)
        altered = deepcopy(snapshot)
        altered['targets']['git.setup.email']['y'] = altered['targets']['git.setup.name']['y']
        with self.assertRaisesRegex(AssertionError, 'overlap'):
            assert_git_setup_geometry(altered, new_repository=True)

    def test_fixture_isolates_git_configuration_and_creates_only_the_remote(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            paths = prepare_git_setup_fixture(root)
            environment = git_setup_environment(root, {
                'PATH': '/fixture/bin', 'GIT_DIR': '/real/repo/.git',
                'GIT_WORK_TREE': '/real/repo', 'GIT_CONFIG_GLOBAL': '/real/home/.gitconfig',
                'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'user.name',
                'GIT_CONFIG_VALUE_0': 'Real person', 'TIPTOPTYP_DESKTOP_TEST_DIR': str(root),
            })
            self.assertNotIn('GIT_DIR', environment)
            self.assertNotIn('GIT_WORK_TREE', environment)
            self.assertNotIn('GIT_CONFIG_COUNT', environment)
            self.assertNotIn('GIT_CONFIG_KEY_0', environment)
            self.assertNotIn('GIT_CONFIG_VALUE_0', environment)
            self.assertEqual(environment['GIT_CONFIG_GLOBAL'], str(paths['global_config']))
            self.assertEqual(environment['GIT_CONFIG_NOSYSTEM'], '1')
            self.assertEqual(environment['GIT_TERMINAL_PROMPT'], '0')
            self.assertEqual(environment['PATH'], '/fixture/bin')
            self.assertFalse((root / '.git').exists())
            self.assertFalse((paths['destination'] / '.git').exists())
            self.assertEqual(paths['global_config'].read_bytes(), b'')
            self.assertEqual(git_fixture_command(root, '--git-dir', paths['remote'],
                                                'rev-parse', '--is-bare-repository'), 'true')

    def test_managed_source_guard_requires_a_fresh_disabled_native_control(self):
        snapshot = dict(document={'viewport': 'owner'}, targets={
            'git.pull': dict(viewport='owner', age_ms=0, enabled=False)})
        assert_git_control_disabled(snapshot, 'git.pull')
        for change in ({'enabled': True}, {'enabled': None}, {'viewport': 'other'}, {'age_ms': 501}):
            with self.subTest(change=change):
                altered = deepcopy(snapshot)
                altered['targets']['git.pull'].update(change)
                with self.assertRaises(AssertionError):
                    assert_git_control_disabled(altered, 'git.pull')
        with self.assertRaises(AssertionError):
            assert_git_control_disabled(snapshot, 'git.revert')

    def create_local_backup(self, directory):
        root = directory / 'working-copy'
        root.mkdir()
        (root / 'main.txt').write_text(GIT_SOURCE, encoding='utf-8')
        (root / 'fixture.bin').write_bytes(b'fixture asset\x00\xff')
        (root / 'references.bib').write_text(GIT_NOTE, encoding='utf-8')
        (root / '.tiptoptyp').mkdir()
        (root / '.tiptoptyp/private.json').write_text('{}', encoding='utf-8')
        command = lambda *args: git_fixture_command(directory, '-C', root, *args)
        git_fixture_command(directory, 'init', '--quiet', '--initial-branch', GIT_BRANCH, root)
        command('config', '--local', 'user.name', GIT_AUTHOR)
        command('config', '--local', 'user.email', GIT_EMAIL)
        command('remote', 'add', 'origin', git_fixture_paths(directory)['remote'])
        with (root / '.git/info/exclude').open('a', encoding='utf-8') as exclude:
            exclude.write('\n.tiptoptyp/\n')
        command('add', '--', 'main.txt', 'fixture.bin', 'references.bib')
        command('commit', '--quiet', '-m', GIT_COMMIT)
        return root, command

    def test_real_backup_detector_requires_source_private_exclusion_and_explicit_upstream_push(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            prepare_git_setup_fixture(directory)
            root, command = self.create_local_backup(directory)
            self.assertEqual(assert_git_backup(directory, root, pushed=False),
                             {'main.txt', 'fixture.bin', 'references.bib'})
            command('push', '--quiet', '--set-upstream', 'origin', GIT_BRANCH)
            self.assertEqual(assert_git_backup(directory, root, pushed=True),
                             {'main.txt', 'fixture.bin', 'references.bib'})
            with self.assertRaisesRegex(AssertionError, 'before explicit Push'):
                assert_git_backup(directory, root, pushed=False)
            command('add', '--force', '--', '.tiptoptyp/private.json')
            command('commit', '--quiet', '-m', 'Forbidden private metadata')
            with self.assertRaisesRegex(AssertionError, 'private/runtime'):
                assert_git_backup(directory, root, pushed=True)


if __name__ == '__main__':
    unittest.main()
