"""Offline references and reversible presets through the real Settings UI."""
from copy import deepcopy
from pathlib import Path

from desktop_ui_settings_links_journeys import (
    assert_document_preserved, assert_preferences_preserved,
)

TOOL_DEFAULTS_JOURNEYS = ('tool_defaults',)
TOOL_IDS = frozenset(('typst', 'tinymist', 'tectonic', 'texlab', 'badness', 'tex-fmt',
                      'ltex', 'harper', 'liquid', 'tex-engine', 'latexmk', 'bibtex',
                      'biber', 'latexdiff', 'texdoc'))
PRESERVED_FIELDS = ('writing_language', 'harper_enabled', 'harper_dictionary_fingerprint',
                    'harper_ignore_capitalized_names')


def tool_profiles(document):
    profiles = document.get('tool_profiles')
    if (not isinstance(profiles, dict) or set(profiles) != TOOL_IDS
            or any(value not in ('Normal', 'Verbose', None) for value in profiles.values())):
        raise AssertionError('Complete current tool-preset observations are unavailable')
    return profiles


def assert_tool_preferences(document, before, changed=None, profile=None):
    expected = dict(tool_profiles(before))
    if changed is not None:
        expected[changed] = profile
    if tool_profiles(document) != expected:
        raise AssertionError('A preset changed another tool or failed to apply its own fields')
    if any(document[field] != before[field] for field in PRESERVED_FIELDS):
        raise AssertionError('A preset changed grammar activation, language, names or dictionary')
    if any(document['ltex'][field] != before['ltex'][field] for field in ('enabled', 'mode')):
        raise AssertionError('A preset changed LTeX activation or executable selection')


def assert_local_reference(document, before, foreground, app_pid, opened):
    assert_document_preserved(document, before)
    if ('tool_reference' not in document or not document['settings_visible']
            or document['tool_reference'] != opened):
        raise AssertionError('The offline reference escaped its Settings owner')
    if type(foreground.get('pid')) is not int or foreground['pid'] != app_pid:
        raise AssertionError('An offline reference activated another application')
    notice = document['completion_notice']
    if (isinstance(notice, str) and notice.startswith(('Opened https://', 'Opened http://'))
            and notice != before['completion_notice']):
        raise AssertionError('An offline reference launched browser documentation')


class ToolDefaultsJourneys:
    def tool_defaults(self):
        self.focus_document_window()
        before = deepcopy(self.snapshot()['document'])
        profiles = tool_profiles(before)
        if any(profiles[provider] != 'Normal' for provider in ('ltex', 'harper')):
            raise AssertionError('The reversible preset journey requires its fresh Normal settings')
        path = Path(before['path']) if before['path'] is not None else None
        disk_source = path.read_bytes() if path is not None else None
        self.settings()
        preferences = self.documentation_settings_fingerprint()
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
        self.click('settings.reference.ltex')
        document = self.wait('LTeX offline reference opens in Settings', lambda d:
                             d['tool_reference'] == 'ltex' and d['settings_visible'])['document']
        assert_local_reference(document, before, self.native('foreground'), self.process.pid, 'ltex')
        assert_tool_preferences(document, before)
        query, find_visible = document['settings_query_fingerprint'], document['find_visible']
        if self.capture_review:
            self.capture_viewport('settings')
        self.key(3)  # Cmd+F cannot search the form or document behind the modal.
        self.stable_state(lambda d: d['tool_reference'] == 'ltex' and d['settings_visible']
                          and d['settings_query_fingerprint'] == query
                          and d['find_visible'] == find_visible, frames=5)
        self.key(13)  # Cmd+W closes only the reference, retaining Settings.
        document = self.wait('Cmd+W closes only the local reference', lambda d:
                             d['tool_reference'] is None and d['settings_visible'])['document']
        assert_local_reference(document, before, self.native('foreground'), self.process.pid, None)
        assert_tool_preferences(document, before)
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.record('settings.reference.offline', tool='ltex', settings_retained=True,
                    browser_activated=False, source_unchanged=True)

        for provider, query, title in (
                ('ltex', 'LTeX', 'LTeX+ spelling and grammar'),
                ('harper', 'Harper', 'Harper spelling and grammar')):
            self.setting_section(query, title)
            self.click(f'settings.profile.{provider}.Verbose')
            document = self.wait(provider + ' Verbose preset applies through Settings', lambda d:
                                 d['tool_profiles'][provider] == 'Verbose')['document']
            assert_document_preserved(document, before)
            assert_tool_preferences(document, before, provider, 'Verbose')
            if self.capture_review:
                self.capture_viewport('settings')
            self.click(f'settings.profile.{provider}.Normal')
            document = self.wait(provider + ' Normal preset restores its fields', lambda d:
                                 d['tool_profiles'][provider] == 'Normal')['document']
            assert_document_preserved(document, before)
            assert_tool_preferences(document, before)
            assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
            self.record('settings.profile.round_trip', tool=provider, activation_unchanged=True,
                        complete_preferences_restored=True, source_unchanged=True)

        self.close_settings()
        initial_panel = self.snapshot()['document']['panel']
        if initial_panel is None:
            self.key(23)
        self.click('panel.problems')
        self.key(3)
        self.wait_target('problems.search', focused=True)
        baseline_filters = deepcopy(self.snapshot()['document']['problems'])
        verbose_tools = ('tinymist', 'tectonic', 'tex-engine', 'texlab', 'badness', 'ltex', 'harper')
        for profile in ('Verbose', 'Normal'):
            self.click('problems.diagnostic_detail')
            self.click('problems.diagnostic_detail.' + profile)
            document = self.wait('Problems applies ' + profile + ' diagnostics together', lambda d:
                                 d['diagnostic_detail'] == profile)['document']
            expected = dict(tool_profiles(before))
            expected.update(dict.fromkeys(verbose_tools, profile))
            if tool_profiles(document) != expected:
                raise AssertionError('Problems detail failed to apply every diagnostic provider')
            for field in PRESERVED_FIELDS:
                if document[field] != before[field]:
                    raise AssertionError('Problems detail changed grammar activation or preferences')
            if any(document['ltex'][field] != before['ltex'][field] for field in ('enabled', 'mode')):
                raise AssertionError('Problems detail changed LTeX activation or executable selection')
            if any(document['problems'][field] != baseline_filters[field]
                   for field in ('query', 'source', 'type', 'category', 'grouped')):
                raise AssertionError('Problems detail changed the current browsing filters')
            assert_document_preserved(document, before)
            if self.capture_review:
                self.capture_viewport('main')
        self.settings()
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.close_settings()
        if initial_panel is None:
            self.key(23)
            self.wait('Originally hidden bottom panel returns', lambda d: d['panel'] is None)
        self.record('problems.diagnostic_detail.round_trip', complete_preferences_restored=True,
                    source_unchanged=True, filters_unchanged=True)
        document = self.snapshot()['document']
        assert_document_preserved(document, before)
        assert_tool_preferences(document, before)
        if path is not None and path.read_bytes() != disk_source:
            raise AssertionError('Inspecting references or presets changed the source on disk')
        self.record('settings.tool_defaults.complete', complete_preferences_restored=True,
                    source_unchanged=True, no_model_required=True)
