"""Offline Overleaf connection controls through native input; no remote writes."""
from pathlib import Path
import re

OVERLEAF_JOURNEYS = ('overleaf_connection',)
INVALID_HTTP = 'http://www.overleaf.com/project/0123456789abcdef01234567'
INVALID_QUERY = 'https://www.overleaf.com/project/0123456789abcdef01234567?token=fixture'
OWNER_FIELDS = ('path', 'source_fingerprint', 'source_bytes', 'revision', 'dirty', 'tabs',
                'active_tab_id', 'preview_tab_id', 'preview_path')


def overleaf_state(document):
    """Require actual observation rather than accepting missing data as closed."""
    state = document.get('overleaf')
    flags = ('visible', 'connection', 'browser', 'create_browser', 'pending')
    required = {*flags, 'query_fingerprint', 'error', 'epoch'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(field)) is not bool for field in flags)
            or type(state['epoch']) is not int or state['epoch'] < 0
            or not isinstance(state['query_fingerprint'], str)
            or re.fullmatch(r'[0-9a-f]{16}', state['query_fingerprint']) is None
            or state['error'] is not None and
            (not isinstance(state['error'], str) or not state['error'].strip())
            or (state['browser'] or state['create_browser'] or state['pending']) and
            not state['connection']):
        raise AssertionError('Overleaf connection inspection is missing or malformed')
    return state


def overleaf_owner_unchanged(document, before):
    if any(field not in document or field not in before for field in OWNER_FIELDS):
        raise AssertionError('Overleaf journey lacks complete source-owner evidence')
    return all(document[field] == before[field] for field in OWNER_FIELDS)


def overleaf_offline(document, before, *, visible, query=None, error=False):
    from desktop_ui_journeys import fingerprint
    state = overleaf_state(document)
    return (overleaf_owner_unchanged(document, before)
            and state['visible'] is visible
            and not any(state[flag] for flag in
                        ('connection', 'browser', 'create_browser', 'pending'))
            and state['epoch'] == 0
            and (query is None or state['query_fingerprint'] == fingerprint(query))
            and (not error or state['error'] is not None))


class OverleafJourneys:
    def open_overleaf_connection(self):
        self.focus_document_window()
        self.palette_open()
        palette = self.palette_query('Connect to Overleaf')
        selected = palette['palette_selected']
        if selected != 'file.overleaf' or selected not in palette['palette_enabled']:
            raise AssertionError('The Overleaf palette command is unavailable or selects another action')
        self.key(36, 'none')
        self.wait('Overleaf opens from the actual command palette', lambda d:
                  not d['palette_visible'] and overleaf_state(d)['visible'])
        self.wait_target('overleaf.url', focused=True)

    def overleaf_type_query(self, value, before):
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        self.key(0)
        self.native('text', value)
        self.wait('The focused Overleaf field receives exact typing without source edits',
                  lambda d: overleaf_offline(d, before, visible=True, query=value))

    def overleaf_undo_initial_query(self, before, query):
        # Exercise a known first undo boundary. Egui intentionally groups rapid
        # replacements, so later Select All edits need not undo one at a time.
        self.wait_target('overleaf.url', focused=True)
        self.key(6)
        self.wait('Native Undo restores the URL field to its exact initial empty state',
                  lambda d: overleaf_offline(d, before, visible=True, query=''))
        self.key(6, 'cmd+shift')
        self.wait('Native Redo restores the exact first URL without source edits',
                  lambda d: overleaf_offline(d, before, visible=True, query=query))

    def overleaf_connection(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        initial = overleaf_state(before)
        if initial['visible'] or initial['connection'] or initial['epoch'] != 0:
            raise AssertionError('Overleaf journey requires a fresh disconnected fixture session')
        source_path = Path(before['path'])
        original = source_path.read_text(encoding='utf-8')
        if fingerprint(original) != before['source_fingerprint']:
            raise AssertionError('The Overleaf journey source differs from its private disk fixture')
        unchanged = lambda d: overleaf_owner_unchanged(d, before)

        self.open_overleaf_connection()
        self.wait('A new connection has an empty URL and no browser', lambda d:
                  overleaf_offline(d, before, visible=True, query=''))
        self.click('overleaf.connect')
        self.wait('Blank URL fails before allocating a browser or connection', lambda d:
                  overleaf_offline(d, before, visible=True, query='', error=True))
        self.record('overleaf.blank_rejected', connection_started=False)

        self.overleaf_type_query(INVALID_HTTP, before)
        self.overleaf_undo_initial_query(before, INVALID_HTTP)
        self.click('overleaf.connect')
        self.wait('Plain HTTP cloud URL is rejected before network activity', lambda d:
                  overleaf_offline(d, before, visible=True, query=INVALID_HTTP, error=True)
                  and 'HTTPS' in overleaf_state(d)['error'])
        self.overleaf_type_query(INVALID_QUERY, before)
        self.click('overleaf.connect')
        self.wait('Token-bearing URL is rejected before browser allocation', lambda d:
                  overleaf_offline(d, before, visible=True, query=INVALID_QUERY, error=True)
                  and 'query' in overleaf_state(d)['error'].lower())
        self.record('overleaf.invalid_rejected', connection_started=False)

        # The replacement above already proves Select All stays in the child.
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        for code, label in ((3, 'Find'), (45, 'New document')):
            self.key(code)
            self.stable_state(lambda d:
                              overleaf_offline(d, before, visible=True, query=INVALID_QUERY)
                              and not d['find_visible'] and not d['palette_visible'])
            self.record('overleaf.child_command_isolated', command=label)
        self.key(13)
        self.wait('Cmd+W hides only the focused Overleaf window', lambda d:
                  overleaf_offline(d, before, visible=False, query=INVALID_QUERY))
        self.stable_state(unchanged)
        self.record('overleaf.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_overleaf_connection()
        self.wait('Reopening retains the unsent URL draft without connecting', lambda d:
                  overleaf_offline(d, before, visible=True, query=INVALID_QUERY))
        self.click('overleaf.hide')
        self.wait('Hide returns to the document and retains the draft', lambda d:
                  overleaf_offline(d, before, visible=False, query=INVALID_QUERY))
        self.focus_document_window()
        self.click('editor.source')
        self.key(3)
        self.wait('The document Find shortcut returns after hiding Overleaf', lambda d:
                  d['find_visible'] and d['find_focused'] and unchanged(d))
        self.key(3)
        self.wait('The focused document Find closes normally without source edits', lambda d:
                  not d['find_visible'] and unchanged(d))
        if source_path.read_text(encoding='utf-8') != original:
            raise AssertionError('The Overleaf connection controls modified the source fixture on disk')
        self.record('overleaf.complete', live_auth_tested=False, remote_writes=False,
                    source_fingerprint=before['source_fingerprint'])
