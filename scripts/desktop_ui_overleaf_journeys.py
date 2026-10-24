"""Offline Overleaf connection controls through native input; no remote writes."""
from pathlib import Path
import math
import re

OVERLEAF_JOURNEYS = ('overleaf_connection', 'overleaf_storage')
INVALID_HTTP = 'http://www.overleaf.com/project/0123456789abcdef01234567'
INVALID_QUERY = 'https://www.overleaf.com/project/0123456789abcdef01234567?token=fixture'
# A syntactically valid project on a reserved, nonexistent host. Invalid local
# settings must reject this before allocating any connection or storage worker.
STORAGE_PROJECT = 'https://overleaf.invalid/project/0123456789abcdef01234567'
RELATIVE_DIRECTORY = 'relative-overleaf-copies'
OWNER_FIELDS = ('path', 'source_fingerprint', 'source_bytes', 'revision', 'dirty', 'tabs',
                'active_tab_id', 'preview_tab_id', 'preview_path')


def overleaf_preview_ready(document, fixture):
    """The storage journey must preserve a real, visible Tinymist source preview."""
    from desktop_ui_journeys import PREVIEW_SOURCE, fingerprint
    required = ('path', 'document_kind', 'source_fingerprint', 'source_bytes', 'active_tab_id',
                'preview_tab_id', 'preview_path', 'preview_backend', 'preview_requested_backend', 'preview_native_ready',
                'preview_surface_visible', 'preview_pages', 'compile_indicator_spinning')
    if (not isinstance(document, dict) or not all(field in document for field in required)
            or any(type(document[field]) is not bool for field in
                   ('preview_native_ready', 'preview_surface_visible', 'compile_indicator_spinning'))
            or type(document['preview_pages']) is not int or document['preview_pages'] < 0):
        raise AssertionError('The real Overleaf fixture preview observation is unavailable')
    return (document['path'] == str(fixture) and document['document_kind'] == 'Typst'
            and document['source_fingerprint'] == fingerprint(PREVIEW_SOURCE)
            and type(document['source_bytes']) is int and document['source_bytes'] == len(PREVIEW_SOURCE.encode())
            and type(document['active_tab_id']) is int
            and document['preview_tab_id'] == document['active_tab_id']
            and document['preview_path'] == str(fixture) and Path(fixture).is_absolute()
            and (document['preview_requested_backend'], document['preview_backend']) in
                (('Interactive', 'Tinymist'), ('InteractivePartial', 'Tinymist partial'))
            and document['preview_native_ready']
            and document['preview_surface_visible'] and document['preview_pages'] == 3
            and not document['compile_indicator_spinning'])


def overleaf_state(document):
    """Require actual observation rather than accepting missing data as closed."""
    state = document.get('overleaf')
    flags = ('visible', 'connection', 'browser', 'create_browser', 'pending',
             'storage_preparing', 'storage_keep_local_copy', 'storage_directory_visible')
    required = {*flags, 'query_fingerprint', 'storage_directory_fingerprint', 'error', 'epoch'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(field)) is not bool for field in flags)
            or type(state['epoch']) is not int or state['epoch'] < 0
            or not isinstance(state['query_fingerprint'], str)
            or re.fullmatch(r'[0-9a-f]{16}', state['query_fingerprint']) is None
            or not isinstance(state['storage_directory_fingerprint'], str)
            or re.fullmatch(r'[0-9a-f]{16}', state['storage_directory_fingerprint']) is None
            or state['storage_directory_visible'] !=
            (state['visible'] and state['storage_keep_local_copy'])
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
                        ('connection', 'browser', 'create_browser', 'pending', 'storage_preparing'))
            and state['epoch'] == 0
            and (query is None or state['query_fingerprint'] == fingerprint(query))
            and (not error or state['error'] is not None))


def overleaf_reconcile_state(document):
    """Require real connection/review observations before claiming a merge or resolution."""
    state = overleaf_state(document)
    review = state.get('review')
    if (any(type(state.get(field)) is not bool for field in ('sync_ready', 'source_locked'))
            or type(state.get('blocked_count')) is not int or state['blocked_count'] < 0
            or not isinstance(review, dict)
            or not {'open', 'loading', 'left', 'right', 'error'}.issubset(review)
            or any(type(review[field]) is not bool for field in ('open', 'loading'))
            or any(value is not None and (not isinstance(value, str)
                   or re.fullmatch(r'[0-9a-f]{16}', value) is None)
                   for value in (review['left'], review['right']))
            or review['error'] is not None and
               (not isinstance(review['error'], str) or not review['error'].strip())
            or not review['open'] and review['loading']):
        raise AssertionError('Overleaf reconciliation inspection is missing or malformed')
    return state


def overleaf_source_preserved(document, before):
    """Service rekeys may change revision while the bound visible source stays intact."""
    fields = ('path', 'source_fingerprint', 'source_bytes', 'dirty', 'tabs',
              'active_tab_id', 'preview_tab_id', 'preview_path', 'cursor')
    for state in (document, before):
        if (not isinstance(state, dict) or not all(field in state for field in fields)
                or not isinstance(state['path'], str) or not state['path']
                or not isinstance(state['source_fingerprint'], str)
                or re.fullmatch(r'[0-9a-f]{16}', state['source_fingerprint']) is None
                or type(state['source_bytes']) is not int or state['source_bytes'] < 0
                or type(state['dirty']) is not bool or type(state['tabs']) is not int or state['tabs'] < 1
                or type(state['active_tab_id']) is not int or state['active_tab_id'] < 0
                or state['preview_tab_id'] is not None and
                   (type(state['preview_tab_id']) is not int or state['preview_tab_id'] < 0)
                or state['preview_path'] is not None and not isinstance(state['preview_path'], str)
                or not isinstance(state['cursor'], list) or len(state['cursor']) != 2
                or any(type(position) is not int or position < 0 for position in state['cursor'])):
            raise AssertionError('Overleaf source/selection ownership evidence is incomplete')
    return all(document[field] == before[field] for field in fields)


def overleaf_chooser_state(document, before, *, local, remote, root):
    """Switching chooser surfaces cannot silently disconnect or edit the open project."""
    from desktop_ui_journeys import fingerprint
    if (type(document.get('workspace_chooser_visible')) is not bool
            or not isinstance(document.get('workspace_root_fingerprint'), str)
            or re.fullmatch(r'[0-9a-f]{16}', document['workspace_root_fingerprint']) is None):
        raise AssertionError('Workspace chooser/root inspection is unavailable')
    state, previous = overleaf_state(document), overleaf_state(before)
    if any(type(item.get('sync_ready')) is not bool for item in (state, previous)):
        raise AssertionError('Overleaf chooser connection readiness is unavailable')
    return (overleaf_source_preserved(document, before)
            and document['workspace_chooser_visible'] is local and state['visible'] is remote
            and document['workspace_root_fingerprint'] == fingerprint(str(root))
            and all(state[field] == previous[field] for field in
                    ('connection', 'browser', 'create_browser', 'pending', 'epoch', 'sync_ready'))
            and state['connection'] and state['sync_ready'])


def overleaf_review_pair(document, remote, local):
    from desktop_ui_journeys import fingerprint
    state = overleaf_reconcile_state(document)
    review = state['review']
    return (state['connection'] and state['sync_ready'] and state['blocked_count'] == 1
            and state['source_locked'] and not state['pending']
            and review['open'] and not review['loading'] and review['error'] is None
            and review['left'] == fingerprint(remote) and review['right'] == fingerprint(local))


def overleaf_review_docked(snapshot):
    """The live full-source comparison belongs to the code pane in the root window."""
    document = snapshot['document']
    review = overleaf_reconcile_state(document)['review']
    if (not review['open'] or review['loading'] or review['error'] is not None
            or review['left'] is None or review['right'] is None):
        raise AssertionError('The Overleaf comparison is not ready')
    targets = []
    for name in ('overleaf.review.panel', 'overleaf.review.diff'):
        target = snapshot.get('targets', {}).get(name)
        if (not isinstance(target, dict) or target.get('viewport') != document.get('viewport')
                or type(target.get('frame')) is not int or target['frame'] != document.get('frame')
                or type(target.get('age_ms')) not in (int, float) or not 0 <= target['age_ms'] <= 500
                or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                       for key in ('x', 'y', 'width', 'height'))
                or target['width'] <= 0 or target['height'] <= 0):
            raise AssertionError('Missing, stale or non-root Overleaf review geometry: ' + name)
        targets.append(target)
    left = lambda target: target['x'] - target['width'] / 2
    right = lambda target: target['x'] + target['width'] / 2
    top = lambda target: target['y'] - target['height'] / 2
    bottom = lambda target: target['y'] + target['height'] / 2
    panel, diff = targets
    if (left(diff) < left(panel) - 1 or right(diff) > right(panel) + 1
            or top(diff) < top(panel) - 1 or bottom(diff) > bottom(panel) + 1):
        raise AssertionError('Overleaf comparison escaped its code panel')
    if document.get('preview_surface_visible') is True:
        preview = snapshot.get('targets', {}).get('preview.page')
        if (not isinstance(preview, dict) or preview.get('viewport') != document.get('viewport')
                or type(preview.get('frame')) is not int or preview['frame'] != document.get('frame')
                or type(preview.get('age_ms')) not in (int, float) or not 0 <= preview['age_ms'] <= 500
                or any(type(preview.get(key)) not in (int, float) or not math.isfinite(preview[key])
                       for key in ('x', 'y', 'width', 'height'))
                or preview['width'] <= 0 or preview['height'] <= 0):
            raise AssertionError('The visible Overleaf preview lacks current root geometry')
        if right(panel) > left(preview) + 1:
            raise AssertionError('Overleaf review covered its visible preview')
    return True


def overleaf_storage(document, *, keep, directory):
    from desktop_ui_journeys import fingerprint
    state = overleaf_state(document)
    return (state['storage_keep_local_copy'] is keep
            and state['storage_directory_fingerprint'] == fingerprint(directory))


def overleaf_storage_targets(snapshot, *, keep):
    """Hidden controls may remain in the inspector, but not in the current frame."""
    targets = snapshot.get('targets', {})
    checkbox = targets.get('overleaf.keep_local_copy')
    if (not checkbox or checkbox.get('enabled') is not True
            or type(checkbox.get('frame')) is not int or checkbox['frame'] < 0
            or not isinstance(checkbox.get('viewport'), str) or not checkbox['viewport']
            or type(checkbox.get('age_ms')) not in (int, float)
            or not 0 <= checkbox['age_ms'] <= 500):
        return False
    for name in ('overleaf.directory', 'overleaf.choose_folder'):
        target = targets.get(name)
        current = bool(target and target.get('enabled') is True
                       and type(target.get('frame')) is int
                       and target.get('frame') == checkbox['frame']
                       and target.get('viewport') == checkbox.get('viewport')
                       and type(target.get('age_ms')) in (int, float)
                       and 0 <= target['age_ms'] <= 500)
        if current != keep:
            return False
    return True


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

    def overleaf_type_directory(self, value, before, query):
        self.click('overleaf.directory')
        self.wait_target('overleaf.directory', focused=True)
        self.key(0)
        self.native('text', value)
        self.wait('The local-copy folder receives exact native typing without source edits',
                  lambda d: overleaf_offline(d, before, visible=True, query=query)
                  and overleaf_storage(d, keep=True, directory=value))

    def overleaf_storage_controls(self, before):
        self.wait_target('overleaf.keep_local_copy')
        self.wait_target('overleaf.directory')
        if not overleaf_storage_targets(self.snapshot(), keep=True):
            raise AssertionError('Local-copy controls are not current visible native targets')
        self.click('overleaf.keep_local_copy')
        hidden = self.wait('Temporary mode hides the physical local-copy folder', lambda d:
                           overleaf_offline(d, before, visible=True, query=INVALID_QUERY)
                           and overleaf_storage(d, keep=False, directory=''))
        if not overleaf_storage_targets(hidden, keep=False):
            raise AssertionError('Temporary mode still exposes local-copy folder controls')
        if getattr(self, 'capture_review', False):
            self.capture_viewport('overleaf')
        self.click('overleaf.keep_local_copy')
        restored = self.wait('Keeping a local copy restores its folder controls', lambda d:
                             overleaf_offline(d, before, visible=True, query=INVALID_QUERY)
                             and overleaf_storage(d, keep=True, directory=''))
        if not overleaf_storage_targets(restored, keep=True):
            raise AssertionError('Local-copy folder controls did not return')
        directory = str(self.directory / 'overleaf-copies')
        if not Path(directory).is_absolute():
            raise AssertionError('The local-copy journey needs an absolute private fixture folder')
        self.overleaf_type_directory(directory, before, INVALID_QUERY)
        self.click('overleaf.connect')
        self.wait('URL rejection preserves the unsent custom local-copy folder', lambda d:
                  overleaf_offline(d, before, visible=True, query=INVALID_QUERY, error=True)
                  and overleaf_storage(d, keep=True, directory=directory))
        self.record('overleaf.custom_folder_preserved', connection_started=False)
        if getattr(self, 'capture_review', False):
            self.capture_viewport('overleaf')

        # The invalid directory itself must stop this syntactically valid URL
        # before an epoch, worker, browser or network connection is allocated.
        self.overleaf_type_query(STORAGE_PROJECT, before)
        self.overleaf_type_directory(RELATIVE_DIRECTORY, before, STORAGE_PROJECT)
        self.click('overleaf.connect')
        self.wait('Relative local-copy folders fail before preparing or connecting', lambda d:
                  overleaf_offline(d, before, visible=True, query=STORAGE_PROJECT, error=True)
                  and overleaf_storage(d, keep=True, directory=RELATIVE_DIRECTORY)
                  and 'absolute' in overleaf_state(d)['error'].lower())
        self.record('overleaf.relative_folder_rejected', connection_started=False,
                    storage_worker_started=False)
        self.overleaf_type_query(INVALID_QUERY, before)
        self.overleaf_type_directory(directory, before, INVALID_QUERY)
        return directory

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
        directory = self.overleaf_storage_controls(before)

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
                  overleaf_offline(d, before, visible=False, query=INVALID_QUERY)
                  and overleaf_storage(d, keep=True, directory=directory))
        self.stable_state(unchanged)
        self.record('overleaf.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_overleaf_connection()
        self.wait('Reopening retains the unsent URL draft without connecting', lambda d:
                  overleaf_offline(d, before, visible=True, query=INVALID_QUERY)
                  and overleaf_storage(d, keep=True, directory=directory))
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


    def overleaf_storage(self):
        """Positive local-HTTP/WebView journey; no real account or source injection."""
        import time
        from overleaf_storage_fixture import ASSET_BYTES, SOURCE, StorageFixture
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        fixture_path = self.directory / 'journey.typ'
        if self.snapshot()['document']['path'] != str(fixture_path):
            self.open_harper_fixture(fixture_path)
        self.click('editor.source')
        self.key(35, 'cmd+alt')  # Pin the real initial Typst document before opening remote text.
        self.key(20)
        self.key(15)
        before = self.wait('The unchanged initial Typst fixture has a visible ready Tinymist preview',
                           lambda d: overleaf_preview_ready(d, fixture_path), timeout=60)['document']
        initial = overleaf_state(before)
        if initial['connection'] or initial['browser']:
            raise AssertionError('Storage journey requires a disconnected fixture session')

        def type_field(target, value, field):
            self.click(target)
            self.wait_target(target, focused=True)
            self.key(0)
            self.native('text', value)
            self.wait('Storage field receives exact native input', lambda d:
                      overleaf_state(d)[field] == fingerprint(value)
                      and overleaf_owner_unchanged(d, before))

        with StorageFixture(connection_events=True) as fixture:
            self.open_overleaf_connection()
            type_field('overleaf.url', fixture.url, 'query_fingerprint')
            self.wait_target('overleaf.keep_local_copy')
            if overleaf_state(self.snapshot()['document'])['storage_keep_local_copy']:
                self.click('overleaf.keep_local_copy')
            self.wait('Temporary storage is selected', lambda d:
                      not overleaf_state(d)['storage_keep_local_copy'])
            self.click('overleaf.connect')
            loaded = self.wait('Fresh source appears locked while the asset is pending', lambda d:
                               d['source_fingerprint'] == fingerprint(SOURCE)
                               and overleaf_state(d)['source_locked']
                               and not overleaf_state(d)['sync_ready']
                               and overleaf_state(d)['keep_local_copy'] is False,
                               timeout=20)['document']
            if not fixture.asset_requested.wait(1):
                raise AssertionError('The real bridge never requested the held asset')
            if getattr(self, 'capture_review', False):
                self.capture_viewport('main')
            temporary = Path(loaded['path']).parent
            self.focus_document_window()
            self.click('editor.source')
            self.native('text', 'MUST STAY LOCKED')
            self.stable_state(lambda d: d['source_fingerprint'] == fingerprint(SOURCE)
                              and overleaf_state(d)['source_locked'])
            fixture.release_assets.set()
            self.wait('Every current server file reaches disk before editing is enabled', lambda d:
                      overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked']
                      and d['source_fingerprint'] == fingerprint(SOURCE), timeout=15)
            if (temporary / 'fixture.bin').read_bytes() != ASSET_BYTES:
                raise AssertionError('Ready was reported before the held asset was written exactly')
            if getattr(self, 'capture_review', False):
                self.capture_viewport('main')
            self.click('editor.source')
            self.key(125)
            self.native('text', 'Accepted.')
            self.wait('Native editing works after fresh hydration', lambda d:
                      d['source_fingerprint'] == fingerprint(SOURCE + 'Accepted.'))
            self.key(6)
            self.wait('Undo restores the original server source', lambda d:
                      d['source_fingerprint'] == fingerprint(SOURCE))
            self.wait('The fixture save transaction settles without a retained draft', lambda d:
                      not d['dirty'] and not overleaf_state(d)['pending'], timeout=15)
            # The active Explorer/connection manager also owns a storage lease.
            # Disconnect explicitly, hide the manager, then close its source tab.
            self.open_overleaf_connection()
            if getattr(self, 'review_preview', False):
                self.click('overleaf.show_browser')
                self.review_native_preview('overleaf-storage-browser', document_focus=False)
            self.click('overleaf.disconnect')
            self.wait('Explicit disconnect drops the hosted browser', lambda d:
                      not overleaf_state(d)['browser'] and not overleaf_state(d)['sync_ready'])
            self.click('overleaf.hide')
            self.wait('Hide returns focus to the source window', lambda d:
                      not overleaf_state(d)['visible'] and d['focused'])
            self.click('editor.source')
            self.key(13)
            returned = self.wait('Closing the remote tab returns to the original source owner', lambda d:
                                 all(d[field] == before[field] for field in OWNER_FIELDS if field != 'revision')
                                 and not overleaf_state(d)['connection'], timeout=15)['document']
            # Switching files advances the editor/LSP revision even when the
            # parked document's source and owner are restored exactly.
            before = returned
            deadline = time.monotonic() + 5
            while temporary.exists() and time.monotonic() < deadline:
                time.sleep(.05)
            if temporary.exists():
                raise AssertionError('An unused temporary Overleaf workspace was not deleted')
            self.record('overleaf.temporary_hydration_and_cleanup', remote_account=False,
                        asset_bytes=len(ASSET_BYTES), blocked_edit=True, native_edit=True)

            # The same official bridge path now exercises retained custom storage.
            self.open_overleaf_connection()
            if not overleaf_state(self.snapshot()['document'])['storage_keep_local_copy']:
                self.click('overleaf.keep_local_copy')
            local_base = self.directory / 'kept-overleaf-copies'
            type_field('overleaf.directory', str(local_base), 'storage_directory_fingerprint')
            self.click('overleaf.connect')
            kept = self.wait('The custom persistent copy hydrates and becomes editable', lambda d:
                             d['source_fingerprint'] == fingerprint(SOURCE)
                             and overleaf_state(d)['sync_ready']
                             and overleaf_state(d)['keep_local_copy'] is True,
                             timeout=20)['document']
            persistent = Path(kept['path']).parent
            if persistent.parent != local_base or persistent.name == local_base.name:
                raise AssertionError('The selected folder was not used as a namespaced project base')
            self.overleaf_reconnect(fixture, persistent)
            self.open_overleaf_connection()
            self.click('overleaf.disconnect')
            self.wait('The retained project disconnects without deleting its files', lambda d:
                      not overleaf_state(d)['browser'] and not overleaf_state(d)['sync_ready'])
            self.click('overleaf.hide')
            self.wait('Hide returns focus to the source window', lambda d:
                      not overleaf_state(d)['visible'] and d['focused'])
            self.click('editor.source')
            self.key(13)
            self.wait('Closing the retained project tab restores the original source', lambda d:
                      d['path'] == before['path']
                      and d['source_fingerprint'] == before['source_fingerprint'])
            if not persistent.is_dir() or (persistent / 'main.txt').read_text() != SOURCE:
                raise AssertionError('Closing the tab removed or changed its persistent copy')
            if (persistent / 'fixture.bin').read_bytes() != ASSET_BYTES:
                raise AssertionError('The persistent copy lost its server asset')
            self.record('overleaf.persistent_copy_survives_close', remote_account=False,
                        source_fingerprint=fingerprint(SOURCE))

    def overleaf_chooser_roundtrip(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        remote_root = Path(before['path']).parent
        local_root = self.directory.resolve()
        if not before['dirty']:
            raise AssertionError('Chooser preservation must exercise a real unsaved Overleaf draft')
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Explorer opens for root chooser switching', lambda d: d['explorer_visible'])
        self.double_click('explorer.workspace')
        self.wait('A remote Explorer root opens its Overleaf chooser without dropping the draft', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=True, root=remote_root))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('overleaf')
        self.click('chooser.local')
        self.wait('Local folders returns from Overleaf without disconnecting its browser', lambda d:
                  overleaf_chooser_state(d, before, local=True, remote=False, root=remote_root))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('workspace')
        self.click('chooser.overleaf')
        self.wait('The local chooser returns to Overleaf without reconnecting or editing its draft', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=True, root=remote_root))
        self.click('chooser.local')
        self.wait('The second local switch preserves the connected draft', lambda d:
                  overleaf_chooser_state(d, before, local=True, remote=False, root=remote_root))
        self.click('chooser.workspace.' + fingerprint(str(local_root)))
        self.wait('Selecting a local root retains the active bound Overleaf source and its draft', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=False, root=local_root)
                  and not d['modal_open'])
        self.double_click('explorer.workspace')
        self.wait('A normal Explorer root opens its local chooser while preserving the remote tab', lambda d:
                  overleaf_chooser_state(d, before, local=True, remote=False, root=local_root))
        self.click('chooser.overleaf')
        self.wait('The normal root chooser can return to the existing Overleaf connection', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=True, root=local_root))
        self.click('chooser.local')
        self.wait('The existing connection returns to the local chooser a second time', lambda d:
                  overleaf_chooser_state(d, before, local=True, remote=False, root=local_root))
        self.key(53, 'none')
        self.wait('Escape dismisses only the local chooser without changing root or source', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=False, root=local_root)
                  and not d['modal_open'])
        self.record('overleaf.chooser_roundtrip', remote_account=False, draft_preserved=True,
                    connection_epoch=overleaf_state(before)['epoch'], local_root_selected=True)
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Chooser roundtrip restores Explorer visibility', lambda d: not d['explorer_visible'])
        self.focus_document_window()
        self.click('editor.source')

    def overleaf_replace_source(self, source):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        self.click('editor.source')
        self.key(0)
        self.paste_source(source)
        return self.wait('Native Overleaf source replacement is exact', lambda d:
                         d['source_fingerprint'] == fingerprint(source)
                         and d['source_bytes'] == len(source.encode('utf-8')))['document']

    def overleaf_save_fixture(self, fixture, source):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        self.click('editor.source')
        writes = fixture.save_count
        self.key(1)
        self.wait('Only an acknowledged real bridge save clears the Overleaf draft', lambda d:
                  d['source_fingerprint'] == fingerprint(source) and not d['dirty']
                  and not overleaf_state(d)['pending'] and fixture.source == source
                  and fixture.save_count > writes, timeout=20)

    def overleaf_network_reconnect(self, fixture, local, remote):
        from desktop_ui_journeys import fingerprint
        edited = self.overleaf_replace_source(local)
        if not edited['dirty']:
            raise AssertionError('Reconnect needs a genuinely unsaved local edit')
        writes = fixture.save_count
        if not fixture.connection_requested.wait(2):
            raise AssertionError('The official fixture frontend has not opened its connection watch')
        fixture.lose_connection()
        self.wait('Server-side transport loss retains the unsaved source and locks it', lambda d:
                  overleaf_source_preserved(d, edited)
                  and overleaf_reconcile_state(d)['connection']
                  and not overleaf_state(d)['sync_ready'] and overleaf_state(d)['source_locked']
                  and not overleaf_state(d)['pending'])
        fixture.replace_remote(remote)
        if fixture.save_count != writes:
            raise AssertionError('An unsaved local draft was published during network loss')
        self.open_overleaf_connection()
        self.click('overleaf.connect')
        self.wait('Reconnect finishes fresh hydration without an unconfirmed publication', lambda d:
                  overleaf_reconcile_state(d)['sync_ready']
                  and not overleaf_state(d)['pending'], timeout=20)
        if fixture.source != remote or fixture.save_count != writes:
            raise AssertionError('Reconnect overwrote the server before explicit conflict/merge approval')
        return edited, writes

    def overleaf_reconnect(self, fixture, persistent):
        """Exercise chooser switching and retained drafts using only the loopback server."""
        from overleaf_storage_fixture import SOURCE
        from desktop_ui_journeys import fingerprint
        original_autosave = self.snapshot()['document'].get('auto_save')
        if type(original_autosave) is not bool:
            raise AssertionError('Autosave observation is unavailable')
        if original_autosave:
            self.tex_settings([('"auto_save": true', '"auto_save": false')])
        seed = 'Opening baseline.\nMiddle baseline.\nClosing baseline.\n'
        self.overleaf_replace_source(seed)
        self.overleaf_chooser_roundtrip()
        self.overleaf_save_fixture(fixture, seed)
        local = seed.replace('Opening baseline.', 'Opening local edit.')
        remote = seed.replace('Closing baseline.', 'Closing remote edit.')
        merged = local.replace('Closing baseline.', 'Closing remote edit.')
        edited, _ = self.overleaf_network_reconnect(fixture, local, remote)
        self.wait('Disjoint reconnect edits merge without a conflict or lost local content', lambda d:
                  all(d[field] == edited[field] for field in
                      ('path', 'active_tab_id', 'tabs', 'preview_tab_id', 'preview_path'))
                  and d['source_fingerprint'] == fingerprint(merged) and d['dirty']
                  and overleaf_reconcile_state(d)['blocked_count'] == 0
                  and not overleaf_state(d)['source_locked']
                  and not overleaf_state(d)['review']['open'])
        self.overleaf_save_fixture(fixture, merged)
        self.record('overleaf.disjoint_reconnect', remote_account=False,
                    merged_fingerprint=fingerprint(merged), acknowledged=True)
        local = merged.replace('Middle baseline.', 'Middle local edit.')
        remote = merged.replace('Middle baseline.', 'Middle remote edit.')
        edited, writes = self.overleaf_network_reconnect(fixture, local, remote)
        conflict = self.wait('Overlapping reconnect edits retain the full local draft for explicit review', lambda d:
                             overleaf_source_preserved(d, edited)
                             and overleaf_reconcile_state(d)['blocked_count'] == 1
                             and overleaf_state(d)['source_locked'])['document']
        for cancel in (True, False):
            self.open_overleaf_connection()
            self.click('overleaf.review')
            self.wait('The docked review shows complete remote and local source', lambda d:
                      overleaf_review_pair(d, remote, local) and not overleaf_state(d)['visible']
                      and overleaf_source_preserved(d, conflict))
            overleaf_review_docked(self.snapshot())
            if getattr(self, 'capture_review', False) and cancel:
                self.capture_viewport('main')
            if cancel:
                self.click('overleaf.review.cancel')
                self.wait('Review Cancel retains the exact local draft and conflict', lambda d:
                          not overleaf_reconcile_state(d)['review']['open']
                          and overleaf_state(d)['blocked_count'] == 1
                          and overleaf_source_preserved(d, conflict))
                if fixture.source != remote or fixture.save_count != writes:
                    raise AssertionError('Cancelling review changed the server')
            else:
                self.click('overleaf.review.use_local')
                self.wait('Keeping local resolves the conflict without silently publishing it', lambda d:
                          not overleaf_reconcile_state(d)['review']['open']
                          and overleaf_state(d)['blocked_count'] == 0
                          and not overleaf_state(d)['source_locked']
                          and d['source_fingerprint'] == fingerprint(local) and d['dirty'])
                if fixture.source != remote or fixture.save_count != writes:
                    raise AssertionError('Choosing a local resolution bypassed explicit Save')
                self.overleaf_save_fixture(fixture, local)
        self.record('overleaf.overlap_reconnect', remote_account=False,
                    cancellation_preserved_draft=True, explicit_local_resolution=True,
                    acknowledged=True)
        self.overleaf_replace_source(SOURCE)
        self.overleaf_save_fixture(fixture, SOURCE)
        if (persistent / 'main.txt').read_text() != SOURCE:
            raise AssertionError('Resolved working copy was not saved durably')
        if original_autosave:
            self.tex_settings([('"auto_save": false', '"auto_save": true')])
