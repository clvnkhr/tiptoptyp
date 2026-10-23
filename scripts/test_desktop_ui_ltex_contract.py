"""Contract guards for bundled/custom LTeX journeys; these are not native evidence."""
from copy import deepcopy
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from desktop_ui_journeys import fingerprint
from desktop_ui_ltex_journeys import (
    assert_ltex_repair_busy, assert_ltex_repair_preserved, assert_ltex_source_unchanged,
    assert_ltex_stale, ltex_environment, ltex_program, ltex_repair_completed, ltex_results,
    LtexJourneys,
)


class LtexContract(unittest.TestCase):
    def document(self, path, source, *, mode='Bundled', current=True):
        return dict(path=str(path), source_fingerprint=fingerprint(source), ltex=dict(
            state='Up to date / idle', mode=mode, enabled=True, current=current, count=1,
            suggestions=1, rules=['AGREEMENT'], locations=[[2, 6]], paths=[str(path)]))

    def test_results_require_suggestions_source_identity_and_real_paths(self):
        source = 'This are a result.\n'
        path = Path('/tmp/ltex-fixture.tex')
        state = self.document(path, source)
        self.assertTrue(ltex_results(state, path, source))
        for key, value in [('current', False), ('suggestions', 0), ('rules', []),
                           ('mode', 'Custom'), ('paths', ['/tmp/.tiptoptyp/projection.tex'])]:
            changed = deepcopy(state)
            changed['ltex'][key] = value
            self.assertFalse(ltex_results(changed, path, source), key)
        self.assertFalse(ltex_results(state, path, source + ' '))
        state['ltex']['state'] = 'Failed'
        with self.assertRaises(AssertionError):
            ltex_results(state, path, source)

    def test_explicit_custom_mode_cannot_pass_as_a_bundled_result(self):
        source = 'This are a result.\n'
        path = Path('/tmp/ltex-fixture.typ')
        state = self.document(path, source, mode='Custom')
        self.assertFalse(ltex_results(state, path, source))
        self.assertTrue(ltex_results(state, path, source, 'Custom'))
        state['ltex']['mode'] = 'Bundled'
        self.assertFalse(ltex_results(state, path, source, 'Custom'))

    def test_default_requires_no_external_launcher_or_path_discovery(self):
        with patch('shutil.which', side_effect=AssertionError('PATH lookup is not a bundled check')):
            with patch.dict(os.environ, {'PATH': '/not-installed'}):
                self.assertIsNone(ltex_program())

    def test_missing_or_nonexecutable_explicit_server_is_unavailable(self):
        with tempfile.TemporaryDirectory() as directory:
            program = Path(directory) / 'ltex-ls-plus'
            with self.assertRaises(ValueError):
                ltex_program(program)
            program.write_text('#!/bin/sh\n')
            with self.assertRaises(ValueError):
                ltex_program(program)
            program.chmod(0o700)
            self.assertEqual(ltex_program(program), program.resolve())

    def test_bundled_launch_masks_external_java_without_mutating_parent_environment(self):
        before = dict(PATH='/custom/bin', JAVA_HOME='/custom/java', JAVACMD='/custom/java/bin/java',
                      TIPTOPTYP_DESKTOP_TEST_DIR='/tmp/journey', OTHER='keep')
        result = ltex_environment(before, Path('/tmp/journey'))
        self.assertEqual(result['PATH'], '/usr/bin:/bin')
        self.assertEqual(result['JAVA_HOME'], '/tmp/journey/unavailable-system-java')
        self.assertEqual(result['JAVACMD'], '/tmp/journey/unavailable-system-java/bin/java')
        self.assertEqual(result['OTHER'], 'keep')
        self.assertEqual(result['TIPTOPTYP_DESKTOP_TEST_DIR'], before['TIPTOPTYP_DESKTOP_TEST_DIR'])
        self.assertEqual(before['PATH'], '/custom/bin')
        self.assertEqual(before['JAVA_HOME'], '/custom/java')

    def test_explicit_custom_launch_preserves_its_java_environment(self):
        before = dict(PATH='/custom/bin', JAVA_HOME='/custom/java', JAVACMD='/custom/java/bin/java')
        self.assertEqual(ltex_environment(before, Path('/tmp/journey'), Path('/custom/ltex')), before)

    def test_stale_result_requires_the_edited_original_source_and_identical_locations(self):
        path = Path('/tmp/ltex-fixture.tex')
        source, corrected = 'This are a result.\n', 'This is a result.\n'
        before = self.document(path, source)['ltex']
        stale = self.document(path, corrected, current=False)
        assert_ltex_stale(stale, before, path, corrected, 'Bundled')
        for key, value in [('current', True), ('count', 0), ('locations', [[1, 1]]),
                           ('mode', 'Custom'), ('enabled', False), ('state', 'Failed')]:
            changed = deepcopy(stale)
            changed['ltex'][key] = value
            with self.assertRaises(AssertionError, msg=key):
                assert_ltex_stale(changed, before, path, corrected, 'Bundled')
        for key, value in [('path', '/tmp/.tiptoptyp/projection.tex'),
                           ('source_fingerprint', fingerprint(source))]:
            changed = deepcopy(stale)
            changed[key] = value
            with self.assertRaises(AssertionError, msg=key):
                assert_ltex_stale(changed, before, path, corrected, 'Bundled')

    def test_unsaved_correction_cannot_write_the_original_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'ltex-fixture.typ'
            source = 'This are a result.\n'
            path.write_text(source, encoding='utf-8')
            assert_ltex_source_unchanged(path, source)
            path.write_text('This is a result.\n', encoding='utf-8')
            with self.assertRaises(AssertionError):
                assert_ltex_source_unchanged(path, source)

    def test_redownload_requires_real_success_and_fresh_busy_controls(self):
        document = dict(resource_repair=dict(busy=True, status='Downloading LTeX+…'))
        snapshot = dict(document=document, targets={
            'settings.ltex.redownload': dict(enabled=False, age_ms=0),
            'settings.ltex.cancel_redownload': dict(enabled=True, age_ms=0),
        })
        assert_ltex_repair_busy(snapshot)
        self.assertFalse(ltex_repair_completed(document))
        for name, field, value in [('settings.ltex.redownload', 'enabled', True),
                                   ('settings.ltex.cancel_redownload', 'enabled', False),
                                   ('settings.ltex.cancel_redownload', 'age_ms', 501)]:
            changed = deepcopy(snapshot)
            changed['targets'][name][field] = value
            with self.assertRaises(AssertionError):
                assert_ltex_repair_busy(changed)
        document['resource_repair'] = dict(busy=False, status='LTeX+ downloaded')
        self.assertTrue(ltex_repair_completed(document))
        for status in (None, 'Resource task cancelled', 'Checksum mismatch'):
            document['resource_repair']['status'] = status
            with self.assertRaises(AssertionError):
                ltex_repair_completed(document)

    def test_redownload_must_not_change_preferences_mode_or_source(self):
        before = dict(path='/tmp/a.tex', source_fingerprint='source', source_bytes=20,
                      ltex=dict(mode='Bundled', enabled=False))
        assert_ltex_repair_preserved(deepcopy(before), before, b'preferences', b'preferences')
        for key, value in [('path', '/tmp/b.tex'), ('source_fingerprint', 'changed'), ('source_bytes', 21)]:
            changed = deepcopy(before)
            changed[key] = value
            with self.assertRaises(AssertionError):
                assert_ltex_repair_preserved(changed, before, b'preferences', b'preferences')
        for key, value in [('mode', 'Custom'), ('enabled', True)]:
            changed = deepcopy(before)
            changed['ltex'][key] = value
            with self.assertRaises(AssertionError):
                assert_ltex_repair_preserved(changed, before, b'preferences', b'preferences')
        with self.assertRaises(AssertionError):
            assert_ltex_repair_preserved(before, before, b'preferences', b'changed preferences')

    def test_settings_capture_retains_its_fresh_original_path_and_copied_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / 'fixture'
            evidence = Path(temporary) / 'evidence'
            capture_dir = directory / '.tiptoptyp/screenshots'
            capture_dir.mkdir(parents=True)
            evidence.mkdir()
            (capture_dir / 'old-settings.png').write_bytes(b'stale')

            class Capturer(LtexJourneys):
                def capture_viewport(self, target):
                    fresh = capture_dir / ('fresh-' + target + '.png')
                    fresh.write_bytes(b'fresh pixels')
                    (self.evidence / fresh.name).write_bytes(fresh.read_bytes())
                def record(self, action, **detail):
                    self.observation = (action, detail)

            journey = Capturer()
            journey.directory, journey.evidence = directory, evidence
            journey.capture_ltex_settings('expanded')
            action, observed = journey.observation
            self.assertEqual(action, 'ltex.settings.framebuffer')
            self.assertEqual(observed['phase'], 'expanded')
            self.assertEqual(Path(observed['original']), capture_dir / 'fresh-settings.png')
            self.assertEqual(Path(observed['retained']).relative_to(evidence),
                             Path('generated-captures/.tiptoptyp/screenshots/fresh-settings.png'))
            for key in ('original', 'retained', 'evidence'):
                self.assertEqual(Path(observed[key]).read_bytes(), b'fresh pixels')
            self.assertFalse((evidence / 'generated-captures/.tiptoptyp/screenshots/old-settings.png').exists())

    def test_focused_resource_check_remains_excluded_from_all(self):
        file = Path(__file__).with_name('test-desktop-ui.py')
        spec = importlib.util.spec_from_file_location('desktop_ltex_contract_runner', file)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertNotIn('ltex', runner.selected_journeys('all'))
        self.assertEqual(runner.selected_journeys('ltex'), ['ltex'])


if __name__ == '__main__':
    unittest.main()
