"""Settings documentation clicks must reach the real system browser."""
from pathlib import Path
import re
import time

SETTINGS_LINK_JOURNEYS = ('settings_links',)
LTEX_REFERENCE = 'https://ltex-plus.github.io/ltex-plus/settings.html'
BROWSER_BUNDLES = frozenset((
    'com.apple.Safari', 'com.apple.SafariTechnologyPreview',
    'com.brave.Browser', 'com.brave.Browser.beta', 'com.brave.Browser.nightly',
    'com.google.Chrome', 'com.google.Chrome.beta', 'com.google.Chrome.dev',
    'com.google.Chrome.canary', 'org.mozilla.firefox', 'org.mozilla.firefoxdeveloperedition',
    'org.mozilla.nightly', 'com.microsoft.edgemac', 'com.microsoft.edgemac.Beta',
    'com.microsoft.edgemac.Dev', 'com.microsoft.edgemac.Canary',
    'company.thebrowser.Browser', 'company.thebrowser.dia', 'com.kagi.kagimacOS',
    'com.kagi.kagimacOS.RC', 'com.operasoftware.Opera', 'com.vivaldi.Vivaldi',
    'net.imput.helium', 'app.zen-browser.zen',
))
SOURCE_FIELDS = ('path', 'source_fingerprint', 'source_bytes', 'revision', 'dirty',
                 'tabs', 'active_tab_id', 'preview_tab_id', 'preview_path')


def assert_document_preserved(document, before):
    if any(document[key] != before[key] for key in SOURCE_FIELDS):
        raise AssertionError('Opening documentation changed the document or its preview owner')


def documentation_click_completed(document, foreground, app_pid):
    """A launch receipt alone cannot prove the external browser actually opened."""
    pid, bundle = foreground.get('pid'), foreground.get('bundle')
    if type(pid) is not int or pid <= 0 or not isinstance(bundle, str) or not bundle:
        raise AssertionError('Native foreground application evidence is unavailable')
    if pid == app_pid:
        return False
    if bundle not in BROWSER_BUNDLES:
        raise AssertionError('Documentation click switched to an unrecognized browser or another app: '
                             + bundle)
    if 'completion_notice' not in document:
        raise AssertionError('Documentation launcher completion evidence is unavailable')
    return document['completion_notice'] == 'Opened ' + LTEX_REFERENCE + ' in the system browser'


def assert_preferences_preserved(before, after):
    if (not isinstance(before, str) or re.fullmatch(r'[0-9a-f]{16}', before) is None
            or before != after):
        raise AssertionError('Opening documentation changed the complete validated settings')


class SettingsLinksJourneys:
    def documentation_settings_fingerprint(self):
        if not self.snapshot()['document']['settings_json']['visible']:
            self.click('settings.json.mode')
        self.wait('Complete Settings JSON is available for comparison', lambda d:
                  d['settings_json']['visible'] and d['settings_json']['valid'])
        # Reload from the live settings rather than compare a parked draft.
        self.click('settings.json.reload')
        document = self.wait('Settings JSON reloads the complete live preferences', lambda d:
                             d['settings_json']['visible'] and d['settings_json']['valid'])['document']
        result = document['settings_json']['fingerprint']
        assert_preferences_preserved(result, result)
        self.click('settings.json.mode')
        self.wait('Settings form returns after comparison', lambda d:
                  not d['settings_json']['visible'])
        return result

    def settings_links(self):
        self.focus_document_window()
        before = self.snapshot()['document']
        path = Path(before['path']) if before['path'] is not None else None
        disk_source = path.read_bytes() if path is not None else None
        self.settings()
        preferences = self.documentation_settings_fingerprint()
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
        self.click('settings.ltex.setup')
        target = 'settings.documentation.Configuration reference'
        self.wait_target(target)
        current = self.snapshot()['document']
        assert_document_preserved(current, before)
        if current.get('completion_notice') == 'Opened ' + LTEX_REFERENCE + ' in the system browser':
            raise AssertionError('The documentation journey cannot reuse an earlier launcher receipt')
        windows = self.native('windows')['windows']
        title = next(window['AXTitle'] for window in windows
                     if 'Settings' in window['AXTitle'] and window.get('AXFocused'))
        if self.native('foreground')['pid'] != self.process.pid:
            raise AssertionError('The fixture must own focus immediately before its documentation click')
        self.click(target)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            # These are observations only. Do not use key/click input here: the
            # ordinary input guard would reactivate the app and hide a failure.
            foreground = self.native('foreground')
            state = self.snapshot()
            assert_document_preserved(state['document'], before)
            if documentation_click_completed(state['document'], foreground, self.process.pid):
                self.record('settings.documentation.browser_opened', target=target,
                            url=LTEX_REFERENCE, foreground=foreground, launcher_completed=True)
                break
            time.sleep(.05)
        else:
            raise AssertionError('The Settings documentation click did not complete and activate the browser')
        # The browser transition above is intentional; explicitly restore the
        # exact Settings window before posting any further native input.
        self.native('activate')
        self.native('raise', title)
        self.native_wait('Settings regains focus after its documentation opens', lambda windows:
                         any(window['AXTitle'] == title and window.get('AXFocused') for window in windows))
        self.wait('Settings stays open after its documentation link', lambda d: d['settings_visible'])
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.close_settings()
        after = self.snapshot()['document']
        assert_document_preserved(after, before)
        if path is not None and path.read_bytes() != disk_source:
            raise AssertionError('Opening Settings documentation changed the source file on disk')
        self.record('settings.documentation.complete', preferences_unchanged=True, source_unchanged=True)
