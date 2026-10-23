"""Fast guards for documentation evidence; these do not claim native coverage."""
from copy import deepcopy
import unittest

from desktop_ui_settings_links_journeys import (
    BROWSER_BUNDLES, LTEX_REFERENCE, SOURCE_FIELDS, assert_document_preserved,
    assert_preferences_preserved, documentation_browser_activated, documentation_click_completed,
)


class SettingsDocumentationContract(unittest.TestCase):
    def document(self):
        return dict(path='/tmp/journey.txt', source_fingerprint='0123456789abcdef',
                    source_bytes=25, revision='revision 1', dirty=False, tabs=1,
                    active_tab_id=1, preview_tab_id=1, preview_path='/tmp/journey.txt',
                    completion_notice='Opened ' + LTEX_REFERENCE + ' in the system browser')

    def test_exact_launcher_notice_and_actual_browser_activation_are_both_required(self):
        document = self.document()
        for bundle in BROWSER_BUNDLES:
            self.assertTrue(documentation_click_completed(document, dict(pid=200, bundle=bundle), 100))
        self.assertFalse(documentation_click_completed(document, dict(pid=100, bundle='dev.tiptoptyp.journey'), 100))
        for notice in (None, 'Opening ' + LTEX_REFERENCE,
                       'Opened https://example.com in the system browser',
                       'Could not open ' + LTEX_REFERENCE):
            document['completion_notice'] = notice
            self.assertFalse(documentation_click_completed(document, dict(pid=200, bundle='com.brave.Browser'), 100))

    def test_browser_activation_is_observable_without_an_occluded_app_frame(self):
        browser = dict(pid=200, bundle='com.brave.Browser')
        self.assertTrue(documentation_browser_activated(browser, 100))
        self.assertFalse(documentation_browser_activated(dict(pid=100, bundle='dev.tiptoptyp.journey'), 100))
        document = self.document()
        document['completion_notice'] = None
        self.assertFalse(documentation_click_completed(document, browser, 100),
                         'Browser activation alone cannot prove a completed launch')
        for foreground in ({}, dict(pid=200, bundle='com.apple.finder')):
            with self.subTest(foreground=foreground), self.assertRaises(AssertionError):
                documentation_browser_activated(foreground, 100)

    def test_unrelated_foreground_apps_and_missing_native_evidence_cannot_pass(self):
        for foreground in (dict(pid=200, bundle='com.apple.finder'),
                           dict(pid=200, bundle='com.openai.codex'),
                           dict(pid=200, bundle=''), dict(pid=0, bundle='com.apple.Safari'),
                           dict(pid=True, bundle='com.apple.Safari'), {}):
            with self.subTest(foreground=foreground), self.assertRaises(AssertionError):
                documentation_click_completed(self.document(), foreground, 100)

    def test_browser_without_launcher_observation_cannot_pass(self):
        document = self.document()
        del document['completion_notice']
        with self.assertRaises(AssertionError):
            documentation_click_completed(document, dict(pid=200, bundle='com.apple.Safari'), 100)
        # The app flattens inspection fields; a fabricated nested receipt is
        # not evidence that the running launcher completed.
        document['interaction'] = {'completion_notice': 'Opened ' + LTEX_REFERENCE + ' in the system browser'}
        with self.assertRaises(AssertionError):
            documentation_click_completed(document, dict(pid=200, bundle='com.apple.Safari'), 100)

    def test_source_and_preview_ownership_must_stay_exact(self):
        before = self.document()
        assert_document_preserved(deepcopy(before), before)
        for field in SOURCE_FIELDS:
            after = deepcopy(before)
            after[field] = 'changed'
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_document_preserved(after, before)

    def test_live_preference_comparison_rejects_changes_and_missing_fingerprints(self):
        assert_preferences_preserved('0123456789abcdef', '0123456789abcdef')
        for before, after in ((None, None), ('', ''), ('invalid', 'invalid'),
                              ('0123456789abcdef', 'fedcba9876543210')):
            with self.subTest(before=before, after=after), self.assertRaises(AssertionError):
                assert_preferences_preserved(before, after)

    def test_focused_journey_is_registered_without_opening_browser_in_all(self):
        import importlib.util
        from pathlib import Path
        spec = importlib.util.spec_from_file_location('settings_links_runner', Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn('settings_links', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('settings_links'), ['settings_links'])
        self.assertNotIn('settings_links', runner.selected_journeys('all'))
        self.assertTrue(callable(runner.Journey.settings_links))


if __name__ == '__main__':
    unittest.main()
