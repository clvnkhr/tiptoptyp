"""Explorer file actions through native input and read-only observations."""
from pathlib import Path
import re
import time

EXPLORER_JOURNEYS = ('explorer_files',)
EXPLORER_FOLDER = 'explorer-actions'
EXPLORER_SEED = 'Original explorer seed: α and two lines.\nPreserve these bytes.\n'.encode()
EXPLORER_NESTED = bytes((0, 1, 127, 128, 254, 255))
OWNER_FIELDS = ('path', 'active_tab_id', 'tabs', 'source_fingerprint', 'source_bytes',
                'revision', 'dirty', 'preview_path', 'preview_tab_id', 'cursor')


def prepare_explorer_files(directory):
    folder = Path(directory) / EXPLORER_FOLDER
    (folder / 'nested').mkdir(parents=True)
    (folder / 'seed.txt').write_bytes(EXPLORER_SEED)
    (folder / 'nested' / 'payload.bin').write_bytes(EXPLORER_NESTED)


def file_dialog(document):
    if 'file_dialog' not in document:
        raise AssertionError('File dialog observation is missing')
    dialog = document['file_dialog']
    if dialog is None:
        return None
    if (not isinstance(dialog, dict) or type(dialog.get('create')) is not bool
            or not isinstance(dialog.get('path'), str) or not Path(dialog['path']).is_absolute()
            or not isinstance(dialog.get('name_fingerprint'), str)
            or not re.fullmatch(r'[0-9a-f]{16}', dialog['name_fingerprint'])
            or dialog.get('error') is not None and
            (not isinstance(dialog['error'], str) or not dialog['error'].strip())
            or 'error' not in dialog):
        raise AssertionError('Invalid file dialog observation')
    return dialog


def explorer_owner_unchanged(document, before):
    if any(field not in document or field not in before for field in OWNER_FIELDS):
        raise AssertionError('Explorer source-owner observation is incomplete')
    return all(document[field] == before[field] for field in OWNER_FIELDS)


def current_explorer_target(state, name):
    target = state.get('targets', {}).get(name)
    document = state.get('document', {})
    frame, viewport = document.get('frame'), document.get('viewport')
    return (isinstance(target, dict) and type(frame) is int and frame >= 0
            and isinstance(viewport, str) and bool(viewport) and target.get('viewport') == viewport
            and target.get('enabled') is True and type(target.get('frame')) is int
            and target['frame'] == frame
            and type(target.get('age_ms')) in (float, int) and 0 <= target['age_ms'] <= 500)


def file_bytes(path, expected):
    path = Path(path)
    if path.is_symlink() or not path.is_file() or path.read_bytes() != expected:
        raise AssertionError('Explorer action produced missing or changed file bytes: ' + path.name)


def tree_bytes(folder):
    folder = Path(folder)
    if folder.is_symlink() or not folder.is_dir():
        raise AssertionError('Explorer folder copy is missing or is a symlink')
    entries = {}
    for path in sorted(folder.rglob('*')):
        if path.is_symlink():
            raise AssertionError('Unexpected symlink in Explorer fixture')
        entries[path.relative_to(folder).as_posix()] = None if path.is_dir() else path.read_bytes()
    return entries


class ExplorerJourneys:
    def explorer_file_dialog(self, *, create, path, before):
        self.wait('Explorer opens the expected file prompt without changing source', lambda d:
                  file_dialog(d) is not None and file_dialog(d)['create'] is create
                  and file_dialog(d)['path'] == str(path) and explorer_owner_unchanged(d, before))
        self.wait_target('rename.name', focused=True)
        if getattr(self, 'capture_review', False) and not getattr(self, '_explorer_prompt_captured', False):
            self.capture_viewport('rename')
            self._explorer_prompt_captured = True
        return self.snapshot()['document']

    def explorer_type_name(self, value, before):
        from desktop_ui_journeys import fingerprint
        self.click('rename.name')
        self.wait_target('rename.name', focused=True)
        self.key(0)
        self.native('text', value)
        self.wait('Filename typing remains in its prompt', lambda d:
                  file_dialog(d) is not None
                  and file_dialog(d)['name_fingerprint'] == fingerprint(value)
                  and explorer_owner_unchanged(d, before))

    def explorer_wait_row(self, relative, *, folder=False):
        self.wait_target(('explorer.folder.' if folder else 'explorer.file.') + relative)

    def explorer_menu(self, relative, action, *, folder=False):
        name = ('explorer.folder.' if folder else 'explorer.file.') + relative
        self.explorer_wait_row(relative, folder=folder)
        self.right_click(name)
        self.wait_target('menu.workspace.' + action)
        self.click('menu.workspace.' + action)

    def explorer_files(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Explorer visible', lambda d: d['explorer_visible'])
        folder = self.directory / EXPLORER_FOLDER
        seed = folder / 'seed.txt'
        file_bytes(seed, EXPLORER_SEED)
        file_bytes(folder / 'nested' / 'payload.bin', EXPLORER_NESTED)
        baseline = tree_bytes(folder)
        owner = lambda d: explorer_owner_unchanged(d, before)
        row = 'explorer.folder.' + EXPLORER_FOLDER
        disclosure = 'explorer.disclosure.' + EXPLORER_FOLDER
        child = 'explorer.file.' + EXPLORER_FOLDER + '/seed.txt'
        self.wait_target(row)
        self.click(row)
        self.wait('Selecting a folder preserves its source owner', owner)
        self.wait_target(disclosure)
        # The original arrow vanished only when selected and not hovered.
        self.wait_target('editor.source')
        editor = self.snapshot()['targets']['editor.source']
        self.native('move', editor['x'], editor['y'])
        self.wait('Selected folder remains unchanged with the pointer away', owner)
        if getattr(self, 'capture_review', False):
            self.capture_viewport('main')
        expanded = current_explorer_target(self.snapshot(), child)
        self.click(disclosure)
        self.explorer_wait_visibility(child, not expanded, owner)
        self.click(disclosure)
        self.explorer_wait_visibility(child, expanded, owner)
        if not expanded:
            self.click(disclosure)
            self.explorer_wait_visibility(child, True, owner)
        self.record('explorer.selected_folder_disclosure', owner=before['active_tab_id'])

        # Cancel a typed filename before creating anything.
        self.explorer_menu(EXPLORER_FOLDER, 'new_file', folder=True)
        self.explorer_file_dialog(create=True, path=folder, before=before)
        self.explorer_type_name('cancelled.typ', before)
        self.click('rename.cancel')
        self.wait('Cancel closes New file and preserves source', lambda d: file_dialog(d) is None and owner(d))
        if tree_bytes(folder) != baseline:
            raise AssertionError('Cancel created or changed a workspace artifact')

        # An existing name must never be overwritten, whether validation disables
        # Create immediately or reports the collision after its first click.
        self.explorer_menu(EXPLORER_FOLDER, 'new_file', folder=True)
        self.explorer_file_dialog(create=True, path=folder, before=before)
        self.explorer_type_name('seed.txt', before)
        state = self.snapshot()
        target = state['targets'].get('rename.submit')
        if target and target.get('enabled') is True:
            self.click('rename.submit')
        self.wait('Existing filename reports an error and retains the prompt', lambda d:
                  file_dialog(d) is not None and file_dialog(d)['error'] is not None and owner(d))
        file_bytes(seed, EXPLORER_SEED)
        if tree_bytes(folder) != baseline:
            raise AssertionError('Colliding New file changed the workspace')
        self.click('rename.cancel')
        self.wait('Collision prompt closes', lambda d: file_dialog(d) is None and owner(d))
        self.record('explorer.new_file_cancel_and_collision')

        # Both folder-relative and file-relative creation use the native prompt.
        for relative, is_folder, name in ((EXPLORER_FOLDER, True, 'created.typ'),
                                          (EXPLORER_FOLDER + '/seed.txt', False, 'beside.txt')):
            self.explorer_menu(relative, 'new_file', folder=is_folder)
            opened = self.explorer_file_dialog(create=True, path=folder, before=before)
            if file_dialog(opened)['name_fingerprint'] != fingerprint('untitled.typ'):
                raise AssertionError('New file did not suggest the expected document extension')
            self.explorer_type_name(name, before)
            self.click('rename.submit')
            destination = folder / name
            self.wait('Create opens exactly the new empty document', lambda d:
                      file_dialog(d) is None and d['path'] == str(destination)
                      and d['source_bytes'] == 0 and d['source_fingerprint'] == fingerprint('')
                      and destination.is_file(), timeout=15)
            file_bytes(destination, b'')
            self.click('tab.' + str(before['active_tab_id']))
            self.wait('Original source tab remains unchanged after creation', lambda d:
                      d['active_tab_id'] == before['active_tab_id'] and d['path'] == before['path']
                      and d['source_fingerprint'] == before['source_fingerprint']
                      and d['source_bytes'] == before['source_bytes']
                      and d['dirty'] == before['dirty'])
            # Creation legitimately changes the tab count and can adopt a new
            # typesetting preview; subsequent filesystem actions retain those.
            before = self.snapshot()['document']
            self.explorer_wait_row(EXPLORER_FOLDER + '/' + name)
            self.record('explorer.new_file', relative=name)

        # Duplicate copies first, then the native Rename prompt chooses its name.
        for relative, is_folder, default, renamed in (
                (EXPLORER_FOLDER + '/seed.txt', False, folder / 'seed copy.txt', folder / 'renamed-seed.txt'),
                (EXPLORER_FOLDER, True, self.directory / (EXPLORER_FOLDER + ' copy'), self.directory / 'explorer-duplicated')):
            expected = tree_bytes(folder) if is_folder else EXPLORER_SEED
            self.explorer_menu(relative, 'duplicate', folder=is_folder)
            opened = self.explorer_file_dialog(create=False, path=default, before=before)
            if file_dialog(opened)['name_fingerprint'] != fingerprint(default.name):
                raise AssertionError('Duplicate Rename did not identify the new copy')
            if is_folder:
                if tree_bytes(default) != expected:
                    raise AssertionError('Folder duplicate omitted or changed nested bytes')
            else:
                file_bytes(default, expected)
            self.explorer_type_name(renamed.name, before)
            self.click('rename.submit')
            self.wait('Rename finishes without changing the source owner', lambda d:
                      file_dialog(d) is None and renamed.exists() and not default.exists()
                      and explorer_owner_unchanged(d, before), timeout=15)
            if is_folder:
                if tree_bytes(renamed) != expected or tree_bytes(folder) != expected:
                    raise AssertionError('Renaming a folder copy changed its contents or original')
            else:
                file_bytes(renamed, expected)
                file_bytes(seed, expected)
            self.record('explorer.duplicate_renamed', folder=is_folder, name=renamed.name)
        self.record('explorer.files.complete', source_fingerprint=before['source_fingerprint'])

    def explorer_wait_visibility(self, target, visible, owner):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            if current_explorer_target(state, target) == visible and owner(state['document']):
                return
            time.sleep(.05)
        raise AssertionError('Folder disclosure did not change child visibility without editing source')
