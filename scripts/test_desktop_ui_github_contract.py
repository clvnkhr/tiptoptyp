"""Native GitHub journey failure detectors; service approval is synthetic."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_git_journeys import (prepare_git_setup_fixture, git_fixture_command,
                                     assert_git_setup_geometry)
from desktop_ui_github_journeys import (github_state, github_account_ready, github_no_remote,
                                       github_departed_outcome, GitHubJourneys)
from github_service_fixture import FIRST_ACCOUNT


class GitHubJourneyContracts(unittest.TestCase):
    def document(self):
        return dict(git_setup=dict(visible=True, busy=False, loaded=True, initialized=False,
            author_set=True, origin_set=False, name_fingerprint=fingerprint('Fixture author'),
            email_fingerprint=fingerprint('fixture@example.invalid'), github=dict(
                selected=True, signed_in=True, private=True, code_ready=False, busy=False,
                open_requested=False, attach_failed=False, uncertain=False, stage='ready',
                account_id=FIRST_ACCOUNT['id'], account_login=FIRST_ACCOUNT['login'],
                name_fingerprint=fingerprint('private-paper'), opened_url_fingerprint=None,
                created_url=None, expected_url=None)))

    def test_account_ready_requires_exact_public_identity_and_no_inflight_job(self):
        document = self.document()
        self.assertTrue(github_account_ready(document, FIRST_ACCOUNT))
        for change in ({'account_id':124}, {'account_login':'other-fixture'}, {'busy':True},
                       {'signed_in':False}, {'selected':False}):
            with self.subTest(change=change):
                altered = deepcopy(document)
                altered['git_setup']['github'].update(change)
                self.assertFalse(github_account_ready(altered, FIRST_ACCOUNT))

    def test_saved_account_reuse_requires_explicit_sign_in_after_reopening(self):
        ready = self.document()
        unopened = deepcopy(ready)
        unopened['git_setup']['github'].update(signed_in=False, account_id=None, account_login=None)
        class Harness(GitHubJourneys):
            def __init__(self, document):
                self.document, self.clicks = document, []
            def snapshot(self):
                return {'document':self.document}
            def click(self, name):
                self.clicks.append(name)
                self.document = ready
            def wait(self, description, predicate, timeout):
                if not predicate(self.document):
                    raise AssertionError(description)
        fresh_form = Harness(unopened)
        fresh_form.github_use_saved_account(FIRST_ACCOUNT)
        self.assertEqual(fresh_form.clicks, ['github.sign_in'])
        already_verified = Harness(ready)
        already_verified.github_use_saved_account(FIRST_ACCOUNT)
        self.assertEqual(already_verified.clicks, [])

    def test_missing_or_malformed_provider_evidence_is_never_an_authentication_pass(self):
        document = self.document()
        for field in ('selected', 'signed_in', 'private', 'code_ready', 'busy', 'open_requested',
                      'attach_failed', 'uncertain', 'stage', 'account_id', 'account_login', 'name_fingerprint'):
            with self.subTest(field=field):
                altered = deepcopy(document)
                altered['git_setup']['github'].pop(field)
                with self.assertRaises(AssertionError):
                    github_state(altered)
        for change in ({'account_id':True}, {'account_id':0}, {'account_login':'https://github.com'},
                       {'name_fingerprint':'raw repository input'}, {'opened_url_fingerprint':'raw URL'},
                       {'open_requested':True}):
            with self.subTest(change=change):
                altered = deepcopy(document)
                altered['git_setup']['github'].update(change)
                with self.assertRaises(AssertionError):
                    github_state(altered)

    def test_github_fields_and_footer_use_the_same_native_child_geometry_contract(self):
        document = self.document()
        document['git_setup'].update(viewport='github-child', rect=[100,100,700,650])
        targets = {}
        for index, name in enumerate(('git.setup.name', 'git.setup.email', 'git.setup.branch',
                                      'github.name', 'github.private')):
            targets[name] = dict(x=400, y=170 + index * 65, width=320, height=24,
                                 viewport='github-child', age_ms=0)
        for name, x in (('git.setup.submit',500), ('git.setup.cancel',250)):
            targets[name] = dict(x=x, y=580, width=150, height=28,
                                 viewport='github-child', age_ms=0)
        snapshot = dict(document=document, targets=targets)
        self.assertEqual(assert_git_setup_geometry(snapshot, new_repository=True, github=True)['controls'], 7)
        altered = deepcopy(snapshot)
        altered['targets']['github.name']['y'] = 680
        with self.assertRaisesRegex(AssertionError, 'clipped'):
            assert_git_setup_geometry(altered, new_repository=True, github=True)

    def test_cancelled_creation_detector_rejects_origin_or_credential_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            paths = prepare_git_setup_fixture(directory)
            root = directory / 'working-copy'
            self.assertTrue(github_no_remote(directory, root))
            git_fixture_command(directory, 'init', '--quiet', root)
            self.assertTrue(github_no_remote(directory, root))
            git_fixture_command(directory, '-C', root, 'remote', 'add', 'origin', paths['remote'])
            with self.assertRaisesRegex(AssertionError, 'attached a remote'):
                github_no_remote(directory, root)
            git_fixture_command(directory, '-C', root, 'remote', 'remove', 'origin')
            git_fixture_command(directory, '-C', root, 'config', '--local', 'credential.helper', 'fixture-helper')
            with self.assertRaisesRegex(AssertionError, 'credential helper'):
                github_no_remote(directory, root)

    def test_late_creation_outcome_keeps_the_original_workspace_and_confirmation_state(self):
        root = Path('/fixture/old-working-copy')
        address = 'https://github.com/ttt-fixture/old-paper'
        document = self.document()
        document['git_setup']['last_github_outcome'] = dict(
            workspace=str(root), confirmed=False, attach_failed=True,
            created_url=None, expected_url=address)
        self.assertTrue(github_departed_outcome(document, root, address))
        self.assertFalse(github_departed_outcome(document, Path('/fixture/new-working-copy'), address))
        for change in ({'confirmed':True}, {'attach_failed':False}, {'expected_url':None},
                       {'workspace':'/fixture/new-working-copy'}):
            with self.subTest(change=change):
                altered = deepcopy(document)
                altered['git_setup']['last_github_outcome'].update(change)
                self.assertFalse(github_departed_outcome(altered, root, address))

    def test_github_journey_is_explicit_and_not_a_real_account_prerequisite_of_all(self):
        path = Path(__file__).with_name('test-desktop-ui.py')
        spec = importlib.util.spec_from_file_location('github_test_runner', path)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn('git_github', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('git_github'), ['git_github'])
        self.assertNotIn('git_github', runner.selected_journeys('all'))
        self.assertTrue(callable(getattr(runner.Journey, 'git_github')))


if __name__ == '__main__':
    unittest.main()
