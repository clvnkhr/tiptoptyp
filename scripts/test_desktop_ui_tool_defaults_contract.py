"""Evidence guards for tool-default journeys; these are not native UI results."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import unittest

from desktop_ui_settings_links_journeys import SOURCE_FIELDS
from desktop_ui_tool_defaults_journeys import (
    PRESERVED_FIELDS, TOOL_IDS, assert_local_reference, assert_tool_preferences,
    tool_profiles,
)


def document():
    return dict(path='/tmp/journey.txt', source_fingerprint='0123456789abcdef',
                source_bytes=25, revision='revision 1', dirty=False, tabs=1,
                active_tab_id=1, preview_tab_id=1, preview_path='/tmp/journey.txt',
                completion_notice=None, settings_visible=True, tool_reference='ltex',
                writing_language='Auto', harper_enabled=False,
                harper_dictionary_fingerprint='fedcba9876543210',
                harper_ignore_capitalized_names=True, ltex={'enabled': False, 'mode': 'Custom'},
                tool_profiles=dict.fromkeys(TOOL_IDS, 'Normal'))


class ToolDefaultsContract(unittest.TestCase):
    def test_missing_partial_extra_and_invalid_profile_observations_are_rejected(self):
        self.assertEqual(set(tool_profiles(document())), TOOL_IDS)
        for invalid in (None, {}, {'harper': 'Normal'},
                        dict.fromkeys(TOOL_IDS | {'legacy'}, 'Normal'),
                        dict.fromkeys(TOOL_IDS, 'Unknown')):
            changed = document()
            changed['tool_profiles'] = invalid
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                tool_profiles(changed)

    def test_only_the_requested_tool_may_switch_profile(self):
        before = document()
        assert_tool_preferences(deepcopy(before), before)
        for provider in ('harper', 'ltex'):
            changed = deepcopy(before)
            changed['tool_profiles'][provider] = 'Verbose'
            assert_tool_preferences(changed, before, provider, 'Verbose')
            with self.assertRaises(AssertionError):
                assert_tool_preferences(changed, before)
            changed['tool_profiles']['tinymist'] = 'Verbose'
            with self.assertRaises(AssertionError):
                assert_tool_preferences(changed, before, provider, 'Verbose')

    def test_disabled_providers_custom_executables_language_and_dictionary_stay_independent(self):
        before = document()
        for field in PRESERVED_FIELDS:
            changed = deepcopy(before)
            changed[field] = 'unexpected'
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_tool_preferences(changed, before)
        for field in ('enabled', 'mode'):
            changed = deepcopy(before)
            changed['ltex'][field] = 'unexpected'
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_tool_preferences(changed, before)

    def test_local_reference_requires_its_owner_exact_modal_and_native_focus(self):
        before = document()
        assert_local_reference(deepcopy(before), before, {'pid': 100}, 100, 'ltex')
        closed = deepcopy(before)
        closed['tool_reference'] = None
        assert_local_reference(closed, before, {'pid': 100}, 100, None)
        for foreground in ({}, {'pid': True}, {'pid': 200}, {'pid': 0}):
            with self.subTest(foreground=foreground), self.assertRaises(AssertionError):
                assert_local_reference(deepcopy(before), before, foreground, 100, 'ltex')
        for field, value in (('settings_visible', False), ('tool_reference', None),
                             ('tool_reference', 'harper')):
            changed = deepcopy(before)
            changed[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                assert_local_reference(changed, before, {'pid': 100}, 100, 'ltex')
        del closed['tool_reference']
        with self.assertRaises(AssertionError):
            assert_local_reference(closed, before, {'pid': 100}, 100, None)

    def test_browser_launch_and_source_mutations_cannot_pass_as_local_reference(self):
        before = document()
        for notice in ('Opened https://example.com in the system browser',
                       'Opened http://example.com in the system browser'):
            changed = deepcopy(before)
            changed['completion_notice'] = notice
            with self.subTest(notice=notice), self.assertRaises(AssertionError):
                assert_local_reference(changed, before, {'pid': 100}, 100, 'ltex')
        for field in SOURCE_FIELDS:
            changed = deepcopy(before)
            changed[field] = 'changed'
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_local_reference(changed, before, {'pid': 100}, 100, 'ltex')

    def test_offline_journey_is_registered_once_in_all_and_needs_no_optional_model(self):
        spec = importlib.util.spec_from_file_location('tool_defaults_runner',
                                                      Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn('tool_defaults', runner.JOURNEY_NAMES)
        self.assertEqual(runner.selected_journeys('tool_defaults'), ['tool_defaults'])
        self.assertEqual(runner.selected_journeys('all').count('tool_defaults'), 1)
        self.assertTrue(callable(runner.Journey.tool_defaults))
        self.assertNotIn('tool_defaults', runner.LIQUID_JOURNEYS)
        self.assertNotIn('tool_defaults', runner.LTEX_JOURNEYS)


if __name__ == '__main__':
    unittest.main()
