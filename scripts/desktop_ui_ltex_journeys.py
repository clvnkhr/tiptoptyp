"""Real bundled LTeX+ checks through native editor interaction."""
import os
from pathlib import Path
import shutil

LTEX_JOURNEYS = ('ltex',)


def ltex_program(value=None):
    """An explicit override is optional; default checks use the app's bundle."""
    if value is None:
        return None
    candidate = Path(value)
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise ValueError('--ltex-executable must be a real executable LTeX+ launcher; an absent custom server is an unavailable check, not a pass')
    return candidate.resolve()


def ltex_environment(environment, directory, executable=None):
    """Make the bundled journey independent of installed Java and LTeX launchers."""
    environment = dict(environment)
    if executable is None:
        absent_java = Path(directory) / 'unavailable-system-java'
        environment.update(PATH='/usr/bin:/bin', JAVA_HOME=str(absent_java),
                           JAVACMD=str(absent_java / 'bin/java'))
    return environment


def ltex_results(document, path, source, mode='Bundled'):
    from desktop_ui_journeys import fingerprint
    state = document.get('ltex')
    if not isinstance(state, dict):
        raise AssertionError('LTeX inspection is unavailable')
    if state['state'] == 'Failed':
        raise AssertionError('LTeX worker failed; inspect the app Activity panel and app.log')
    return (document['path'] == str(path) and document['source_fingerprint'] == fingerprint(source)
            and state.get('mode') == mode and state['enabled'] and state['current']
            and state['count'] > 0 and state['suggestions'] > 0
            and len(state['rules']) == state['count']
            and state['paths'] == [str(path)] * state['count'])


def assert_ltex_stale(document, before, path, source, mode):
    from desktop_ui_journeys import fingerprint
    state = document['ltex']
    if (document['path'] != str(path) or document['source_fingerprint'] != fingerprint(source)
            or state.get('mode') != mode or not state['enabled'] or state['state'] == 'Failed'
            or state['current'] or state['count'] != before['count'] or state['locations'] != before['locations']):
        raise AssertionError('LTeX discarded or remapped its stale result before replacement')


def assert_ltex_source_unchanged(path, source):
    if path.read_text(encoding='utf-8') != source:
        raise AssertionError('Checking grammar wrote unsaved source to disk')


def ltex_repair_completed(document):
    state = document.get('resource_repair')
    if not isinstance(state, dict) or type(state.get('busy')) is not bool:
        raise AssertionError('Resource repair inspection is unavailable')
    if state['busy']:
        return False
    if state.get('status') != 'LTeX+ downloaded':
        raise AssertionError('LTeX+ redownload did not succeed: ' + str(state.get('status')))
    return True


def assert_ltex_repair_busy(snapshot):
    state = snapshot['document'].get('resource_repair')
    if not isinstance(state, dict) or state.get('busy') is not True or state.get('status') != 'Downloading LTeX+…':
        raise AssertionError('LTeX+ redownload never entered its working state')
    targets = snapshot.get('targets', {})
    for name, enabled in [('settings.ltex.redownload', False), ('settings.ltex.cancel_redownload', True)]:
        target = targets.get(name)
        if (not isinstance(target, dict) or target.get('enabled') is not enabled
                or type(target.get('age_ms')) not in (int, float)
                or not 0 <= target['age_ms'] <= 500):
            raise AssertionError('LTeX+ repair controls are missing, stale or wrongly enabled: ' + name)


def assert_ltex_repair_preserved(document, before, preferences_before, preferences_after):
    if any(document.get(key) != before.get(key) for key in ('path', 'source_fingerprint', 'source_bytes')):
        raise AssertionError('Redownloading LTeX+ changed the source document')
    if any(document['ltex'].get(key) != before['ltex'].get(key) for key in ('enabled', 'mode')):
        raise AssertionError('Redownloading LTeX+ changed its enabled state or selected mode')
    if preferences_before != preferences_after:
        raise AssertionError('Redownloading LTeX+ changed saved preferences')


class LtexJourneys:
    def capture_ltex_settings(self, phase):
        directory = self.directory / '.tiptoptyp/screenshots'
        previous = set(directory.rglob('*.png'))
        self.capture_viewport('settings')
        originals = [path for path in set(directory.rglob('*.png')) - previous
                     if 'settings' in path.name]
        if not originals:
            raise AssertionError('No fresh original Grammar & spelling Settings capture was generated')
        for original in originals:
            retained = self.evidence / 'generated-captures' / original.relative_to(self.directory)
            retained.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, retained)
            self.record('ltex.settings.framebuffer', phase=phase, original=str(original),
                        retained=str(retained), evidence=str(self.evidence / original.name))

    def ltex(self):
        executable = ltex_program(self.ltex_executable)
        mode = 'Bundled' if executable is None else 'Custom'
        self.tex_settings([
            ('"auto_save": true', '"auto_save": false'),
            ('"build_enabled": true', '"build_enabled": false'),
        ])
        self.settings()
        if executable is not None:
            # The custom launcher controls include the full configuration
            # editor. Expand the real window before editing those controls.
            window = next(window for window in self.native('windows')['windows']
                          if 'Settings' in window['AXTitle'] and window.get('AXFocused'))
            self.native('zoom-focused')
            self.native_wait('Settings expands for the custom LTeX launcher controls', lambda windows:
                             any(candidate['AXTitle'] == window['AXTitle'] and candidate.get('size') != window['size']
                                 for candidate in windows))
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
        self.click('settings.ltex.setup')
        self.wait_target('settings.ltex.mode.bundled')
        self.wait_target('settings.ltex.redownload')
        if executable is not None:
            self.click('settings.ltex.mode.custom')
            self.wait_target('settings.ltex.executable')
            self.click('settings.ltex.executable')
            self.key(0)
            self.native('text', str(executable))
            self.wait_target('settings.ltex.apply')
            self.click('settings.ltex.apply')
        self.wait('LTeX uses the selected bundled/custom mode', lambda d: d['ltex'].get('mode') == mode)
        if self.capture_review:
            self.capture_ltex_settings('expanded')
        if executable is None:
            before = self.snapshot()['document']
            preferences = before['settings_json']['fingerprint']
            if not preferences:
                raise AssertionError('The complete validated settings snapshot is unavailable')
            self.click('settings.ltex.redownload')
            self.wait('LTeX+ redownload starts on the resource worker',
                      lambda d: d['resource_repair']['busy'])
            self.wait_target('settings.ltex.cancel_redownload')
            assert_ltex_repair_busy(self.snapshot())
            self.wait('Pinned LTeX+ distribution downloads and verifies',
                      ltex_repair_completed, timeout=180)
            self.click('settings.json.mode')
            after = self.wait('Resource repair leaves complete live settings valid', lambda d:
                              d['settings_json']['visible'] and d['settings_json']['valid'])['document']
            assert_ltex_repair_preserved(after, before, preferences, after['settings_json']['fingerprint'])
            self.click('settings.json.mode')
            self.wait('Grammar & spelling settings returns after preference comparison', lambda d:
                      not d['settings_json']['visible'])
            self.record('ltex.redownload.completed', status=after['resource_repair']['status'],
                        source_unchanged=True, preferences_unchanged=True)
            if self.capture_review:
                self.capture_ltex_settings('recovered')
        self.record('ltex.resource_controls', mode=mode, redownload_available=True,
                    external_launcher=executable is not None,
                    network_action_performed=executable is None)
        self.click('settings.ltex.enabled')
        self.close_settings()
        for suffix, source, corrected in (
            ('tex', '\\documentclass{article}\n\\begin{document}\nThis are a simple result.\n\\end{document}\n',
             '\\documentclass{article}\n\\begin{document}\nThis is a simple result.\n\\end{document}\n'),
            ('typ', 'This are a simple result.\n', 'This is a simple result.\n'),
        ):
            path = self.directory / ('ltex-grammar.' + suffix)
            path.write_text(source, encoding='utf-8')
            self.open_harper_fixture(path)
            before = self.wait('Real LTeX+ returns mapped ' + suffix + ' grammar and corrections',
                               lambda d: ltex_results(d, path, source, mode), timeout=90)['document']['ltex']
            self.key(0)

            def stale():
                assert_ltex_stale(self.source_is(corrected)['document'], before, path, corrected, mode)

            self.paste_source(corrected, stale)
            self.wait('Corrected ' + suffix + ' source receives a clean LTeX result', lambda d:
                      d['ltex']['current'] and d['ltex']['count'] == 0, timeout=60)
            self.stable_state(lambda d: d['ltex']['current'] and d['ltex']['count'] == 0)
            assert_ltex_source_unchanged(path, source)
            self.record('ltex.mapped_stale_and_refresh', language=suffix, mode=mode,
                        rules=before['rules'], locations=before['locations'])
        self.settings()
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
        self.click('settings.ltex.enabled')
        self.close_settings()
        self.wait('Disabling LTeX retires its diagnostics', lambda d:
                  not d['ltex']['enabled'] and d['ltex']['count'] == 0)
