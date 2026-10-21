"""Optional real LTeX+ checks through native editor interaction."""
import json
import os
from pathlib import Path
import shutil

LTEX_JOURNEYS = ('ltex',)


def ltex_program(value=None):
    candidate = str(value) if value else shutil.which('ltex-ls-plus')
    if not candidate or not Path(candidate).is_file() or not os.access(candidate, os.X_OK):
        raise ValueError('ltex requires a real LTeX+ launcher; pass --ltex-executable. An absent server is an unavailable check, not a pass')
    return Path(candidate).resolve()


def ltex_results(document, path, source):
    from desktop_ui_journeys import fingerprint
    state = document.get('ltex')
    if not isinstance(state, dict):
        raise AssertionError('LTeX inspection is unavailable')
    if state['state'] == 'Failed':
        raise AssertionError('LTeX worker failed; inspect the app Activity panel and app.log')
    return (document['path'] == str(path) and document['source_fingerprint'] == fingerprint(source)
            and state['enabled'] and state['current'] and state['count'] > 0
            and state['suggestions'] > 0 and len(state['rules']) == state['count']
            and state['paths'] == [str(path)] * state['count'])


class LtexJourneys:
    def ltex(self):
        executable = ltex_program(self.ltex_executable)
        self.tex_settings([
            ('"executable": "ltex-ls-plus"', '"executable": ' + json.dumps(str(executable))),
            ('"auto_save": true', '"auto_save": false'),
            ('"build_enabled": true', '"build_enabled": false'),
        ])
        self.settings()
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
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
                               lambda d: ltex_results(d, path, source), timeout=90)['document']['ltex']
            self.key(0)

            def stale():
                state = self.source_is(corrected)['document']['ltex']
                if state['current'] or state['count'] != before['count'] or state['locations'] != before['locations']:
                    raise AssertionError('LTeX discarded its stale result before replacement')

            self.paste_source(corrected, stale)
            self.wait('Corrected ' + suffix + ' source receives a clean LTeX result', lambda d:
                      d['ltex']['current'] and d['ltex']['count'] == 0, timeout=60)
            self.stable_state(lambda d: d['ltex']['current'] and d['ltex']['count'] == 0)
            if path.read_text(encoding='utf-8') != source:
                raise AssertionError('Checking grammar wrote unsaved source to disk')
            self.record('ltex.mapped_stale_and_refresh', language=suffix,
                        rules=before['rules'], locations=before['locations'])
        self.settings()
        self.setting_section('LTeX', 'LTeX+ spelling and grammar')
        self.click('settings.ltex.enabled')
        self.close_settings()
        self.wait('Disabling LTeX retires its diagnostics', lambda d:
                  not d['ltex']['enabled'] and d['ltex']['count'] == 0)
