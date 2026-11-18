"""Offline Overleaf connection controls through native input; no remote writes."""
from pathlib import Path
import math
import re

OVERLEAF_JOURNEYS = ('overleaf',)
LOGIN_URL = 'https://www.overleaf.com/login'
INVALID_SCRIPT = 'javascript:alert(1)'
INVALID_FILE = 'file:///private/tmp/fixture.tex'
INVALID_CREDENTIALS = 'https://user:password@overleaf.invalid/login'
# A syntactically valid project on a reserved, nonexistent host. Invalid local
# settings must reject this before allocating any connection or storage worker.
STORAGE_PROJECT = 'https://overleaf.invalid/project/0123456789abcdef01234567'
RELATIVE_DIRECTORY = 'relative-overleaf-copies'
LONG_PROJECT_NAME = ('Collaborative mathematics — 非線形 equations, long-time estimates, '
                     'and a deliberately long project name for the workspace header')
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
             'storage_preparing', 'storage_keep_local_copy', 'storage_controls_visible', 'storage_directory_visible')
    required = {*flags, 'query_fingerprint', 'browser_url_fingerprint', 'browser_start_url_fingerprint', 'storage_directory_fingerprint', 'error', 'epoch'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(field)) is not bool for field in flags)
            or type(state['epoch']) is not int or state['epoch'] < 0
            or not isinstance(state['query_fingerprint'], str)
            or re.fullmatch(r'[0-9a-f]{16}', state['query_fingerprint']) is None
            or not isinstance(state['storage_directory_fingerprint'], str)
            or re.fullmatch(r'[0-9a-f]{16}', state['storage_directory_fingerprint']) is None
            or state['storage_directory_visible'] !=
            (state['visible'] and state['storage_controls_visible'] and state['storage_keep_local_copy'])
            or any(state[field] is not None and
                   (not isinstance(state[field], str) or re.fullmatch(r'[0-9a-f]{16}', state[field]) is None)
                   for field in ('browser_url_fingerprint', 'browser_start_url_fingerprint'))
            or state['error'] is not None and
            (not isinstance(state['error'], str) or not state['error'].strip())
            or state['pending'] and not state['connection']):
        raise AssertionError('Overleaf connection inspection is missing or malformed')
    return state


def overleaf_follow_state(document):
    """Presence actions require explicit state; missing follow data never means stopped."""
    state = overleaf_state(document)
    required = ('following_peer', 'seeking_peer', 'following_head', 'collaborator_count', 'cursor_count')
    if (not all(field in state for field in required)
            or any(state[field] is not None and
                   (not isinstance(state[field], str) or not state[field].strip() or len(state[field]) > 128)
                   for field in ('following_peer', 'seeking_peer'))
            or state['following_head'] is not None and
               (type(state['following_head']) is not int or state['following_head'] < 0)
            or any(type(state[field]) is not int or state[field] < 0
                   for field in ('collaborator_count', 'cursor_count'))
            or state['following_peer'] is None and state['following_head'] is not None):
        raise AssertionError('Overleaf cursor following inspection is missing or malformed')
    return state


def overleaf_peer_closed(document, peer, main):
    """Closing a peer retires its page; neighboring tabs need not be the main file."""
    from desktop_ui_journeys import PREVIEW_SOURCE, fingerprint
    from desktop_ui_workspace_switch_journeys import workspace_tab_ids
    orders = [workspace_tab_ids(item) for item in (document, peer, main)]
    for item, order in zip((document, peer, main), orders):
        overleaf_source_preserved(item, item)
        if (type(item['active_tab_id']) is not int or item['active_tab_id'] not in order
                or type(item['preview_tab_id']) is not int or item['preview_tab_id'] not in order
                or not isinstance(item['preview_path'], str) or not item['preview_path']
                or not isinstance(item.get('workspace_root_fingerprint'), str)
                or re.fullmatch(r'[0-9a-f]{16}', item['workspace_root_fingerprint']) is None):
            raise AssertionError('Peer-close tab ownership evidence is incomplete')
    current_ids, peer_ids, main_ids = orders
    peer_id, main_id, preview_id = peer['active_tab_id'], main['active_tab_id'], main['preview_tab_id']
    if (peer_id in main_ids or main_id == preview_id
            or peer_ids != main_ids + [peer_id]):
        raise AssertionError('Peer-close fixture lacks independent main, preview and peer owners')
    active_is_main = document['active_tab_id'] == main_id
    return (current_ids == [tab for tab in peer_ids if tab != peer_id]
            and document['active_tab_id'] in (main_id, preview_id)
            and all(item['workspace_root_fingerprint'] == main['workspace_root_fingerprint']
                    and item['preview_tab_id'] == preview_id and item['preview_path'] == main['preview_path']
                    for item in (document, peer))
            and document['path'] == (main['path'] if active_is_main else main['preview_path'])
            and document['source_fingerprint'] == (main['source_fingerprint'] if active_is_main
                                                   else fingerprint(PREVIEW_SOURCE))
            and document['source_bytes'] == (main['source_bytes'] if active_is_main
                                             else len(PREVIEW_SOURCE.encode('utf-8')))
            and not document['dirty'] and overleaf_follow_state(document)['following_peer'] is None)


def overleaf_follow_caret(snapshot):
    """A fresh painted caret proves the followed position is actually in the viewport."""
    document = snapshot['document']
    state = overleaf_follow_state(document)
    target = snapshot.get('targets', {}).get('overleaf.cursor.fixture-peer')
    if (state['following_peer'] is None or state['following_head'] is None
            or not isinstance(target, dict) or target.get('enabled') is not True
            or target.get('viewport') != document.get('viewport')
            or type(target.get('frame')) is not int or target['frame'] != document.get('frame')
            or type(target.get('age_ms')) not in (int, float) or not 0 <= target['age_ms'] <= 500
            or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                   for key in ('x', 'y', 'width', 'height'))
            or target['width'] <= 0 or target['height'] <= 0):
        raise AssertionError('Followed collaborator caret is not freshly visible in the root editor')
    return True


def overleaf_owner_unchanged(document, before):
    if any(field not in document or field not in before for field in OWNER_FIELDS):
        raise AssertionError('Overleaf journey lacks complete source-owner evidence')
    return all(document[field] == before[field] for field in OWNER_FIELDS)


def overleaf_browser_at(document, url):
    from desktop_ui_journeys import fingerprint
    state = overleaf_state(document)
    return (state['browser'] and not state['create_browser']
            and state['browser_url_fingerprint'] == fingerprint(url))


def overleaf_browsing(document, before, *, visible, query=None, url=None, error=False):
    """A real browsing window need not have a selected or connected project."""
    from desktop_ui_journeys import fingerprint
    state = overleaf_state(document)
    return (overleaf_owner_unchanged(document, before)
            and state['visible'] is visible and state['browser']
            and state['browser_url_fingerprint'] is not None
            and not any(state[flag] for flag in
                        ('connection', 'create_browser', 'pending', 'storage_preparing'))
            and state['epoch'] == 0
            and (query is None or state['query_fingerprint'] == fingerprint(query))
            and (url is None or overleaf_browser_at(document, url))
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


def overleaf_session_preserved(document, before, *, reconnected=False):
    """Storage drafts cannot replace the bound workspace or its open source owner."""
    state, previous = overleaf_reconcile_state(document), overleaf_reconcile_state(before)
    for item in (document, before):
        root = item.get('workspace_root_fingerprint')
        if not isinstance(root, str) or re.fullmatch(r'[0-9a-f]{16}', root) is None:
            raise AssertionError('Overleaf workspace ownership observation is unavailable')
    if any(type(item.get('keep_local_copy')) is not bool for item in (state, previous)):
        raise AssertionError('Overleaf active storage mode observation is unavailable')
    return (overleaf_source_preserved(document, before)
            and document['workspace_root_fingerprint'] == before['workspace_root_fingerprint']
            and previous['connection'] and previous['browser']
            and state['connection'] and state['browser'] and not state['storage_preparing']
            and not state['pending'] and state['keep_local_copy'] == previous['keep_local_copy']
            and (state['epoch'] > previous['epoch'] if reconnected
                 else state['epoch'] == previous['epoch']))


def overleaf_profile(state):
    profile = state.get('profile_fingerprint')
    if not isinstance(profile, str) or re.fullmatch(r'[0-9a-f]{16}', profile) is None:
        raise AssertionError('Exact Overleaf browser-profile observation is unavailable')
    return profile


def overleaf_staged_workspace(document, before, *, query, directory_fingerprint,
                               keep=True, profile=None):
    """A new workspace login pauses the retained account under a fresh profile."""
    from desktop_ui_journeys import fingerprint
    state, previous = overleaf_state(document), overleaf_state(before)
    actual_profile = overleaf_profile(state)
    original_profile = overleaf_profile(previous)
    creation = workspace_creation_state(document)
    return (overleaf_session_preserved(document, before)
            and not document['workspace_chooser_visible'] and state['visible']
            and not creation['visible'] and not creation['busy'] and not document['modal_open']
            and actual_profile != original_profile
            and (profile is None or actual_profile == profile)
            and state.get('online') is False and state['sync_ready'] is False
            and state['source_locked'] is True and not state['create_browser']
            and state['error'] is None
            and state['browser_start_url_fingerprint'] == fingerprint(LOGIN_URL)
            and state['browser_url_fingerprint'] == fingerprint(LOGIN_URL)
            and state['query_fingerprint'] == fingerprint(query)
            and state['storage_keep_local_copy'] is keep
            and (not keep or (state['storage_controls_visible'] is True
                              and state['storage_directory_visible'] is True))
            and state['storage_directory_fingerprint'] == directory_fingerprint)


def overleaf_workspace_reconnected(document, before):
    """Restoring the online account reuses its profile and advances one epoch."""
    state, previous = overleaf_state(document), overleaf_state(before)
    return (overleaf_session_preserved(document, before, reconnected=True)
            and state['epoch'] == previous['epoch'] + 1
            and overleaf_profile(state) == overleaf_profile(previous)
            and not document['workspace_chooser_visible'] and not state['visible']
            and document.get('focused') is True and document.get('editor_keyboard_focused') is True
            and not document['modal_open']
            and state.get('online') is True and state['sync_ready'] is True
            and state['source_locked'] is False and not state['create_browser']
            and state['error'] is None
            and state['query_fingerprint'] == previous['query_fingerprint']
            and state['storage_keep_local_copy'] == previous['storage_keep_local_copy']
            and state['storage_directory_fingerprint'] == previous['storage_directory_fingerprint'])


def overleaf_edit_rejection(document, before, *, visible):
    """A rejected edit acknowledges the attempt without changing owner or focus."""
    feedback = document.get('read_only_rejection')
    if not isinstance(feedback, dict) or type(feedback.get('visible')) is not bool:
        raise AssertionError('Painted read-only rejection feedback is unavailable')
    return (feedback['visible'] is visible and overleaf_reconcile_state(document)['source_locked']
            and document.get('focused') is True and document.get('editor_keyboard_focused') is True
            and overleaf_source_preserved(document, before))


def overleaf_header_geometry(snapshot, *, present, reference=None):
    """A status column must stay thin and separate even when its name is elided."""
    document = snapshot.get('document', {})
    frame, viewport, scale = (document.get(field) for field in ('frame', 'viewport', 'interface_scale'))
    if (type(frame) is not int or frame < 0 or not isinstance(viewport, str)
            or type(scale) not in (int, float) or not math.isfinite(scale) or scale <= 0):
        raise AssertionError('Workspace header frame/scale observation is unavailable')
    targets = snapshot.get('targets', {})
    if not isinstance(targets, dict):
        raise AssertionError('Workspace header targets observation is unavailable')

    def current(target):
        return (isinstance(target, dict) and type(target.get('frame')) is int
                and target['frame'] == frame and target.get('viewport') == viewport)

    def geometry(target):
        if (not current(target) or target.get('enabled') is not True
                or type(target.get('age_ms')) not in (int, float) or not 0 <= target['age_ms'] <= 500
                or any(type(target.get(field)) not in (int, float) or not math.isfinite(target[field])
                       for field in ('x', 'y', 'width', 'height'))
                or target['width'] <= 0 or target['height'] <= 0):
            raise AssertionError('Workspace header target is missing, stale, or malformed')
        return tuple(target[field] for field in ('x', 'y', 'width', 'height'))

    name = geometry(targets.get('explorer.workspace'))
    status = targets.get('explorer.overleaf_status')
    if not present:
        if current(status):
            raise AssertionError('The local workspace incorrectly reserves an Overleaf status column')
        return None
    indicator = geometry(status)
    x, y, width, height = indicator
    name_x, name_y, name_width, name_height = name
    tolerance = 0.6 * scale
    gap = x - width / 2 - (name_x + name_width / 2)
    if (abs(width - 12 * scale) > tolerance or abs(y - name_y) > tolerance
            or abs(height - name_height) > tolerance or gap < 4 * scale - tolerance):
        raise AssertionError('The Overleaf status column overlaps the name or loses its thin row alignment')
    if reference is not None and (abs(x - reference[0]) > tolerance
                                  or abs(width - reference[2]) > tolerance):
        raise AssertionError('The Overleaf status column moved or resized during a status change')
    return indicator


def overleaf_authentication_state(document, before, *, url, phase, epoch_delta):
    """Authentication must retain the exact dirty owner until fresh disk hydration."""
    if type(epoch_delta) is not int or epoch_delta not in (1, 2):
        raise AssertionError('Authentication requires an exact one- or two-epoch transition')
    state, previous = overleaf_reconcile_state(document), overleaf_reconcile_state(before)
    if any(type(item.get(field)) is not bool for item in (state, previous)
           for field in ('needs_signin', 'online')):
        raise AssertionError('Overleaf authentication observation is unavailable')
    expected = {'signin': (True, False, False, True),
                'hydrating': (False, True, False, True),
                'ready': (False, True, True, False)}[phase]
    actual = tuple(state[field] for field in ('needs_signin', 'online', 'sync_ready', 'source_locked'))
    return (previous['online'] and previous['sync_ready'] and not previous['source_locked']
            and not previous['needs_signin']
            and overleaf_session_preserved(document, before, reconnected=True)
            and state['epoch'] == previous['epoch'] + epoch_delta
            and overleaf_browser_at(document, url) and actual == expected
            and (phase != 'ready' or state['error'] is None))


def overleaf_native_browser_rect(document):
    """Use the measured native WebView for controlled local HTML button clicks."""
    state = overleaf_state(document)
    keyboard = state.get('browser_keyboard')
    rect = keyboard.get('web_screen_rect_top_left') if isinstance(keyboard, dict) else None
    if (state.get('show_browser') is not True or not state['visible']
            or not isinstance(keyboard, dict) or keyboard.get('window_visible') is not True
            or keyboard.get('window_key') is not True
            or not isinstance(rect, list) or len(rect) != 4
            or any(type(value) not in (int, float) or not math.isfinite(value) for value in rect)
            or rect[2] <= 220 or rect[3] <= 110):
        raise AssertionError('Native browser bounds are unavailable for the authentication form')
    return rect


def overleaf_page_departed(document, before):
    """A committed departure must close editing without losing its bound draft."""
    state, previous = overleaf_reconcile_state(document), overleaf_reconcile_state(before)
    if any(type(value.get('online')) is not bool for value in (state, previous)):
        raise AssertionError('Overleaf online observation is unavailable')
    return (previous['connection'] and previous['browser'] and previous['online']
            and previous['sync_ready'] and not previous['source_locked']
            and state['connection'] and state['browser'] and not state['online']
            and not state['sync_ready'] and state['source_locked'] and not state['pending']
            and state['epoch'] == previous['epoch']
            and overleaf_source_preserved(document, before))


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
            and document['workspace_root_fingerprint'] == before['workspace_root_fingerprint']
            and all(state[field] == previous[field] for field in
                    ('connection', 'browser', 'create_browser', 'pending', 'epoch', 'sync_ready'))
            and state['connection'] and state['sync_ready'])


def workspace_creation_state(document):
    """Creation must expose actual draft/worker state, including failed submissions."""
    state = document.get('workspace_creation')
    flags = ('visible', 'busy', 'use_overleaf')
    hashes = ('name_fingerprint', 'parent_fingerprint')
    if (not isinstance(state, dict) or not all(field in state for field in (*flags, *hashes, 'error'))
            or any(type(state[field]) is not bool for field in flags)
            or any(state[field] is not None and
                   (not isinstance(state[field], str) or re.fullmatch(r'[0-9a-f]{16}', state[field]) is None)
                   for field in hashes)
            or state['visible'] and any(state[field] is None for field in hashes)
            or state['error'] is not None and
               (not isinstance(state['error'], str) or not state['error'].strip())):
        raise AssertionError('New workspace creation inspection is missing or malformed')
    return state


def workspace_creation_options_hidden(snapshot):
    """Checking Overleaf keeps project selection in the normal Overleaf window."""
    state = workspace_creation_state(snapshot.get('document', {}))
    targets = snapshot.get('targets', {})
    name = targets.get('workspace.name')
    if (not state['visible'] or not state['use_overleaf']
            or not isinstance(name, dict) or name.get('enabled') is not True
            or type(name.get('frame')) is not int or name['frame'] < 0
            or not isinstance(name.get('viewport'), str) or not name['viewport']
            or type(name.get('age_ms')) not in (int, float) or not 0 <= name['age_ms'] <= 500):
        return False
    checkbox = targets.get('workspace.overleaf')
    if (not isinstance(checkbox, dict) or checkbox.get('enabled') is not True
            or checkbox.get('frame') != name['frame'] or checkbox.get('viewport') != name['viewport']
            or type(checkbox.get('age_ms')) not in (int, float) or not 0 <= checkbox['age_ms'] <= 500):
        return False
    project = targets.get('workspace.project_url')
    return not (isinstance(project, dict) and project.get('frame') == name['frame']
                and project.get('viewport') == name['viewport'])


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


def overleaf_projectless_storage(document, before, *, query, directory, controls):
    """An offline folder can be staged before choosing any project or replacing the local root."""
    state = overleaf_state(document)
    for item in (document, before):
        root = item.get('workspace_root_fingerprint')
        if not isinstance(root, str) or re.fullmatch(r'[0-9a-f]{16}', root) is None:
            raise AssertionError('Projectless storage workspace inspection is unavailable')
    return (overleaf_browsing(document, before, visible=True, query=query)
            and document['workspace_root_fingerprint'] == before['workspace_root_fingerprint']
            and state['error'] is None and state['storage_controls_visible'] is controls
            and state['storage_directory_visible'] is controls
            and overleaf_storage(document, keep=True, directory=directory))


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


def overleaf_storage_draft_ready(snapshot, before, *, query, keep):
    """A URL losing focus cannot retire its storage controls before the next click."""
    from desktop_ui_journeys import fingerprint
    document = snapshot.get('document', {})
    state = overleaf_state(document)
    if (not overleaf_owner_unchanged(document, before)
            or state['query_fingerprint'] != fingerprint(query)
            or not state['visible'] or not state['storage_controls_visible']
            or state['storage_keep_local_copy'] is not keep
            or not overleaf_storage_targets(snapshot, keep=keep)):
        return False
    address = snapshot.get('targets', {}).get('overleaf.url')
    checkbox = snapshot['targets']['overleaf.keep_local_copy']
    return (isinstance(address, dict) and address.get('enabled') is True
            and type(address.get('frame')) is int and address['frame'] == checkbox['frame']
            and address.get('viewport') == checkbox['viewport']
            and type(address.get('age_ms')) in (int, float) and 0 <= address['age_ms'] <= 500)


def overleaf_pending_storage_targets(snapshot, *, pending):
    """Apply/Reset must appear together in the current storage controls frame."""
    state = overleaf_state(snapshot.get('document', {}))
    if (not state['visible'] or not state['storage_controls_visible']
            or not overleaf_storage_targets(snapshot, keep=state['storage_keep_local_copy'])):
        return False
    checkbox = snapshot['targets']['overleaf.keep_local_copy']
    for name in ('overleaf.apply_storage', 'overleaf.reset_storage'):
        target = snapshot['targets'].get(name)
        current = (isinstance(target, dict) and type(target.get('frame')) is int
                   and target['frame'] == checkbox['frame']
                   and target.get('viewport') == checkbox['viewport']
                   and type(target.get('age_ms')) in (int, float)
                   and 0 <= target['age_ms'] <= 500)
        if current != pending or current and target.get('enabled') is not True:
            return False
    return True


class OverleafJourneys:
    def overleaf_pin_workspace_preview(self, root):
        """Open the real hydrated project document through its Explorer row."""
        from desktop_ui_journeys import PREVIEW_SOURCE, fingerprint
        preview = root / 'preview.typ'
        if not preview.is_file() or preview.read_text(encoding='utf-8') != PREVIEW_SOURCE:
            raise AssertionError('The authoritative preview document was not hydrated exactly')
        previous = self.snapshot()['document']['path']
        if not self.snapshot()['document']['explorer_visible']:
            self.key(18)
            self.wait('Explorer opens for the hydrated project preview', lambda d: d['explorer_visible'])
        self.wait_target('explorer.file.preview.typ')
        self.double_click('explorer.file.preview.typ')
        self.wait('Explorer opens the server-owned Typst document', lambda d:
                  d['path'] == str(preview) and d['source_fingerprint'] == fingerprint(PREVIEW_SOURCE))
        self.click('editor.source')
        self.key(35, 'cmd+alt')
        self.key(20)
        self.key(15)
        pinned = self.wait('The current project owns a ready real Typst preview', lambda d:
                           overleaf_preview_ready(d, preview), timeout=60)['document']
        self.key(48, 'ctrl+shift')
        self.wait('The bound remote source returns beside its own workspace preview', lambda d:
                  d['path'] == previous and d['preview_tab_id'] == pinned['preview_tab_id']
                  and d['preview_path'] == str(preview))
        return pinned

    def overleaf_restore_local_workspace(self, before, retired):
        """Return through the chooser, reopening saved files as new owners."""
        from desktop_ui_journeys import fingerprint
        from desktop_ui_workspace_switch_journeys import workspace_root_is, workspace_tab_ids
        root = self.directory.resolve()
        self.choose_workspace(root)
        self.wait('Returning to Local folders retires every project-owned page', lambda d:
                  workspace_root_is(d, root)
                  and not set(workspace_tab_ids(d)).intersection(retired)
                  and not d['modal_open'])
        path = Path(before['path'])
        if self.snapshot()['document']['path'] != str(path):
            self.open_harper_fixture(path)
        self.click('editor.source')
        self.key(35, 'cmd+alt')
        self.key(20)
        self.key(15)
        restored = self.wait('The unchanged local source reopens with a new ready preview owner', lambda d:
                             overleaf_preview_ready(d, path)
                             and d['source_fingerprint'] == before['source_fingerprint']
                             and d['source_fingerprint'] == fingerprint(path.read_text(encoding='utf-8'))
                             and not set(workspace_tab_ids(d)).intersection(retired), timeout=60)['document']
        # A local workspace can reopen its own recent file. Inspect every fresh
        # owner rather than borrowing the departed workspace's tab count.
        for tab in workspace_tab_ids(restored):
            self.click('tab.' + str(tab))
            page = self.wait('Each returned page belongs to the selected local workspace', lambda d:
                             d['active_tab_id'] == tab and workspace_root_is(d, root)
                             and d['preview_tab_id'] == restored['preview_tab_id']
                             and d['preview_path'] == restored['preview_path'])['document']
            page_path = Path(page['path']) if isinstance(page['path'], str) else None
            if (page_path is None or not page_path.is_absolute()
                    or not page_path.resolve().is_relative_to(root) or not page_path.is_file()
                    or page['dirty']
                    or page['source_fingerprint'] != fingerprint(page_path.read_text(encoding='utf-8'))
                    or page['source_bytes'] != page_path.stat().st_size):
                raise AssertionError('Returned local page has foreign or changed source provenance')
            self.record('overleaf.local_page_provenance', tab=tab,
                        path_fingerprint=fingerprint(str(page_path)), exact_saved_source=True)
        self.click('tab.' + str(restored['active_tab_id']))
        return self.wait('The checked local pages return to their designated source and preview', lambda d:
                         overleaf_preview_ready(d, path)
                         and workspace_tab_ids(d) == workspace_tab_ids(restored))['document']

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

    def overleaf_type_query(self, value, before, *, browser_url=None):
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        self.key(0)
        if value:
            self.native('text', value)
        else:
            self.key(51, 'none')
        self.wait('The focused Overleaf address field receives exact typing without source edits',
                  lambda d: overleaf_browsing(d, before, visible=True, query=value, url=browser_url))

    def overleaf_undo_initial_query(self, before, query):
        # The first edit begins at the default login address. Later rapid
        # replacements can intentionally share an egui undo group.
        self.wait_target('overleaf.url', focused=True)
        self.key(6)
        self.wait('Native Undo restores the default login address without source edits',
                  lambda d: overleaf_browsing(d, before, visible=True, query=LOGIN_URL))
        self.key(6, 'cmd+shift')
        self.wait('Native Redo restores the first typed address without source edits',
                  lambda d: overleaf_browsing(d, before, visible=True, query=query))

    def overleaf_type_directory(self, value, before, query, *, browser_url):
        self.click('overleaf.directory')
        self.wait_target('overleaf.directory', focused=True)
        self.key(0)
        self.native('text', value)
        self.wait('The local-copy folder receives exact native typing without source edits',
                  lambda d: overleaf_browsing(d, before, visible=True, query=query, url=browser_url)
                  and overleaf_storage(d, keep=True, directory=value))

    def overleaf_storage_controls(self, before, *, browser_url):
        self.overleaf_type_query(STORAGE_PROJECT, before, browser_url=browser_url)
        self.wait_target('overleaf.keep_local_copy')
        self.wait_target('overleaf.directory')
        if not overleaf_storage_targets(self.snapshot(), keep=True):
            raise AssertionError('Local-copy controls are not current visible native targets')
        self.click('overleaf.keep_local_copy')
        hidden = self.wait('Temporary mode hides the physical local-copy folder', lambda d:
                           overleaf_browsing(d, before, visible=True, query=STORAGE_PROJECT, url=browser_url)
                           and overleaf_storage(d, keep=False, directory=''))
        if not overleaf_storage_targets(hidden, keep=False):
            raise AssertionError('Temporary mode still exposes local-copy folder controls')
        if getattr(self, 'capture_review', False):
            self.capture_viewport('overleaf')
        self.click('overleaf.keep_local_copy')
        restored = self.wait('Keeping a local copy restores its folder controls', lambda d:
                             overleaf_browsing(d, before, visible=True, query=STORAGE_PROJECT, url=browser_url)
                             and overleaf_storage(d, keep=True, directory=''))
        if not overleaf_storage_targets(restored, keep=True):
            raise AssertionError('Local-copy folder controls did not return')
        directory = str(self.directory / 'overleaf-copies')
        if not Path(directory).is_absolute():
            raise AssertionError('The local-copy journey needs an absolute private fixture folder')
        self.overleaf_type_directory(directory, before, STORAGE_PROJECT, browser_url=browser_url)

        # Local validation must stop the request before an epoch or storage job
        # is allocated; the existing unrelated browsing window stays alive.
        self.overleaf_type_directory(RELATIVE_DIRECTORY, before, STORAGE_PROJECT, browser_url=browser_url)
        self.click('overleaf.connect')
        self.wait('Relative local-copy folders fail without replacing the browsing page', lambda d:
                  overleaf_browsing(d, before, visible=True, query=STORAGE_PROJECT, url=browser_url, error=True)
                  and overleaf_storage(d, keep=True, directory=RELATIVE_DIRECTORY)
                  and 'absolute' in overleaf_state(d)['error'].lower())
        self.record('overleaf.relative_folder_rejected', connection_started=False,
                    storage_worker_started=False, browser_preserved=True)
        return RELATIVE_DIRECTORY

    def overleaf_workspace_before_project(self, before):
        """The usual login window can own a new offline folder before a project is chosen."""
        from desktop_ui_journeys import fingerprint
        self.overleaf_type_query(LOGIN_URL, before)
        local_root = self.directory.resolve()
        created = local_root / 'before-project-workspace'

        def chooser(document):
            return (overleaf_browsing(document, before, visible=False, query=LOGIN_URL)
                    and document['workspace_root_fingerprint'] == before['workspace_root_fingerprint']
                    and document['workspace_chooser_visible'])

        self.click('chooser.local')
        self.wait('The login browser opens Local folders without selecting a project', chooser)
        self.click('chooser.new_workspace')
        self.wait('A projectless new workspace form preserves the source and browser', lambda d:
                  chooser(d) and workspace_creation_state(d)['visible'])
        for target, value, field in (
                ('workspace.parent', str(local_root), 'parent_fingerprint'),
                ('workspace.name', created.name, 'name_fingerprint')):
            self.click(target)
            self.wait_target(target, focused=True)
            self.key(0)
            self.native('text', value)
            self.wait('The projectless workspace form receives exact native text', lambda d:
                      chooser(d) and workspace_creation_state(d)[field] == fingerprint(value))
        self.click('workspace.overleaf')
        self.wait('Choosing Overleaf storage adds no project field to the creation form', lambda d:
                  chooser(d) and workspace_creation_options_hidden(self.snapshot()))
        self.click('workspace.create')
        self.wait('Create returns to the usual login window with the offline folder selected', lambda d:
                  overleaf_projectless_storage(d, before, query=LOGIN_URL,
                                              directory=str(created), controls=True)
                  and not workspace_creation_state(d)['visible'] and not d['workspace_chooser_visible'])
        self.wait('Projectless storage controls have current native geometry', lambda d:
                  overleaf_storage_draft_ready(self.snapshot(), before, query=LOGIN_URL, keep=True))
        if not created.is_dir() or any(created.iterdir()):
            raise AssertionError('Creating projectless storage downloaded or materialized a project')
        for query in (STORAGE_PROJECT, LOGIN_URL):
            self.overleaf_type_query(query, before)
            self.wait('The pending offline folder follows the normal address field without connecting', lambda d:
                      overleaf_projectless_storage(d, before, query=query,
                                                  directory=str(created), controls=True))
        self.wait_target('overleaf.reset_storage')
        self.click('overleaf.reset_storage')
        self.wait('Reset clears projectless storage and restores the ordinary login window', lambda d:
                  overleaf_projectless_storage(d, before, query=LOGIN_URL, directory='', controls=False))
        self.record('overleaf.workspace_before_project', remote_account=False, checkbox_adds_no_fields=True,
                    usual_login_preserved=True, offline_folder_preselected=True,
                    project_chosen_in_overleaf=True, reset_clears_pending_folder=True,
                    source_and_root_preserved=True, connection_started=False)

    def overleaf(self):
        """One browser → file actions → reconnect lifecycle in the same real app."""
        from desktop_ui_workspace_switch_journeys import workspace_root_is, workspace_tab_ids
        before = self.snapshot()['document']
        expected_source = {field: before[field] for field in
                           ('path', 'source_fingerprint', 'source_bytes', 'dirty', 'preview_path')}
        # Keep the phase assertions and add a shared lifecycle boundary. A fresh
        # app per phase hid leaked tabs, drafts and keyboard/window ownership.
        for label, operation, retained_browser, retained_project in (
                ('browser', self.overleaf_connection, True, False),
                ('files', self.overleaf_files, False, False),
                ('storage', self.overleaf_storage, False, True)):
            self.record('overleaf.phase.start', phase=label)
            phase_before = self.snapshot()['document']
            operation()
            self.wait('Overleaf phase returns to the unchanged local file in its local workspace', lambda d:
                      all(d[field] == expected for field, expected in expected_source.items())
                      and workspace_root_is(d, self.directory.resolve())
                      and d['active_tab_id'] == d['preview_tab_id']
                      and d['active_tab_id'] in workspace_tab_ids(d)
                      and (all(d[field] == phase_before[field] for field in OWNER_FIELDS if field != 'revision')
                           and workspace_tab_ids(d) == workspace_tab_ids(phase_before)
                           if label == 'browser' else
                           not set(workspace_tab_ids(d)).intersection(workspace_tab_ids(phase_before)))
                      and d['focused'] and not overleaf_state(d)['visible']
                      and overleaf_state(d)['connection'] is retained_project
                      and overleaf_state(d)['browser'] is retained_browser
                      and overleaf_state(d).get('online') is False
                      and overleaf_state(d).get('sync_ready') is False
                      and overleaf_state(d)['pending'] is False
                      and overleaf_state(d).get('source_locked') is False
                      and (not retained_project or overleaf_state(d)['keep_local_copy'] is True))
            self.record('overleaf.phase.passed', phase=label,
                        local_file_unchanged=True, old_workspace_pages_retired=label != 'browser',
                        network_disconnected=True,
                        retained_project=retained_project)

    def overleaf_connection(self):
        from desktop_ui_journeys import fingerprint
        from overleaf_storage_fixture import StorageFixture
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
        self.wait('A new Overleaf window creates a login browser before selecting a project', lambda d:
                  overleaf_browsing(d, before, visible=True, query=LOGIN_URL)
                  and overleaf_state(d)['browser_start_url_fingerprint'] == fingerprint(LOGIN_URL))
        self.record('overleaf.default_login', project_selected=False, browser_created=True)
        self.overleaf_type_query(INVALID_SCRIPT, before)
        self.overleaf_undo_initial_query(before, INVALID_SCRIPT)
        self.click('overleaf.connect')
        self.wait('A script URL is rejected without creating a project connection', lambda d:
                  overleaf_browsing(d, before, visible=True, query=INVALID_SCRIPT, error=True))
        self.overleaf_workspace_before_project(before)

        with StorageFixture() as fixture:
            # This is an ordinary HTTP page on a different loopback hostname,
            # not a project or a mock native connection event.
            browser_url = fixture.browsing_url
            self.overleaf_type_query(browser_url, before)
            self.click('overleaf.connect')
            if not fixture.page_requested.wait(5):
                raise AssertionError('The browser Go control never requested the ordinary local page')
            self.wait('The address field opens ordinary HTTP pages without a selected project', lambda d:
                      overleaf_browsing(d, before, visible=True, query=browser_url, url=browser_url))
            # Click the real HTML input using the observed native view bounds.
            # Its input handler reports to the local HTTP server, never native IPC.
            keyboard = overleaf_state(self.snapshot()['document']).get('browser_keyboard')
            rect = keyboard.get('web_screen_rect_top_left') if isinstance(keyboard, dict) else None
            if (not isinstance(rect, list) or len(rect) != 4
                    or any(type(value) not in (int, float) or not math.isfinite(value) for value in rect)
                    or rect[2] <= 80 or rect[3] <= 40):
                raise AssertionError('Native browser bounds are unavailable for the HTML input check')
            self.native('move', rect[0] + 64, rect[1] + 30)
            self.native('click', rect[0] + 64, rect[1] + 30)
            self.key(0)
            self.native('text', 'actual browser typing')
            self.wait('A real browser click receives typing without changing native source', lambda d:
                      '/fixture/browser-input?text=actual%20browser%20typing' in fixture.requests
                      and overleaf_browsing(d, before, visible=True, query=browser_url, url=browser_url))
            self.record('overleaf.ordinary_browsing', local_page_requested=True, project_selected=False,
                        browser_click_and_typing=True, source_unchanged=True)
            for query in ('', INVALID_SCRIPT, INVALID_FILE, INVALID_CREDENTIALS):
                self.overleaf_type_query(query, before, browser_url=browser_url)
                self.click('overleaf.connect')
                self.wait('Invalid navigation preserves the existing page and source owner', lambda d:
                          overleaf_browsing(d, before, visible=True, query=query,
                                            url=browser_url, error=True))
            self.record('overleaf.invalid_rejected', connection_started=False, browser_preserved=True)
            directory = self.overleaf_storage_controls(before, browser_url=browser_url)

            self.click('overleaf.url')
            self.wait_target('overleaf.url', focused=True)
            for code, label in ((3, 'Find'), (45, 'New document')):
                self.key(code)
                self.stable_state(lambda d:
                                  overleaf_browsing(d, before, visible=True, query=STORAGE_PROJECT, url=browser_url)
                                  and not d['find_visible'] and not d['palette_visible'])
                self.record('overleaf.child_command_isolated', command=label)
            self.key(13)
            self.wait('Cmd+W hides only the focused Overleaf window', lambda d:
                      overleaf_browsing(d, before, visible=False, query=STORAGE_PROJECT, url=browser_url)
                      and overleaf_storage(d, keep=True, directory=directory))
            self.stable_state(unchanged)
            self.record('overleaf.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

            self.open_overleaf_connection()
            self.wait('Reopening retains its browser and unsent project folder without connecting', lambda d:
                      overleaf_browsing(d, before, visible=True, query=STORAGE_PROJECT, url=browser_url)
                      and overleaf_storage(d, keep=True, directory=directory))
            self.click('overleaf.hide')
            self.wait('Hide returns to the document and retains the browser', lambda d:
                      overleaf_browsing(d, before, visible=False, query=STORAGE_PROJECT, url=browser_url))
            if fixture.save_count != 0:
                raise AssertionError('Ordinary browsing unexpectedly wrote to the fixture project')
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


    def overleaf_blocked_typing(self, before=None, *, capture=False):
        """Actual blocked typing must paint feedback, then expire without source changes."""
        self.wait_target('editor.source', focused=True)
        if before is None:
            before = self.snapshot()['document']
        self.wait('Read-only feedback is hidden before the next actual edit attempt', lambda d:
                  overleaf_edit_rejection(d, before, visible=False))
        self.native('text', 'MUST REMAIN LOCKED')
        self.wait('Blocked typing visibly acknowledges rejection without changing source, selection or focus', lambda d:
                  overleaf_edit_rejection(d, before, visible=True))
        if capture and getattr(self, 'capture_review', False):
            self.capture_viewport('main')
        self.wait('Read-only rejection feedback expires while the exact source remains locked and focused', lambda d:
                  overleaf_edit_rejection(d, before, visible=False))
        self.record('overleaf.read_only_rejection', native_typing=True, visible_then_expired=True,
                    source_and_selection_preserved=True, focus_preserved=True)

    def overleaf_header_indicator(self, *, present, reference=None, hover=False):
        self.focus_document_window()
        self.wait_target('explorer.workspace')
        snapshot = self.snapshot()
        geometry = overleaf_header_geometry(snapshot, present=present, reference=reference)
        before = snapshot['document']
        if hover:
            if not present:
                raise AssertionError('A local header has no Overleaf status to hover')
            self.wait_target('editor.source')
            editor = self.snapshot()['targets']['editor.source']
            self.native('move', editor['x'], editor['y'])
            self.wait('Any previous tooltip closes before hovering the status indicator', lambda d:
                      not d['hover_tooltip_open'] and overleaf_source_preserved(d, before))
            self.native('move', geometry[0], geometry[1])
            self.wait('The Overleaf indicator exposes its status only on hover without moving the selection', lambda d:
                      d['hover_tooltip_open'] and overleaf_source_preserved(d, before))
            overleaf_header_geometry(self.snapshot(), present=True, reference=geometry)
            self.native('move', editor['x'], editor['y'])
            self.wait('Leaving the indicator dismisses its hover text without changing source', lambda d:
                      not d['hover_tooltip_open'] and overleaf_source_preserved(d, before))
        self.record('overleaf.header_indicator', present=present, hover_checked=hover,
                    geometry_preserved=True, source_unchanged=True)
        return geometry

    def overleaf_storage(self):
        """Positive local-HTTP/WebView journey; no real account or source injection."""
        import time
        from overleaf_storage_fixture import ASSET_BYTES, SOURCE
        from overleaf_auth_fixture import AuthStorageFixture
        from desktop_ui_journeys import PREVIEW_SOURCE, fingerprint
        from desktop_ui_workspace_switch_journeys import workspace_root_is, workspace_tab_ids
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
        self.overleaf_header_indicator(present=False)

        def type_field(target, value, field):
            self.click(target)
            self.wait_target(target, focused=True)
            self.key(0)
            self.native('text', value)
            self.wait('Storage field receives exact native input', lambda d:
                      overleaf_state(d)[field] == fingerprint(value)
                      and overleaf_owner_unchanged(d, before))

        with AuthStorageFixture(connection_events=True, presence=True, lazy_startup=True,
                                preview_source=PREVIEW_SOURCE,
                                project_name=LONG_PROJECT_NAME) as fixture:
            self.open_overleaf_connection()
            type_field('overleaf.url', fixture.url, 'query_fingerprint')
            self.wait_target('overleaf.keep_local_copy')
            if overleaf_state(self.snapshot()['document'])['storage_keep_local_copy']:
                self.click('overleaf.keep_local_copy')
            self.wait('Temporary storage is selected', lambda d:
                      not overleaf_state(d)['storage_keep_local_copy'])
            self.click('overleaf.connect')
            if not fixture.editor_chunk_requested.wait(8):
                raise AssertionError('The hosted source editor never requested its separate lazy bundle')
            self.stable_state(lambda d: overleaf_owner_unchanged(d, before)
                              and not overleaf_state(d)['sync_ready'])
            if fixture.project_loads != 1:
                raise AssertionError('The bridge reloaded the project while its source bundle was pending')
            fixture.release_editor_chunk.set()
            loaded = self.wait('Fresh source appears locked while the asset is pending', lambda d:
                               d['source_fingerprint'] == fingerprint(SOURCE)
                               and overleaf_state(d)['source_locked']
                               and not overleaf_state(d)['sync_ready']
                               and overleaf_state(d)['keep_local_copy'] is False,
                               timeout=20)['document']
            if not fixture.asset_requested.wait(1):
                raise AssertionError('The real bridge never requested the held asset')
            if fixture.project_loads != 1:
                raise AssertionError('Lazy editor mounting navigated away from its original project load')
            self.record('overleaf.lazy_editor_bootstrap', project_loads=fixture.project_loads,
                        lazy_chunk=True, initial_tree_mounted=False, native_source_unchanged=True)
            header = self.overleaf_header_indicator(present=True)
            if getattr(self, 'capture_review', False):
                self.capture_viewport('main')
            temporary = Path(loaded['path']).parent
            self.wait('The first remote document owns the new root after closing local pages', lambda d:
                      workspace_root_is(d, temporary)
                      and not set(workspace_tab_ids(d)).intersection(workspace_tab_ids(before)))
            self.focus_document_window()
            self.click('editor.source')
            self.overleaf_blocked_typing(capture=True)
            self.stable_state(lambda d: d['source_fingerprint'] == fingerprint(SOURCE)
                              and overleaf_state(d)['source_locked'])
            fixture.release_assets.set()
            self.wait('Every current server file reaches disk before editing is enabled', lambda d:
                      overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked']
                      and d['source_fingerprint'] == fingerprint(SOURCE), timeout=15)
            if (temporary / 'fixture.bin').read_bytes() != ASSET_BYTES:
                raise AssertionError('Ready was reported before the held asset was written exactly')
            self.overleaf_header_indicator(present=True, reference=header, hover=True)
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
            project_preview = self.overleaf_pin_workspace_preview(temporary)
            self.overleaf_collaborator_badges(fixture, temporary, project_preview['preview_path'])
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
            retired = workspace_tab_ids(self.snapshot()['document'])
            before = self.overleaf_restore_local_workspace(before, retired)
            self.wait('Leaving the temporary workspace releases its disconnected project', lambda d:
                      not overleaf_state(d)['connection'])
            deadline = time.monotonic() + 5
            while temporary.exists() and time.monotonic() < deadline:
                time.sleep(.05)
            if temporary.exists():
                raise AssertionError('An unused temporary Overleaf workspace was not deleted')
            self.overleaf_header_indicator(present=False)
            self.record('overleaf.temporary_hydration_and_cleanup', remote_account=False,
                        asset_bytes=len(ASSET_BYTES), blocked_edit=True, native_edit=True)

            # The same official bridge path now exercises retained custom storage.
            self.open_overleaf_connection()
            type_field('overleaf.url', fixture.url, 'query_fingerprint')
            self.wait_target('overleaf.keep_local_copy')
            if not overleaf_state(self.snapshot()['document'])['storage_keep_local_copy']:
                self.click('overleaf.keep_local_copy')
            self.wait('Local-copy controls remain attached to the entered project after leaving its address', lambda d:
                      overleaf_storage_draft_ready(self.snapshot(), before, query=fixture.url, keep=True))
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
            self.wait('The persistent project replaces every previous local-owned page', lambda d:
                      workspace_root_is(d, persistent)
                      and not set(workspace_tab_ids(d)).intersection(workspace_tab_ids(before)))
            self.overleaf_pin_workspace_preview(persistent)
            self.overleaf_reconnect(fixture, persistent)
            self.open_overleaf_connection()
            self.click('overleaf.disconnect')
            self.wait('The retained project disconnects without deleting its files', lambda d:
                      not overleaf_state(d)['browser'] and not overleaf_state(d)['sync_ready'])
            self.click('overleaf.hide')
            self.wait('Hide returns focus to the source window', lambda d:
                      not overleaf_state(d)['visible'] and d['focused'])
            retired = workspace_tab_ids(self.snapshot()['document'])
            self.overleaf_restore_local_workspace(before, retired)
            if not persistent.is_dir() or (persistent / 'main.txt').read_text() != SOURCE:
                raise AssertionError('Closing the tab removed or changed its persistent copy')
            if (persistent / 'fixture.bin').read_bytes() != ASSET_BYTES:
                raise AssertionError('The persistent copy lost its server asset')
            self.record('overleaf.persistent_copy_survives_close', remote_account=False,
                        source_fingerprint=fingerprint(SOURCE))

    def overleaf_wait_follow_visible(self, description, predicate, timeout=6):
        """Wait for state and actual post-scroll caret painting in the same root frame."""
        import time
        deadline = time.monotonic() + timeout
        last_error = None
        while time.monotonic() < deadline:
            snapshot = self.snapshot()
            if predicate(snapshot['document']):
                try:
                    overleaf_follow_caret(snapshot)
                except AssertionError as error:
                    last_error = error
                else:
                    return snapshot
            time.sleep(.05)
        raise AssertionError('timed out: ' + description) from last_error

    def overleaf_collaborator_badges(self, fixture, directory, preview_path):
        """Seek/follow real socket carets through badge clicks, retaining source and selection."""
        from overleaf_storage_fixture import DOCUMENT, SECOND_DOCUMENT, SECOND_SOURCE, PEER, SOURCE
        from desktop_ui_journeys import fingerprint
        main, other = str(directory / 'main.txt'), str(directory / 'other.txt')
        badge = 'overleaf.peer.' + PEER
        def offset(row, column):
            return sum(len(line) + 1 for line in SECOND_SOURCE.split('\n')[:row]) + column
        def unchanged(document, path, source):
            return (document['path'] == path and document['source_fingerprint'] == fingerprint(source)
                    and document['preview_path'] == preview_path and not document['dirty'])
        def following(document, head):
            state = overleaf_follow_state(document)
            return (state['following_peer'] == PEER and state['seeking_peer'] is None
                    and state['following_head'] == head and state['collaborator_count'] == 1)
        self.focus_document_window()
        main_before = self.snapshot()['document']
        if not unchanged(main_before, main, SOURCE):
            raise AssertionError('Collaborator checks need the unchanged main source and designated preview')
        fixture.move_peer(DOCUMENT, 0, 6)
        self.wait('The official presence event displays a real clickable collaborator badge', lambda d:
                  overleaf_follow_state(d)['collaborator_count'] == 1
                  and overleaf_follow_state(d)['cursor_count'] == 1)
        self.wait_target(badge)
        self.click(badge)
        self.wait('Single-click seeks the peer cursor once without following or changing source', lambda d:
                  unchanged(d, main, SOURCE) and d['cursor'] == [6, 6]
                  and overleaf_follow_state(d)['following_peer'] is None
                  and overleaf_follow_state(d)['seeking_peer'] is None)
        fixture.move_peer(DOCUMENT, 0, 12)
        self.stable_state(lambda d: unchanged(d, main, SOURCE) and d['cursor'] == [6, 6]
                          and overleaf_follow_state(d)['following_peer'] is None)
        self.double_click(badge)
        self.overleaf_wait_follow_visible('Double-click follows the latest peer viewport without moving the local selection', lambda d:
                  unchanged(d, main, SOURCE) and d['cursor'] == [6, 6] and following(d, 12))
        self.key(1)  # Save is a global command, not local cursor navigation.
        self.overleaf_wait_follow_visible('The Save shortcut preserves follow and the local selection', lambda d:
                  unchanged(d, main, SOURCE) and d['cursor'] == [6, 6] and following(d, 12)
                  and not overleaf_state(d)['pending'])
        if getattr(self, 'capture_review', False):
            self.capture_viewport('main')
        fixture.move_peer(SECOND_DOCUMENT, 80, 5)
        crossed = self.overleaf_wait_follow_visible('Following opens the peer document far below the screen and retains the pinned preview', lambda d:
                            unchanged(d, other, SECOND_SOURCE) and following(d, offset(80, 5)))['document']
        local_cursor = crossed['cursor']
        fixture.move_peer(SECOND_DOCUMENT, 110, 3)
        self.overleaf_wait_follow_visible('Later remote movement scrolls the follow viewport without overwriting the local cursor', lambda d:
                  unchanged(d, other, SECOND_SOURCE) and following(d, offset(110, 3))
                  and d['cursor'] == local_cursor)
        self.key(123, 'none')
        cancelled = self.wait('Local arrow navigation cancels follow', lambda d:
                              unchanged(d, other, SECOND_SOURCE)
                              and overleaf_follow_state(d)['following_peer'] is None)['document']
        fixture.move_peer(SECOND_DOCUMENT, 30, 1)
        self.stable_state(lambda d: unchanged(d, other, SECOND_SOURCE)
                          and d['cursor'] == cancelled['cursor']
                          and overleaf_follow_state(d)['following_peer'] is None)
        self.key(13)
        self.wait('Closing the peer retires only its page and keeps the main and preview owners', lambda d:
                  overleaf_peer_closed(d, cancelled, main_before))
        self.wait_target('explorer.file.main.txt')
        self.double_click('explorer.file.main.txt')
        self.wait('Explorer returns to the unchanged original remote source', lambda d:
                  unchanged(d, main, SOURCE) and d['active_tab_id'] == main_before['active_tab_id']
                  and overleaf_follow_state(d)['following_peer'] is None)
        fixture.move_peer(DOCUMENT, 0, 6)
        self.wait('The peer returns to the active original document before another badge action', lambda d:
                  unchanged(d, main, SOURCE) and overleaf_follow_state(d)['cursor_count'] == 1)
        self.double_click(badge)
        self.overleaf_wait_follow_visible('Double-click can start following again after local cancellation', lambda d:
                  unchanged(d, main, SOURCE) and following(d, 6))
        self.double_click(badge)
        self.overleaf_wait_follow_visible('A second double-click keeps following the same collaborator', lambda d:
                  unchanged(d, main, SOURCE) and following(d, 6))
        self.triple_click(badge)
        self.wait('Triple-click stops following the same collaborator', lambda d:
                  unchanged(d, main, SOURCE) and overleaf_follow_state(d)['following_peer'] is None)
        self.double_click(badge)
        self.overleaf_wait_follow_visible('Following restarts for collaborator departure coverage', lambda d: following(d, 6))
        fixture.remove_peer()
        self.wait('A departed collaborator clears follow without editing or moving the local cursor', lambda d:
                  unchanged(d, main, SOURCE) and overleaf_follow_state(d)['following_peer'] is None
                  and overleaf_follow_state(d)['collaborator_count'] == 0)
        self.record('overleaf.collaborator_badges', remote_account=False, single_click_seek=True,
                    double_click_follow=True, triple_click_stop=True, cross_document=True, local_navigation_cancels=True,
                    selection_preserved_while_following=True, save_shortcut_preserves_follow=True, peer_leave_clears_follow=True,
                    pinned_preview_preserved=True)

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
        self.wait('Leaving the actual project root asks before closing its dirty source', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=False, root=remote_root)
                  and d['modal_open'])
        self.click('modal.cancel')
        self.wait('Cancel retains the project root and every bound source owner', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=False, root=remote_root)
                  and not d['modal_open'])
        self.double_click('explorer.workspace')
        self.wait('The retained remote root reopens its existing Overleaf connection', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=True, root=remote_root))
        self.click('chooser.local')
        self.wait('The existing connection returns to the local chooser a second time', lambda d:
                  overleaf_chooser_state(d, before, local=True, remote=False, root=remote_root))
        self.key(53, 'none')
        self.wait('Escape dismisses only the local chooser without changing root or source', lambda d:
                  overleaf_chooser_state(d, before, local=False, remote=False, root=remote_root)
                  and not d['modal_open'])
        self.record('overleaf.chooser_roundtrip', remote_account=False, draft_preserved=True,
                    connection_epoch=overleaf_state(before)['epoch'], actual_root_switch_cancelled=True)
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Chooser roundtrip restores Explorer visibility', lambda d: not d['explorer_visible'])
        self.focus_document_window()
        self.click('editor.source')

    def overleaf_workspace_creation(self, fixture):
        """Create real folders through native controls while a dirty remote tab stays bound."""
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        local_root = self.directory.resolve()
        remote_root = Path(before['path']).parent
        if not before['dirty'] or before['workspace_root_fingerprint'] != fingerprint(str(remote_root)):
            raise AssertionError('Workspace creation needs a dirty source owned by the current project root')
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Explorer opens for workspace creation', lambda d: d['explorer_visible'])

        def unchanged(document, *, root=remote_root, local=True, remote=False):
            return overleaf_chooser_state(document, before, local=local, remote=remote, root=root)

        def open_creation():
            self.click('chooser.new_workspace')
            self.wait('New Workspace opens without creating a folder or changing the connected draft', lambda d:
                      unchanged(d) and workspace_creation_state(d)['visible']
                      and not workspace_creation_state(d)['busy'])

        def type_field(target, value, field):
            self.click(target)
            self.wait_target(target, focused=True)
            self.key(0)
            self.native('text', value)
            self.wait('The workspace field receives exact native input without editing source', lambda d:
                      unchanged(d) and workspace_creation_state(d)[field] == fingerprint(value))

        self.key(31, 'cmd+shift')
        self.wait('The local chooser opens before creating a workspace', unchanged)
        open_creation()
        cancelled = local_root / 'cancelled-workspace'
        type_field('workspace.parent', str(local_root), 'parent_fingerprint')
        type_field('workspace.name', cancelled.name, 'name_fingerprint')
        self.click('workspace.cancel')
        self.wait('Cancel returns to the chooser without creating a workspace', lambda d:
                  unchanged(d) and not workspace_creation_state(d)['visible'])
        if cancelled.exists():
            raise AssertionError('Cancelling workspace creation left a folder behind')

        open_creation()
        type_field('workspace.parent', str(local_root), 'parent_fingerprint')
        escaped = local_root.parent / (local_root.name + '-escaped-workspace')
        type_field('workspace.name', '../' + escaped.name, 'name_fingerprint')
        self.click('workspace.create')
        self.wait('An invalid workspace name stays in the form with an explicit error', lambda d:
                  unchanged(d) and workspace_creation_state(d)['visible']
                  and not workspace_creation_state(d)['busy']
                  and workspace_creation_state(d)['error'] is not None)
        if escaped.exists():
            raise AssertionError('An invalid workspace name created a folder outside its parent')
        occupied = local_root / 'occupied-workspace'
        occupied.mkdir()
        retained = occupied / 'keep.txt'
        retained.write_text('Existing workspace must remain unchanged.\n')
        type_field('workspace.name', occupied.name, 'name_fingerprint')
        self.click('workspace.create')
        self.wait('An existing workspace is rejected without replacing its files', lambda d:
                  unchanged(d) and workspace_creation_state(d)['visible']
                  and not workspace_creation_state(d)['busy']
                  and workspace_creation_state(d)['error'] is not None)
        if retained.read_text() != 'Existing workspace must remain unchanged.\n':
            raise AssertionError('Workspace creation overwrote an existing folder')
        created = local_root / 'new-local-workspace'
        type_field('workspace.name', created.name, 'name_fingerprint')
        if workspace_creation_state(self.snapshot()['document'])['use_overleaf']:
            self.click('workspace.overleaf')
            self.wait('The new folder is a plain local workspace', lambda d:
                      unchanged(d) and not workspace_creation_state(d)['use_overleaf'])
        self.click('workspace.create')
        self.wait('A created local workspace asks before closing the current project draft', lambda d:
                  unchanged(d, local=False) and not workspace_creation_state(d)['visible']
                  and not workspace_creation_state(d)['busy'] and d['modal_open'])
        self.click('modal.cancel')
        self.wait('Cancelling new-workspace adoption preserves every current project page', lambda d:
                  unchanged(d, local=False) and not d['modal_open'])
        if not created.is_dir() or (created / '.tiptoptyp/overleaf-project.json').exists():
            raise AssertionError('The new local workspace was not created as an independent folder')

        self.key(31, 'cmd+shift')
        self.wait('The retained project root reopens Local folders for storage-only creation', unchanged)
        open_creation()
        linked = local_root / 'new-overleaf-workspace'
        type_field('workspace.parent', str(local_root), 'parent_fingerprint')
        type_field('workspace.name', linked.name, 'name_fingerprint')
        if not workspace_creation_state(self.snapshot()['document'])['use_overleaf']:
            self.click('workspace.overleaf')
        self.wait('The Overleaf checkbox keeps project selection out of the creation form', lambda d:
                  unchanged(d) and workspace_creation_options_hidden(self.snapshot()))
        self.click('workspace.create')
        staged = self.wait('Workspace creation isolates a new login and pauses the retained account', lambda d:
                           overleaf_staged_workspace(
                               d, before, query=fixture.url,
                               directory_fingerprint=fingerprint(str(linked))))['document']
        staged_profile = overleaf_profile(overleaf_state(staged))
        if not linked.is_dir() or any(linked.iterdir()):
            raise AssertionError('Staging an Overleaf storage folder connected or materialized remote files')
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        self.key(0)
        self.native('text', STORAGE_PROJECT)
        self.wait('Choosing a project in the normal address field retains the new storage folder', lambda d:
                  overleaf_staged_workspace(d, before, query=STORAGE_PROJECT,
                                             directory_fingerprint=fingerprint(str(linked)),
                                             profile=staged_profile))
        self.click('overleaf.url')
        self.wait_target('overleaf.url', focused=True)
        self.key(0)
        self.native('text', fixture.url)
        self.wait('Returning to the existing project keeps the pending folder until Reset', lambda d:
                  overleaf_staged_workspace(d, before, query=fixture.url,
                                             directory_fingerprint=fingerprint(str(linked)),
                                             profile=staged_profile))
        self.wait_target('overleaf.reset_storage')
        self.click('overleaf.reset_storage')
        reconnected = self.wait('Reset restores the original online account and its retained draft', lambda d:
                                overleaf_workspace_reconnected(d, before), timeout=30)['document']
        self.wait('Workspace creation returns to the original source and root', lambda d:
                  overleaf_chooser_state(d, reconnected, local=False, remote=False, root=remote_root)
                  and d['focused'])
        if not before['explorer_visible']:
            self.key(18)
            self.wait('Workspace creation restores Explorer visibility', lambda d: not d['explorer_visible'])
        self.record('overleaf.workspace_creation', remote_account=False, cancelled_without_writes=True,
                    invalid_name_rejected=True, existing_files_preserved=True, local_root_created=True,
                    actual_root_switch_cancelled=True,
                    optional_storage_preselected_without_connecting=True, project_chosen_in_overleaf=True,
                    isolated_login_pauses_old_account=True, reset_reconnects_online_account=True,
                    reset_restores_original_profile=True,
                    reset_restores_active_storage=True, dirty_source_and_connection_preserved=True)
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

    def overleaf_page_departure(self, fixture, baseline):
        """Prove native page-load invalidation with no disconnect/IPC shortcut."""
        from desktop_ui_journeys import fingerprint
        if self.snapshot()['document'].get('auto_save') is not False:
            raise AssertionError('Page-departure coverage requires a genuinely unsaved draft')
        draft = baseline + 'Retained page-departure draft. αβ\n'
        edited = self.overleaf_replace_source(draft)
        state = overleaf_reconcile_state(edited)
        if (not edited['dirty'] or state.get('online') is not True
                or not state['sync_ready'] or state['source_locked']):
            raise AssertionError('Page departure must begin with a ready connected native draft')
        writes = fixture.save_count
        if not fixture.connection_requested.wait(2):
            raise AssertionError('The page-departure frontend transport never started')
        fixture.depart_page()
        if not fixture.page_requested.wait(5):
            raise AssertionError('The real browser never navigated to the isolated loopback sign-in page')
        self.wait('Committed main-frame departure locks the exact draft and selection while marking it offline',
                  lambda d: overleaf_page_departed(d, edited), timeout=10)
        self.focus_document_window()
        self.wait_target('editor.source', focused=True)
        self.native('text', 'MUST REMAIN LOCKED')
        self.stable_state(lambda d: overleaf_page_departed(d, edited))
        if fixture.source != baseline or fixture.save_count != writes:
            raise AssertionError('Leaving the page or blocked typing unexpectedly published the local draft')
        fixture.restore_page()
        fixture.asset_requested.clear()
        fixture.document_requested.clear()
        fixture.release_assets.clear()
        try:
            self.open_overleaf_connection()
            self.click('overleaf.url')
            self.wait_target('overleaf.url', focused=True)
            self.key(0)
            self.native('text', fixture.url)
            self.wait('Reconnect addresses the original project without editing its retained draft', lambda d:
                      overleaf_source_preserved(d, edited)
                      and overleaf_state(d)['query_fingerprint'] == fingerprint(fixture.url))
            self.click('overleaf.connect')
            if not fixture.asset_requested.wait(5) or not fixture.document_requested.wait(1):
                raise AssertionError('Reconnect never requested fresh source and the held server asset')
            self.wait('Reconnect remains locked until its newly downloaded asset is written', lambda d:
                      overleaf_source_preserved(d, edited)
                      and overleaf_reconcile_state(d).get('online') is True
                      and not overleaf_state(d)['sync_ready'] and overleaf_state(d)['source_locked'])
        finally:
            fixture.release_assets.set()
        self.wait('The actual Reconnect control requires fresh hydration and preserves the unconfirmed draft', lambda d:
                  overleaf_source_preserved(d, edited)
                  and overleaf_reconcile_state(d).get('online') is True
                  and overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked']
                  and overleaf_state(d)['epoch'] > state['epoch'] and not overleaf_state(d)['pending'],
                  timeout=20)
        if fixture.source != baseline or fixture.save_count != writes:
            raise AssertionError('Reconnecting after page departure published an unconfirmed draft')
        self.overleaf_replace_source(baseline)
        self.key(1)
        self.wait('Reverting the test draft restores the original acknowledged baseline', lambda d:
                  d['source_fingerprint'] == fingerprint(baseline) and not d['dirty']
                  and overleaf_reconcile_state(d).get('online') is True
                  and overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked']
                  and not overleaf_state(d)['pending'])
        if fixture.source != baseline or fixture.save_count != writes:
            raise AssertionError('Returning to the unchanged baseline should not require a server write')
        self.record('overleaf.page_departure', remote_account=False, committed_main_frame=True,
                    separate_loopback_origin=True, no_socket_disconnect=True,
                    draft_and_selection_preserved=True, blocked_edit=True,
                    reconnect_rehydrated=True, no_server_write=True)

    def overleaf_fixture_button(self, y):
        state = overleaf_state(self.snapshot()['document'])
        if type(state.get('show_browser')) is not bool:
            raise AssertionError('Overleaf browser visibility observation is unavailable')
        if not state['show_browser']:
            self.click('overleaf.show_browser')
            self.wait('The controlled local HTML page is visible before native input', lambda d:
                      overleaf_state(d).get('show_browser') is True)
        rect = overleaf_native_browser_rect(self.snapshot()['document'])
        self.native('move', rect[0] + 80, rect[1] + y)
        self.native('click', rect[0] + 80, rect[1] + y)

    def overleaf_authentication(self, fixture):
        """Real HTTP sign-in, cookie reuse and expiry through recreated native browsers."""
        self.focus_document_window()
        self.click('editor.source')
        self.key(125, 'cmd')
        self.key(123, 'shift')
        before = self.wait('Authentication starts with a genuinely dirty selected source', lambda d:
                           d['dirty'] and d['cursor'][0] != d['cursor'][1]
                           and overleaf_reconcile_state(d)['sync_ready'])['document']
        source, writes = fixture.source, fixture.save_count
        for phase, signin_count in (('signin', 1), ('reuse', 1), ('expired', 2)):
            if phase == 'signin':
                fixture.enable_auth()
            elif phase == 'expired':
                fixture.expire_session()
                fixture.arm_attacks()
            previous = fixture.auth_observation()
            fixture.asset_requested.clear()
            fixture.document_requested.clear()
            fixture.release_assets.clear()
            self.open_overleaf_connection()
            epoch_delta = 1 if phase == 'reuse' else 2
            try:
                self.click('overleaf.connect')
                if phase != 'reuse':
                    self.wait('A real project redirect requests sign-in and locks the retained source', lambda d:
                              overleaf_authentication_state(d, before, url=fixture.login_url,
                                                            phase='signin', epoch_delta=1), timeout=10)
                    self.focus_document_window()
                    self.wait_target('editor.source', focused=True)
                    self.overleaf_blocked_typing(before)
                    self.stable_state(lambda d: overleaf_authentication_state(
                        d, before, url=fixture.login_url, phase='signin', epoch_delta=1))
                    self.open_overleaf_connection()
                    # The fixture has one ordinary form button at (16,16),
                    # never a scripted native IPC shortcut for authentication.
                    self.overleaf_fixture_button(34)
                if not fixture.asset_requested.wait(8) or not fixture.document_requested.wait(1):
                    raise AssertionError('Authenticated return did not fetch fresh source and the held asset')
                self.wait('Authenticated return uses a fresh epoch and waits for fresh disk hydration',
                          lambda d: overleaf_authentication_state(
                              d, before, url=fixture.url, phase='hydrating', epoch_delta=epoch_delta), timeout=10)
            finally:
                fixture.release_assets.set()
            ready = self.wait('Authentication restores the same workspace, dirty draft and selection only after hydration',
                              lambda d: overleaf_authentication_state(
                                  d, before, url=fixture.url, phase='ready', epoch_delta=epoch_delta), timeout=20)['document']
            observation = fixture.auth_observation()
            if (observation['signins'] != signin_count
                    or observation['authenticated_documents'] <= previous['authenticated_documents']
                    or observation['authenticated_assets'] <= previous['authenticated_assets']
                    or fixture.source != source or fixture.save_count != writes):
                raise AssertionError('Authentication failed to reuse the cookie, hydrate fresh files, or retain the draft')
            if phase == 'reuse' and (observation['login_requests'] != previous['login_requests']
                                     or observation['dashboard_requests'] != previous['dashboard_requests']):
                raise AssertionError('A recreated native browser did not reuse its existing authenticated cookie')
            if phase != 'reuse' and (observation['login_requests'] <= previous['login_requests']
                                     or observation['dashboard_requests'] <= previous['dashboard_requests']):
                raise AssertionError('Sign-in bypassed its real HTTP form and dashboard redirect')
            if phase == 'expired':
                # This creates the same-project iframe and popup only after
                # the legitimate root is ready, so URL-only IPC guards fail.
                self.open_overleaf_connection()
                self.overleaf_fixture_button(82)
                self.wait('Both hostile children attempt native IPC without poisoning the ready parent', lambda d:
                          all(fixture.auth_observation()[kind + suffix]
                              for kind in ('iframe', 'popup')
                              for suffix in ('_attempted', '_ipc_available'))
                          and overleaf_authentication_state(
                              d, before, url=fixture.url, phase='ready', epoch_delta=epoch_delta), timeout=10)
                self.stable_state(lambda d: overleaf_authentication_state(
                    d, before, url=fixture.url, phase='ready', epoch_delta=epoch_delta))
                if fixture.source != source or fixture.save_count != writes:
                    raise AssertionError('A hostile child published or replaced the retained source')
                self.record('overleaf.native_frame_isolation', remote_account=False,
                            parent_ready_before_attack=True, iframe_ipc_attempted=True,
                            popup_ipc_attempted=True, parent_healthy=True, no_server_write=True)
            before = ready
        self.open_overleaf_connection()
        self.click('overleaf.hide')
        self.wait('Authenticated source returns to the main window without a server write', lambda d:
                  overleaf_source_preserved(d, before) and not overleaf_state(d)['visible'] and d['focused'])
        self.record('overleaf.native_authentication', remote_account=False, real_http_form=True,
                    dashboard_return=True, post_login_epoch_rotated=True, exact_epoch_progression=True,
                    cookie_reused_after_browser_recreation=True,
                    expired_cookie_requires_signin=True, fresh_hydration=True,
                    draft_and_selection_preserved=True, no_server_write=True)

    def overleaf_pending_storage_reconnect(self, fixture, persistent):
        """An unfinished relocation cannot prevent reconnecting the current dirty project."""
        self.key(125, 'cmd')
        self.key(123, 'shift')
        before = self.wait('Storage changes exercise a genuinely dirty selected source', lambda d:
                           d['dirty'] and d['cursor'][0] != d['cursor'][1]
                           and overleaf_reconcile_state(d)['sync_ready'])['document']
        previous = overleaf_state(before)
        source, writes = fixture.source, fixture.save_count
        alternate = self.directory / 'unapplied-overleaf-copies'
        if alternate.exists() or Path(before['path']).parent != persistent:
            raise AssertionError('Pending storage fixture must start in its unchanged current workspace')
        self.open_overleaf_connection()
        self.click('overleaf.directory')
        self.wait_target('overleaf.directory', focused=True)
        self.key(0)
        self.native('text', str(alternate))
        self.wait('The alternative folder stays an unsaved storage draft', lambda d:
                  overleaf_session_preserved(d, before)
                  and overleaf_storage(d, keep=True, directory=str(alternate)))
        self.wait_target('overleaf.apply_storage')
        self.wait_target('overleaf.reset_storage')
        if not overleaf_pending_storage_targets(self.snapshot(), pending=True):
            raise AssertionError('Pending storage changes lack current Apply and Reset controls')
        self.click('overleaf.apply_storage')
        self.wait('Applying another storage root asks before closing the dirty project', lambda d:
                  overleaf_session_preserved(d, before)
                  and d['modal_open']
                  and overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked'])
        self.click('modal.cancel')
        self.wait('Cancelling relocation preserves the current project and pending storage fields', lambda d:
                  overleaf_session_preserved(d, before) and not d['modal_open']
                  and overleaf_storage(d, keep=True, directory=str(alternate)))
        self.stable_state(lambda d: overleaf_session_preserved(d, before))
        if alternate.exists():
            raise AssertionError('Cancelled Apply created a replacement project directory')
        # The unsaved prompt returns focus to the source window. Bring the
        # existing manager forward before using its Reconnect button.
        self.open_overleaf_connection()
        self.wait('The raised manager retains the current session and exact pending folder', lambda d:
                  overleaf_session_preserved(d, before)
                  and overleaf_storage(d, keep=True, directory=str(alternate)))
        fixture.asset_requested.clear()
        fixture.document_requested.clear()
        fixture.release_assets.clear()
        try:
            self.click('overleaf.connect')
            if not fixture.asset_requested.wait(5) or not fixture.document_requested.wait(1):
                raise AssertionError('Reconnect with pending storage did not fetch fresh project files')
            self.wait('Reconnect retains the original draft and workspace while hydration locks editing', lambda d:
                      overleaf_session_preserved(d, before, reconnected=True)
                      and overleaf_state(d).get('online') is True
                      and not overleaf_state(d)['sync_ready'] and overleaf_state(d)['source_locked'])
        finally:
            fixture.release_assets.set()
        ready = self.wait('Reconnect succeeds in the existing workspace despite pending folder changes', lambda d:
                          overleaf_session_preserved(d, before, reconnected=True)
                          and overleaf_state(d)['sync_ready'] and not overleaf_state(d)['source_locked']
                          and overleaf_storage(d, keep=True, directory=str(alternate)), timeout=20)['document']
        self.open_overleaf_connection()
        self.click('overleaf.reset_storage')
        self.wait('Reset restores active storage settings and removes pending controls', lambda d:
                  overleaf_session_preserved(d, ready)
                  and overleaf_state(d)['storage_keep_local_copy'] == previous['storage_keep_local_copy']
                  and overleaf_state(d)['storage_directory_fingerprint'] == previous['storage_directory_fingerprint']
                  and overleaf_pending_storage_targets(self.snapshot(), pending=False))
        if (alternate.exists() or not persistent.is_dir()
                or fixture.source != source or fixture.save_count != writes):
            raise AssertionError('Storage controls or reconnect replaced files or published the unsaved draft')
        self.click('overleaf.hide')
        self.wait('The original selected dirty source remains editable after Reset', lambda d:
                  overleaf_session_preserved(d, ready)
                  and not overleaf_state(d)['visible'] and d['focused'])
        self.record('overleaf.pending_storage_reconnect', remote_account=False,
                    relocation_cancelled=True, current_workspace_preserved=True,
                    draft_and_selection_preserved=True, fresh_hydration=True,
                    storage_reset=True, no_server_write=True)

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
        self.overleaf_authentication(fixture)
        self.overleaf_pending_storage_reconnect(fixture, persistent)
        self.overleaf_chooser_roundtrip()
        self.overleaf_workspace_creation(fixture)
        self.overleaf_save_fixture(fixture, seed)
        self.overleaf_page_departure(fixture, seed)
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
