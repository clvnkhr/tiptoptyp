"""Texdoc lookup through native keys/clicks and read-only app observations."""
import os
from pathlib import Path
import re

TEXDOC_JOURNEYS = ('tex_documentation',)


def texdoc_program():
    """An optional local integration must fail explicitly when its tool is absent."""
    directories = [Path(value) for value in os.get_exec_path() if Path(value).is_absolute()]
    directories.append(Path('/Library/TeX/texbin'))
    for directory in directories:
        candidate = directory / 'texdoc'
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise ValueError('tex_documentation requires a local executable Texdoc (TeX Live/MacTeX); this is an unavailable check, not a pass')


def texdoc_state(document):
    state = document.get('tex_documentation')
    required = {'visible', 'busy', 'query_fingerprint', 'error', 'unavailable', 'manual_paths'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(key)) is not bool for key in ('visible', 'busy', 'unavailable'))
            or state['busy'] and not state['visible']):
        raise AssertionError('Texdoc inspection is missing or malformed')
    query = state['query_fingerprint']
    if ((state['visible'] or query is not None)
            and (not isinstance(query, str) or re.fullmatch(r'[0-9a-f]{16}', query) is None)):
        raise AssertionError('Texdoc query fingerprint is invalid')
    if state['error'] is not None and (not isinstance(state['error'], str) or not state['error'].strip()):
        raise AssertionError('Texdoc search error inspection is invalid')
    paths = state['manual_paths']
    if (not isinstance(paths, list) or len(paths) > 100
            or any(not isinstance(path, str) or not Path(path).is_absolute() for path in paths)
            or len(paths) != len(set(paths))):
        raise AssertionError('Texdoc manuals are invalid or duplicated')
    if not state['visible'] and (state['busy'] or paths or state['unavailable']):
        raise AssertionError('Closed Texdoc dialog retains active search state')
    return state


def texdoc_owner_unchanged(document, before):
    return all(document[key] == before[key] for key in
               ('path', 'source_fingerprint', 'source_bytes', 'tabs', 'active_tab_id',
                'preview_tab_id', 'preview_path'))


def texdoc_results(document):
    state = texdoc_state(document)
    if state['visible'] and not state['busy'] and (state['error'] or state['unavailable']):
        raise AssertionError('Local Texdoc search unavailable: ' + (state['error'] or 'Texdoc is not installed'))
    return state['visible'] and not state['busy'] and bool(state['manual_paths'])


class TexdocJourneys:
    def open_tex_documentation(self):
        self.focus_document_window()
        self.palette_open()
        palette = self.palette_query('TeX Documentation')
        selected = palette['palette_selected']
        if selected is None or selected not in palette['palette_enabled']:
            raise AssertionError('TeX documentation palette action is unavailable')
        self.key(36, 'none')
        self.wait('Texdoc opens from the palette on a Typst document', lambda d:
                  not d['palette_visible'] and texdoc_state(d)['visible'])
        self.wait_target('tex-documentation.query', focused=True)

    def texdoc_type(self, query):
        from desktop_ui_journeys import fingerprint
        self.click('tex-documentation.query')
        self.wait_target('tex-documentation.query', focused=True)
        self.key(0)
        self.native('text', query)
        self.wait('Texdoc query receives exact native typing', lambda d:
                  texdoc_state(d)['query_fingerprint'] == fingerprint(query))

    def tex_documentation(self):
        texdoc_program()
        self.focus_document_window()
        before = self.wait('The paper owns its preview before opening a manual', lambda d:
                           d['path'] is not None and d['preview_tab_id'] == d['active_tab_id']
                           and d['preview_path'] == d['path'])['document']
        unchanged = lambda d: texdoc_owner_unchanged(d, before)

        self.open_tex_documentation()
        self.texdoc_type('amsmath')
        self.key(53, 'none')
        self.wait('Escape closes only Texdoc without changing source or preview', lambda d:
                  not texdoc_state(d)['visible'] and unchanged(d))
        self.record('texdoc.escape_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_tex_documentation()
        self.texdoc_type('amsmath')
        self.key(13)
        self.wait('Cmd+W closes only the focused documentation dialog', lambda d:
                  not texdoc_state(d)['visible'] and unchanged(d))
        self.record('texdoc.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_tex_documentation()
        self.texdoc_type('amsmath')
        self.key(36, 'none')
        found = self.wait('The real Texdoc worker returns installed amsmath manuals', lambda d:
                          texdoc_results(d) and unchanged(d), timeout=20)['document']
        manual = Path(texdoc_state(found)['manual_paths'][0])
        if not manual.is_file() or manual.suffix.lower() != '.pdf':
            raise AssertionError('The amsmath journey requires its real installed PDF manual; none was returned')
        self.record('texdoc.local_results', count=len(texdoc_state(found)['manual_paths']))
        self.click('tex-documentation.open.0')
        self.wait('The local manual opens in its own tab and preserves the paper preview', lambda d:
                  not texdoc_state(d)['visible'] and d['path'] == str(manual)
                  and d['tabs'] == before['tabs'] + 1 and d['active_tab_id'] != before['active_tab_id']
                  and d['preview_tab_id'] == before['preview_tab_id']
                  and d['preview_path'] == before['preview_path'] and not d['dirty'], timeout=20)
        self.focus_document_window()
        self.key(13)
        self.wait('Closing the documentation tab restores the unchanged paper', lambda d:
                  unchanged(d) and not texdoc_state(d)['visible'])
        self.record('texdoc.complete', source_fingerprint=before['source_fingerprint'])
