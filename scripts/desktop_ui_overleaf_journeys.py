"""Offline Overleaf connection controls through native input; no remote writes."""
from pathlib import Path
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
        before = self.snapshot()['document']
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

        with StorageFixture() as fixture:
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
