"""Native workspace replacement with exact source and tab ownership checks."""
import json
from pathlib import Path

WORKSPACE_SWITCH_JOURNEYS = ('workspace_switch',)


def _fingerprint(value):
    from desktop_ui_journeys import fingerprint
    return fingerprint(value)


def workspace_tab_ids(document):
    ids = document.get('tab_order')
    if (not isinstance(ids, list) or any(type(item) is not int or item < 1 for item in ids)
            or len(ids) != len(set(ids)) or type(document.get('tabs')) is not int
            or document['tabs'] != len(ids)):
        raise AssertionError('Workspace tab ownership inspection is unavailable')
    return ids


def workspace_owner_unchanged(document, before):
    fields = ('path', 'active_tab_id', 'preview_tab_id', 'preview_path', 'source_fingerprint',
              'source_bytes', 'cursor', 'dirty', 'workspace_root_fingerprint')
    if any(field not in item for item in (document, before) for field in fields):
        raise AssertionError('Workspace source ownership inspection is unavailable')
    for item in (document, before):
        if (type(item['source_bytes']) is not int or item['source_bytes'] < 0
                or type(item['dirty']) is not bool
                or not isinstance(item['source_fingerprint'], str)
                or not isinstance(item['workspace_root_fingerprint'], str)):
            raise AssertionError('Workspace source ownership inspection is malformed')
    return (workspace_tab_ids(document) == workspace_tab_ids(before)
            and all(document[field] == before[field] for field in fields))


def workspace_root_is(document, root):
    value = document.get('workspace_root_fingerprint')
    if not isinstance(value, str):
        raise AssertionError('Workspace root inspection is unavailable')
    return value == _fingerprint(str(root))


def workspace_has_only_destination(document, root, path, source, retired):
    ids = workspace_tab_ids(document)
    return (workspace_root_is(document, root) and len(ids) == 1
            and not set(ids).intersection(retired)
            and document.get('active_tab_id') == ids[0]
            and document.get('path') == str(path)
            and document.get('source_fingerprint') == _fingerprint(source)
            and document.get('source_bytes') == len(source.encode('utf-8'))
            and document.get('dirty') is False and not document.get('modal_open'))


class WorkspaceSwitchJourneys:
    def choose_workspace(self, root):
        self.key(31, 'cmd+shift')
        self.wait('Local workspace chooser opens', lambda d: d['workspace_chooser_visible'])
        target = 'chooser.workspace.' + _fingerprint(str(root))
        self.wait_target(target)
        self.click(target)

    def workspace_switch(self):
        from desktop_ui_journeys import empty_workspace_ready
        old_root = self.directory.resolve()
        destination = old_root / 'workspace-destination'
        destination.mkdir()
        first = old_root / 'workspace-first.txt'
        second = old_root / 'workspace-second.txt'
        destination_file = destination / 'destination.txt'
        first_saved = 'First saved workspace document.\n'
        first_dirty = first_saved + 'Unsaved parked draft.'
        second_saved = 'Second saved workspace document.\n'
        destination_saved = 'Destination saved document.\n'
        destination_dirty = destination_saved + 'Saved during the workspace change.'
        first.write_text(first_saved, encoding='utf-8')
        second.write_text(second_saved, encoding='utf-8')
        destination_file.write_text(destination_saved, encoding='utf-8')

        # Make the real folder appear in the normal chooser through Settings.
        # The launch root is already in this nonempty history, so prepending an
        # entry preserves it without editing settings on disk or native state.
        recent_prefix = '"recent_workspaces": ['
        self.tex_settings([
            ('"auto_save": true', '"auto_save": false'),
            (recent_prefix, recent_prefix + json.dumps(str(destination)) + ',')])
        self.open_harper_fixture(first)
        self.key(125, 'cmd')
        self.native('text', 'Unsaved parked draft.')
        self.source_is(first_dirty)
        self.wait('First workspace tab is dirty', lambda d: d['dirty'])
        self.open_harper_fixture(second)
        self.source_is(second_saved)
        before = self.snapshot()['document']
        retired = workspace_tab_ids(before)
        if len(retired) < 3 or before['dirty']:
            raise AssertionError('Workspace switch needs a clean active tab and a parked dirty tab')

        self.choose_workspace(destination)
        self.wait('Switching prompts for the parked dirty source before changing roots', lambda d:
                  d['modal_open'] and workspace_root_is(d, old_root)
                  and workspace_tab_ids(d) == retired)
        self.wait_target('modal.cancel')
        self.click('modal.cancel')
        self.wait('Cancel restores the exact original workspace, active tab and selection', lambda d:
                  not d['modal_open'] and workspace_owner_unchanged(d, before))
        self.key(48, 'ctrl+shift')
        self.wait('Cancelled switch retains the parked draft', lambda d:
                  d['path'] == str(first) and d['dirty']
                  and d['source_fingerprint'] == _fingerprint(first_dirty))
        self.key(48, 'ctrl')
        self.wait('The original active source remains available after Cancel', lambda d:
                  workspace_owner_unchanged(d, before))
        if first.read_text(encoding='utf-8') != first_saved:
            raise AssertionError('Cancelling the switch wrote the parked draft to disk')

        self.choose_workspace(destination)
        self.wait('A repeated switch still prompts for the dirty tab', lambda d: d['modal_open'])
        self.wait_target('modal.discard')
        self.click('modal.discard')
        self.wait('Discard switches to an empty destination and closes every old-owned tab', lambda d:
                  workspace_root_is(d, destination) and empty_workspace_ready(d)
                  and not d['modal_open'])
        if first.read_text(encoding='utf-8') != first_saved:
            raise AssertionError('Discard overwrote the old workspace file')
        if second.read_text(encoding='utf-8') != second_saved:
            raise AssertionError('Switching rewrote the old clean file')
        self.record('workspace.switch.discard', parked_dirty_prompt=True,
                    cancel_restores_owner=True, all_old_tabs_closed=True,
                    empty_destination=True, discarded_disk_unchanged=True)

        self.open_harper_fixture(destination_file)
        self.key(125, 'cmd')
        self.native('text', 'Saved during the workspace change.')
        self.source_is(destination_dirty)
        self.wait('Destination document has a real unsaved edit', lambda d: d['dirty'])
        destination_before = self.snapshot()['document']
        destination_ids = workspace_tab_ids(destination_before)
        self.choose_workspace(destination)
        self.wait('Selecting the current canonical root keeps dirty tabs without a prompt', lambda d:
                  not d['workspace_chooser_visible'] and not d['modal_open']
                  and workspace_owner_unchanged(d, destination_before))

        # The destination is nested inside the old root. Ownership must win over
        # path containment: returning to the parent still closes its owned tab.
        self.choose_workspace(old_root)
        self.wait('Leaving the destination prompts before changing its owning workspace', lambda d:
                  d['modal_open'] and workspace_root_is(d, destination)
                  and workspace_tab_ids(d) == destination_ids)
        self.wait_target('modal.primary')
        self.click('modal.primary')
        returned = self.wait('Save completes before the old-owned destination tab closes', lambda d:
                            workspace_root_is(d, old_root) and not d['modal_open']
                            and len(workspace_tab_ids(d)) == 1
                            and not set(workspace_tab_ids(d)).intersection(destination_ids)
                            and d['path'] in (str(first), str(second), str(old_root / 'journey.txt'))
                            and not d['dirty']
                            and destination_file.read_text(encoding='utf-8') == destination_dirty,
                            timeout=20)['document']
        restored_path = Path(returned['path'])
        if returned['source_fingerprint'] != _fingerprint(restored_path.read_text(encoding='utf-8')):
            raise AssertionError('Returning to a workspace restored a discarded draft instead of its saved file')

        self.choose_workspace(destination)
        self.wait('Revisiting a workspace restores only its saved remembered document', lambda d:
                  workspace_has_only_destination(d, destination, destination_file,
                                                 destination_dirty, retired + destination_ids), timeout=20)
        if first.read_text(encoding='utf-8') != first_saved or second.read_text(encoding='utf-8') != second_saved:
            raise AssertionError('A later workspace visit rewrote an old source file')
        self.record('workspace.switch.save', same_root_keeps_tabs=True,
                    save_precedes_close=True, parent_root_closes_nested_owned_tab=True,
                    remembered_destination_only=True, saved_bytes_match=True)
