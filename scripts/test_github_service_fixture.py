"""Protocol/isolation contracts for the synthetic GitHub provider, not real OAuth."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import time
import unittest
from urllib.error import HTTPError
from urllib.request import urlopen

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_git_journeys import prepare_git_setup_fixture, git_fixture_command
from github_service_fixture import (GitHubFixture, DEVICE_CODE, FIRST_ACCOUNT, SECOND_ACCOUNT,
                                    prepare_github_helper, github_fixture_environment)

STATUS = ['auth', 'status', '--active', '--hostname', 'github.com', '--json', 'hosts']
ACCOUNT = ['api', '--hostname', 'github.com', '--method', 'GET', 'user']
LOGIN = ['auth', 'login', '--web', '--hostname', 'github.com', '--git-protocol', 'https',
         '--skip-ssh-key', '--clipboard=false']


def create_arguments(name, *, private='true'):
    return ['api', '--hostname', 'github.com', '--method', 'POST', 'user/repos',
            '--raw-field', 'name=' + name, '--field', 'private=' + private,
            '--field', 'auto_init=false']


class GitHubServiceContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name).resolve()
        prepare_git_setup_fixture(self.directory)
        self.helper = prepare_github_helper(self.directory)
        self.fixture = GitHubFixture(self.directory)
        self.fixture.__enter__()
        self.addCleanup(self.fixture.__exit__, None, None, None)

    def command(self, arguments, *, check=True):
        return subprocess.run([str(self.helper), *arguments], capture_output=True,
                              text=True, timeout=5, check=check,
                              env=github_fixture_environment(self.directory))

    def login(self):
        process = subprocess.Popen([str(self.helper), *LOGIN], stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True,
                                   env=github_fixture_environment(self.directory))
        self.addCleanup(lambda: process.poll() is None and process.kill())
        deadline = time.monotonic() + 3
        while self.fixture.observation()['auth_starts'] != 1:
            if process.poll() is not None or time.monotonic() >= deadline:
                self.fail('Synthetic device authentication did not start')
            time.sleep(0.01)
        self.fixture.approve()
        stdout, stderr = process.communicate(timeout=3)
        self.assertEqual(process.returncode, 0, stderr)
        self.assertIn(DEVICE_CODE, stderr)
        self.assertIn(self.fixture.verification_uri, stderr)
        self.assertNotIn('token', stdout.lower())
        profile = self.directory / 'github-auth/hosts.yml'
        self.assertTrue(profile.is_file())
        self.assertNotIn('oauth_token', profile.read_text())

    def test_exact_cli_schema_uses_only_the_synthetic_active_public_identity(self):
        self.assertEqual(json.loads(self.command(STATUS).stdout), {'hosts':{}})
        self.assertNotEqual(self.command(ACCOUNT, check=False).returncode, 0)
        self.login()
        status = json.loads(self.command(STATUS).stdout)['hosts']['github.com']
        self.assertEqual(len(status), 1)
        self.assertEqual(status[0]['login'], FIRST_ACCOUNT['login'])
        self.assertTrue(status[0]['active'])
        self.assertNotIn('oauth_token', status[0])
        self.assertEqual(json.loads(self.command(ACCOUNT).stdout), FIRST_ACCOUNT)
        self.fixture.switch_account(SECOND_ACCOUNT)
        self.assertEqual(json.loads(self.command(ACCOUNT).stdout), SECOND_ACCOUNT)

    def test_only_private_empty_repositories_are_created_below_the_owned_fixture_root(self):
        self.login()
        repository = json.loads(self.command(create_arguments('private-paper')).stdout)
        remote = Path(self.fixture.observation()['repositories'][0]['fixture_remote'])
        self.assertTrue(repository['private'])
        self.assertEqual(repository['owner']['id'], FIRST_ACCOUNT['id'])
        self.assertEqual(repository['html_url'], 'https://github.com/ttt-fixture/private-paper')
        self.assertEqual(repository['clone_url'], 'https://github.com/ttt-fixture/private-paper.git')
        self.assertTrue(remote.is_relative_to(self.directory))
        self.assertEqual(git_fixture_command(self.directory, '--git-dir', remote,
                                            'rev-parse', '--is-bare-repository'), 'true')
        self.assertEqual(git_fixture_command(self.directory, '--git-dir', remote,
                                            'for-each-ref', '--format=%(refname)'), '')
        lookup = ['api', '--hostname', 'github.com', '--method', 'GET',
                  'repos/ttt-fixture/private-paper']
        self.assertEqual(json.loads(self.command(lookup).stdout), repository)
        self.assertEqual(self.fixture.observation()['recovery_reads'], 1)
        self.assertNotEqual(self.command(create_arguments('public-paper', private='false'),
                                        check=False).returncode, 0)
        self.assertNotEqual(self.command(create_arguments('../outside'), check=False).returncode, 0)
        self.assertNotEqual(self.command(create_arguments('private-paper'), check=False).returncode, 0)
        self.assertEqual(self.fixture.observation()['create_count'], 1)

    def test_canonical_origins_push_only_through_the_owned_file_transport_rewrite(self):
        self.login()
        repository = json.loads(self.command(create_arguments('private-push')).stdout)
        remote = Path(self.fixture.observation()['repositories'][0]['fixture_remote'])
        working = self.directory / 'source'
        git_fixture_command(self.directory, 'init', '--quiet', '--initial-branch', 'main', working)
        (working / 'main.txt').write_text('Synthetic private backup.\n')
        for arguments in (('config', 'user.name', 'Synthetic Author'),
                          ('config', 'user.email', 'synthetic@example.invalid'),
                          ('remote', 'add', 'origin', repository['clone_url']),
                          ('add', '--', 'main.txt'), ('commit', '--quiet', '-m', 'Private backup')):
            git_fixture_command(self.directory, '-C', working, *arguments)
        self.assertEqual(git_fixture_command(self.directory, '-C', working,
                                            'remote', 'get-url', 'origin'), str(remote))
        subprocess.run(['git', '-C', str(working), 'push', '--quiet', 'origin', 'HEAD:refs/heads/main'],
                       env=github_fixture_environment(self.directory), check=True,
                       capture_output=True, timeout=10)
        self.assertEqual(git_fixture_command(self.directory, '-C', working, 'rev-parse', 'HEAD'),
                         git_fixture_command(self.directory, '--git-dir', remote,
                                             'rev-parse', 'refs/heads/main'))

    def test_unconfirmed_creation_and_cancel_do_not_create_a_second_remote(self):
        self.login()
        self.fixture.pause_next_creation()
        process = subprocess.Popen([str(self.helper), *create_arguments('paused-paper')],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                   env=github_fixture_environment(self.directory))
        try:
            self.assertTrue(self.fixture.creation_started.wait(3))
            self.assertEqual(self.fixture.observation()['create_count'], 1)
            process.terminate()
            process.communicate(timeout=3)
            self.fixture.release_creation.set()
            self.assertEqual(self.fixture.observation()['create_count'], 1)
            remote = Path(self.fixture.observation()['repositories'][0]['fixture_remote'])
            self.assertEqual(git_fixture_command(self.directory, '--git-dir', remote,
                                                'for-each-ref', '--format=%(refname)'), '')
        finally:
            self.fixture.release_creation.set()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=3)

    def test_fixture_rejects_unowned_requests_and_never_uses_an_external_endpoint(self):
        with self.assertRaises(HTTPError) as failure:
            urlopen(self.fixture.origin + '/api/user', timeout=2)
        self.assertEqual(failure.exception.code, 403)
        failure.exception.close()
        config = self.directory / 'github-fixture.json'
        value = json.loads(config.read_text())
        value['origin'] = 'https://api.github.com'
        config.write_text(json.dumps(value))
        rejected = self.command(STATUS, check=False)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn('Unsafe synthetic provider endpoint', rejected.stderr)
        self.assertEqual(self.fixture.observation()['user_reads'], 0)

    def test_fixture_environment_never_inherits_real_authentication(self):
        environment = github_fixture_environment(self.directory, {
            'PATH':'/fixture/bin', 'GH_TOKEN':'real-token', 'GITHUB_TOKEN':'real-token',
            'GH_ENTERPRISE_TOKEN':'real-token', 'GITHUB_ENTERPRISE_TOKEN':'real-token',
            'GH_CONFIG_DIR':'/real/account', 'GIT_DIR':'/real/repository',
        })
        self.assertNotIn('GH_TOKEN', environment)
        self.assertNotIn('GITHUB_TOKEN', environment)
        self.assertNotIn('GH_ENTERPRISE_TOKEN', environment)
        self.assertNotIn('GITHUB_ENTERPRISE_TOKEN', environment)
        self.assertNotIn('GIT_DIR', environment)
        self.assertEqual(environment['GH_CONFIG_DIR'], str(self.directory / 'github-auth'))
        self.assertEqual(environment['GIT_ALLOW_PROTOCOL'], 'file')
        self.assertEqual(environment['PATH'], '/fixture/bin')
        self.assertTrue(self.helper.is_file())
        self.assertTrue(self.helper.stat().st_mode & 0o100)


if __name__ == '__main__':
    unittest.main()
