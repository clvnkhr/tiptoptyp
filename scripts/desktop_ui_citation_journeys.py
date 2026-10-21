"""Citation review/import through native input with deterministic offline metadata."""
from pathlib import Path
import re

CITATION_JOURNEYS = ('citation_import',)
CITATION_DOI = '10.5555/tiptoptyp-desktop-test'
CITATION_METADATA = ('@article{Desktop2026,\n  title = {A deterministic metadata fixture},\n'
                     '  author = {Example, Alice},\n  year = {2026},\n'
                     '  doi = {10.5555/tiptoptyp-desktop-test}\n}\n')
CITATION_EDITED = CITATION_METADATA.replace('A deterministic metadata fixture', 'The reviewed title')
CITATION_EXISTING = '@article{Existing2020, title={Existing entry}, year={2020}}\n'
CITATION_SOURCE = 'See CITATION here.\n#bibliography("references.bib")\n'
CITATION_KEY = 'Reviewed2026+'
CITATION_APPLIED = CITATION_SOURCE.replace('CITATION', '#cite(label("Reviewed2026+"))')


def citation_state(document):
    state = document.get('citation')
    required = {'visible', 'busy', 'review', 'completed', 'query_fingerprint',
                'destination_fingerprint', 'key_fingerprint', 'bibtex_fingerprint',
                'insert', 'added', 'error'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(key)) is not bool for key in
                   ('visible', 'busy', 'review', 'completed', 'insert'))
            or state['busy'] and not state['visible']
            or state['completed'] and not state['review']):
        raise AssertionError('Citation import inspection is missing or malformed')
    for key in ('query_fingerprint', 'destination_fingerprint', 'key_fingerprint', 'bibtex_fingerprint'):
        value = state[key]
        if (state['visible'] or value is not None) and (not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{16}', value) is None):
            raise AssertionError('Citation field fingerprint is invalid')
    if state['added'] is not None and type(state['added']) is not bool:
        raise AssertionError('Citation result status is invalid')
    if state['completed'] != (state['added'] is not None):
        raise AssertionError('Citation completion status is incomplete')
    if state['error'] is not None and (not isinstance(state['error'], str) or not state['error'].strip()):
        raise AssertionError('Citation error inspection is invalid')
    if not state['visible'] and (state['busy'] or state['review'] or state['completed'] or state['insert']):
        raise AssertionError('Closed citation dialog retains active review state')
    return state


def citation_review(document):
    state = citation_state(document)
    if state['visible'] and not state['busy'] and state['error']:
        raise AssertionError('Citation lookup failed: ' + state['error'])
    return state['visible'] and not state['busy'] and state['review'] and not state['completed']


def citation_owner_unchanged(document, before):
    return all(document[key] == before[key] for key in
               ('path', 'source_fingerprint', 'source_bytes', 'tabs', 'active_tab_id',
                'preview_tab_id', 'preview_path'))


def citation_artifact(path, original):
    """Require preservation plus the reviewed fields, never merely a success flag."""
    if not path.is_file() or not 0 < path.stat().st_size <= 1_000_000:
        raise AssertionError('Citation import did not produce a bounded bibliography')
    content = path.read_text(encoding='utf-8')
    if (not content.startswith(original) or content.count('@article{') != 2
            or '@article{' + CITATION_KEY + ',' not in content
            or 'The reviewed title' not in content or CITATION_DOI not in content):
        raise AssertionError('Citation import failed to preserve the bibliography or reviewed fields')
    return content


class CitationJourneys:
    def open_citation_import(self):
        self.focus_document_window()
        self.palette_open()
        palette = self.palette_query('Import citation')
        selected = palette['palette_selected']
        if selected is None or selected not in palette['palette_enabled']:
            raise AssertionError('Citation import palette command is unavailable')
        self.key(36, 'none')
        self.wait('Citation import opens from the real palette', lambda d:
                  not d['palette_visible'] and citation_state(d)['visible'])
        self.wait_target('citation.query', focused=True)

    def citation_type(self, target, value, field):
        from desktop_ui_journeys import fingerprint
        self.click(target)
        self.wait_target(target, focused=True)
        self.key(0)
        def received():
            self.wait('Citation field receives the exact native input', lambda d:
                      citation_state(d)[field] == fingerprint(value))
        if '\n' in value or '\r' in value:
            self.paste_source(value, check=received)
        else:
            self.native('text', value)
            received()

    def citation_import(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        if before['source_fingerprint'] != fingerprint(CITATION_SOURCE):
            raise AssertionError('The citation journey did not receive its exact source fixture')
        path = self.directory / 'references.bib'
        if path.read_text(encoding='utf-8') != CITATION_EXISTING:
            raise AssertionError('Citation fixture bibliography was not prepared exactly')
        unchanged = lambda d: citation_owner_unchanged(d, before)
        self.open_citation_import()
        self.citation_type('citation.query', CITATION_DOI, 'query_fingerprint')
        self.key(13)
        self.wait('Cmd+W closes only citation import without editing source', lambda d:
                  not citation_state(d)['visible'] and unchanged(d))
        if path.read_text(encoding='utf-8') != CITATION_EXISTING:
            raise AssertionError('Closing citation import modified its destination')
        self.record('citation.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.click('editor.source')
        self.key(126)  # Cmd+Up, then select only the placeholder in the first line.
        for _ in range(4): self.key(124, 'none')
        for _ in range(8): self.key(124, 'shift')
        self.wait('The original citation placeholder is selected', lambda d:
                  sorted(d['cursor']) == [4, 12] and unchanged(d))
        self.open_citation_import()
        self.citation_type('citation.query', CITATION_DOI, 'query_fingerprint')
        self.key(36, 'none')
        reviewed = self.wait('The background metadata lookup reaches editable review', lambda d:
                             citation_review(d) and unchanged(d), timeout=15)['document']
        if citation_state(reviewed)['bibtex_fingerprint'] != fingerprint(CITATION_METADATA.strip()):
            raise AssertionError('Citation review differs from the parsed metadata fixture')
        self.citation_type('citation.key', CITATION_KEY, 'key_fingerprint')
        self.citation_type('citation.bibtex', CITATION_EDITED, 'bibtex_fingerprint')
        self.click('citation.import')
        self.wait('Import appends the reviewed citation and replaces only selected text', lambda d:
                  citation_state(d)['completed'] and citation_state(d)['added'] is True
                  and citation_state(d)['error'] is None
                  and d['source_fingerprint'] == fingerprint(CITATION_APPLIED)
                  and d['tabs'] == before['tabs'] and d['active_tab_id'] == before['active_tab_id']
                  and d['preview_tab_id'] == before['preview_tab_id'], timeout=15)
        citation_artifact(path, CITATION_EXISTING)
        self.record('citation.imported_reviewed_entry', source_fingerprint=fingerprint(CITATION_APPLIED))
        self.click('citation.done')
        self.wait('Done closes citation review', lambda d: not citation_state(d)['visible'])
        self.click('editor.source')
        self.key(6)
        self.wait('One native Undo restores the complete original selection and source', lambda d:
                  unchanged(d) and not citation_state(d)['visible'])
        citation_artifact(path, CITATION_EXISTING)
        self.record('citation.complete', source_fingerprint=before['source_fingerprint'])
