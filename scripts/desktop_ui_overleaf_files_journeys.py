"""Explorer file operations through the real bridge and an authoritative server."""
from pathlib import Path
import math
import re
import time
from desktop_ui_journeys import fingerprint
from desktop_ui_explorer_journeys import current_explorer_target
from desktop_ui_native_scroll import native_scroll_advance
from desktop_ui_overleaf_journeys import overleaf_state, overleaf_preview_ready

TEX = '\\documentclass{article}\n\\begin{document}\nAn editable Overleaf fixture.\n\\end{document}\n'


def overleaf_files_state(document):
    state = overleaf_state(document).get('files')
    required = {'dialog', 'pending', 'writable', 'action', 'name_fingerprint', 'parent_fingerprint', 'error'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(field)) is not bool for field in ('dialog', 'pending', 'writable'))
            or any(state.get(field) is not None and not isinstance(state[field], str)
                   for field in ('action', 'name_fingerprint', 'error'))
            or state['action'] not in (None, 'createfile', 'createfolder', 'duplicate', 'rename', 'delete', 'move')
            or state['name_fingerprint'] is not None and re.fullmatch(r'[0-9a-f]{16}', state['name_fingerprint']) is None
            or state['dialog'] and (state['action'] is None or state['name_fingerprint'] is None)
            or not state['dialog'] and any(state[field] is not None for field in ('action', 'name_fingerprint', 'parent_fingerprint', 'error'))
            or state['parent_fingerprint'] is not None and re.fullmatch(r'[0-9a-f]{16}', state['parent_fingerprint']) is None
            or state['pending'] and not state['dialog']):
        raise AssertionError('Overleaf file-operation observation is missing or malformed')
    return state


def overleaf_file_rows_visible(state, paths):
    """A hydrated model is insufficient: every row needs a current visible hit area."""
    return all(current_explorer_target(state, 'explorer.file.' + path)
               and all(type(state['targets']['explorer.file.' + path].get(field)) in (int, float)
                       and math.isfinite(state['targets']['explorer.file.' + path][field])
                       and state['targets']['explorer.file.' + path][field] > 1
                       for field in ('width', 'height')) for path in paths)


class OverleafFileJourneys:
    def overleaf_scroll_local_files(self):
        deadline = time.monotonic() + 10
        while True:
            state = self.snapshot()
            scroll = state.get('scrolls', {}).get('Files')
            if (scroll and scroll['frame'] == state['document']['frame']
                    and scroll['content_height'] > scroll['viewport_height'] + 100):
                break
            if time.monotonic() >= deadline:
                raise AssertionError('The local Files fixture did not expose a scrollable list')
            time.sleep(0.05)
        # The combined journey has just hidden a native browser child. Claim
        # the root Files section explicitly before sending its wheel event.
        self.native('click', scroll['x'], scroll['y'])
        self.wait('Files owns focus before the local scroll precondition', lambda d:
                  d['focused'] and d['explorer_focused'] == 'Files')
        _, offset, attempts = native_scroll_advance(
            self, 'Files', lines=-24, minimum_advance=0, minimum_offset=80)
        self.record('overleaf.explorer_local_scroll', offset=offset, wheel_attempts=attempts)
        return offset

    def overleaf_wait_file_rows(self, paths):
        deadline = time.monotonic() + 6
        while True:
            state = self.snapshot()
            if overleaf_file_rows_visible(state, paths):
                return state
            if time.monotonic() >= deadline:
                raise AssertionError('Hydrated Overleaf files are not visible without scrolling: ' + ', '.join(paths))
            time.sleep(0.05)

    def overleaf_file_menu(self, path, action, *, folder=False):
        self.focus_document_window()
        target = ('explorer.folder.' if folder else 'explorer.file.') + path
        self.wait_target(target)
        self.right_click(target)
        self.wait_target('menu.overleaf.' + action)
        self.click('menu.overleaf.' + action)
        self.wait('The actual Overleaf file prompt opens', lambda d: overleaf_files_state(d)['dialog'])

    def overleaf_file_name(self, value):
        self.click('overleaf.files.name')
        self.wait_target('overleaf.files.name', focused=True)
        self.key(0)
        self.native('text', value)
        self.wait('Name input belongs to the file prompt', lambda d:
                  overleaf_files_state(d)['name_fingerprint'] == fingerprint(value))

    def overleaf_file_submit(self):
        self.click('overleaf.files.submit')
        self.wait('Overleaf confirms the server-side operation', lambda d:
                  not overleaf_files_state(d)['dialog'] and not overleaf_files_state(d)['pending']
                  and overleaf_state(d)['sync_ready'] and overleaf_state(d)['error'] is None, timeout=25)

    def overleaf_files(self):
        from overleaf_storage_fixture import SOURCE, StorageFixture
        from desktop_ui_journeys import PREVIEW_SOURCE
        from desktop_ui_workspace_switch_journeys import workspace_root_is, workspace_tab_ids
        self.focus_document_window()
        source_path = self.directory / 'journey.typ'
        self.click('editor.source')
        self.key(35, 'cmd+alt')
        self.key(20)
        self.key(15)
        before = self.wait('A real pinned preview remains visible during server file actions', lambda d:
                           overleaf_preview_ready(d, source_path), timeout=60)['document']
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Explorer is visible', lambda d: d['explorer_visible'])
        local_offset = self.overleaf_scroll_local_files()
        with StorageFixture(file_actions=True, preview_source=PREVIEW_SOURCE) as fixture:
            self.open_overleaf_connection()
            self.click('overleaf.url')
            self.wait_target('overleaf.url', focused=True)
            self.key(0)
            self.native('text', fixture.url)
            self.wait('Project URL is entered', lambda d:
                      overleaf_state(d)['query_fingerprint'] == fingerprint(fixture.url))
            if overleaf_state(self.snapshot()['document'])['storage_keep_local_copy']:
                self.click('overleaf.keep_local_copy')
            self.click('overleaf.connect')
            loaded = self.wait('Authoritative Explorer files are editable after hydration', lambda d:
                               d['source_fingerprint'] == fingerprint(SOURCE)
                               and overleaf_state(d)['sync_ready'] and overleaf_files_state(d)['writable'],
                               timeout=25)['document']
            root = Path(loaded['path']).parent
            self.wait('Adopting the remote root retires the previous local file and preview', lambda d:
                      workspace_root_is(d, root)
                      and not set(workspace_tab_ids(d)).intersection(workspace_tab_ids(before)))
            self.overleaf_wait_file_rows(('main.txt', 'fixture.bin'))
            self.record('overleaf.explorer_workspace_handoff', local_offset=local_offset,
                        remote_files_visible=True, native_scroll_after_connection=False)
            project_preview = self.overleaf_pin_workspace_preview(root)
            self.overleaf_file_menu('main.txt', 'new_folder')
            self.overleaf_file_name('notes')
            self.overleaf_file_submit()
            self.wait('A new server folder appears in Explorer', lambda d:
                      fixture.files.entry('notes') is not None and (root / 'notes').is_dir())
            self.wait_target('explorer.folder.notes')
            self.click('explorer.folder.notes')
            self.overleaf_file_menu('notes', 'new_file', folder=True)
            self.overleaf_file_name('draft.tex')
            self.overleaf_file_submit()
            self.wait('A new TeX document opens from the server tree', lambda d:
                      d['path'] == str(root / 'notes/draft.tex') and d['document_kind'] == 'Tex'
                      and d['source_bytes'] == 0, timeout=20)
            created = fixture.files.entry('notes/draft.tex')
            if created is None or created['source'] != '':
                raise AssertionError('New TeX document did not come from an empty server document')
            self.overleaf_replace_source(TEX)
            self.key(1)
            self.wait('The edited TeX file saves through OT and HTTP confirmation', lambda d:
                      not d['dirty'] and not overleaf_state(d)['pending']
                      and fixture.files.source(created['id']) == TEX, timeout=25)
            self.overleaf_file_menu('notes/draft.tex', 'duplicate')
            self.overleaf_file_name('copy.tex')
            self.overleaf_file_submit()
            copied = fixture.files.entry('notes/copy.tex')
            if copied is None or copied['id'] == created['id'] or copied['source'] != TEX:
                raise AssertionError('Duplicate did not create an independent server-confirmed document')
            self.wait('The independent copy opens with its complete source', lambda d:
                      d['path'] == str(root / 'notes/copy.tex')
                      and d['source_fingerprint'] == fingerprint(TEX), timeout=20)
            self.overleaf_file_menu('notes/copy.tex', 'rename')
            self.overleaf_file_name('renamed.txt')
            self.overleaf_file_submit()
            self.wait('Rename keeps server identity and source while changing the language', lambda d:
                      fixture.files.entry('notes/renamed.txt') is not None
                      and fixture.files.entry('notes/renamed.txt')['id'] == copied['id']
                      and d['path'] == str(root / 'notes/renamed.txt') and d['source_fingerprint'] == fingerprint(TEX)
                      and d['document_kind'] == 'Text')
            self.overleaf_file_menu('notes/renamed.txt', 'rename')
            self.overleaf_file_name('renamed.tex')
            self.overleaf_file_submit()
            self.wait('Renaming back restores TeX editing without replacing its source', lambda d:
                      fixture.files.entry('notes/renamed.tex') is not None
                      and fixture.files.entry('notes/renamed.tex')['id'] == copied['id']
                      and d['path'] == str(root / 'notes/renamed.tex') and d['source_fingerprint'] == fingerprint(TEX)
                      and d['document_kind'] == 'Tex')
            self.overleaf_file_menu('notes/renamed.tex', 'move')
            self.click('overleaf.files.parent')
            self.wait_target('overleaf.files.folder.root')
            self.click('overleaf.files.folder.root')
            self.overleaf_file_submit()
            self.wait('Move changes only the copy location', lambda d:
                      fixture.files.entry('renamed.tex') is not None
                      and fixture.files.entry('renamed.tex')['id'] == copied['id']
                      and fixture.files.entry('notes/renamed.tex') is None
                      and d['path'] == str(root / 'renamed.tex') and d['source_fingerprint'] == fingerprint(TEX))
            self.overleaf_file_menu('notes/draft.tex', 'delete')
            self.overleaf_file_submit()
            self.wait('Delete is reflected by the server and mirror', lambda d:
                      fixture.files.entry('notes/draft.tex') is None and not (root / 'notes/draft.tex').exists())
            self.overleaf_file_menu('notes', 'delete', folder=True)
            self.overleaf_file_submit()
            self.wait('Deleting an empty server folder removes the mirrored folder', lambda d:
                      fixture.files.entry('notes') is None and not (root / 'notes').exists())
            if fixture.files.source(copied['id']) != TEX or fixture.source != SOURCE:
                raise AssertionError('File operations modified an unrelated source')
            self.wait('The current workspace pinned Tinymist preview remains ready', lambda d:
                      d['preview_tab_id'] == project_preview['preview_tab_id']
                      and d['preview_path'] == project_preview['preview_path']
                      and d['preview_backend'] == project_preview['preview_backend'] and d['preview_native_ready']
                      and d['preview_surface_visible'] and d['preview_pages'] == 3, timeout=30)
            self.record('overleaf.explorer_file_actions', remote_account=False, server_authoritative=True,
                        new_folder=True, new_tex=True, edit_and_save=True, duplicate=True,
                        independent_ids=True, rename=True, language_switch=True, move=True, delete_file=True, delete_folder=True,
                        old_workspace_pages_closed=True, current_workspace_preview_preserved=True)
            retired = workspace_tab_ids(self.snapshot()['document'])
            self.open_overleaf_connection()
            self.click('overleaf.disconnect')
            self.click('overleaf.hide')
            self.focus_document_window()
            self.overleaf_restore_local_workspace(before, retired)
