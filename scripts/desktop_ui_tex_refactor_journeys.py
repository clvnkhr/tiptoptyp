"""Native TeX refactoring uses real menus and a review before source changes."""
import math
import time

TEX_REFACTOR_JOURNEYS = ('tex_refactor',)
REFACTOR_OWNER_FIELDS = ('path', 'document_kind', 'tabs', 'active_tab_id',
                         'preview_tab_id', 'preview_path', 'workspace_root_fingerprint')
REFACTOR_PHASES = ('closed', 'naming', 'reviewing', 'rejected', 'stale')

REFACTOR_SOURCE = (
    '% café α\n'
    '\\documentclass{article}\n'
    '\\newcommand{\\sq}[1]{#1^2}\n'
    '\\newcommand{\\term}{\\alpha}\n'
    '\\begin{document}\n'
    '$\\sq{ab}_i + a^\\sq{xy}$\n'
    '$\\term x_i$\n'
    '$a^{x_i+\\alpha}+z$\n'
    '% Protected \\sq{comment}\n'
    '\\verb|\\sq{raw}|\n'
    '\\end{document}\n'
)
REFACTOR_SENTINEL = '% native refactor undo sentinel\n'
REFACTOR_WORKING = REFACTOR_SOURCE + REFACTOR_SENTINEL
REFACTOR_INLINE = REFACTOR_WORKING.replace(r'\sq{ab}_i', r'ab^2_i', 1)
REFACTOR_ALL = REFACTOR_INLINE.replace(r'a^\sq{xy}', 'a^xy^2', 1)
REFACTOR_BOUNDARY = REFACTOR_WORKING.replace(r'\term x_i', r'\alpha x_i', 1)
REFACTOR_FRAGMENT = r'x_i+\alpha'
REFACTOR_EXTRACTED = REFACTOR_WORKING.replace(REFACTOR_FRAGMENT, r'\energy', 1).replace(
    r'\begin{document}', '\\newcommand{\\energy}{x_i+\\alpha}%\n\\begin{document}', 1)


def refactor_state(document):
    state = document.get('tex_refactor')
    if (not isinstance(state, dict) or state.get('phase') not in REFACTOR_PHASES
            or any(type(state.get(key)) is not bool for key in ('visible', 'current'))):
        raise AssertionError('TeX refactoring lifecycle inspection is unavailable')
    if state['visible'] is not (state['phase'] != 'closed'):
        raise AssertionError('TeX refactoring visibility disagrees with its phase')
    if state['current'] is not (state['phase'] not in ('closed', 'stale')):
        raise AssertionError('TeX refactoring source ownership disagrees with its phase')
    if 'comparison' not in state:
        raise AssertionError('TeX refactoring comparison inspection is unavailable')
    comparison = state['comparison']
    if state['phase'] == 'reviewing':
        if (not isinstance(comparison, dict)
                or any(not isinstance(comparison.get(key), str) or not comparison[key]
                       for key in ('before', 'after'))
                or type(comparison.get('occurrences')) is not int or comparison['occurrences'] <= 0):
            raise AssertionError('TeX refactoring comparison inspection is unavailable')
    elif comparison is not None:
        raise AssertionError('Only a TeX refactoring review may expose a comparison')
    return state


def refactor_source(document, owner, source):
    from desktop_ui_journeys import fingerprint
    for field in REFACTOR_OWNER_FIELDS:
        if field not in document or field not in owner:
            raise AssertionError('TeX refactoring source inspection is missing ' + field)
    return (all(type(document[field]) is type(owner[field]) and document[field] == owner[field]
                for field in REFACTOR_OWNER_FIELDS)
            and document.get('source_fingerprint') == fingerprint(source)
            and type(document.get('source_bytes')) is int
            and document['source_bytes'] == len(source.encode('utf-8')))


def refactor_review(document, owner, before, after, occurrences):
    from desktop_ui_journeys import fingerprint
    state = refactor_state(document)
    for item in (document, owner):
        if not isinstance(item.get('revision'), str) or type(item.get('dirty')) is not bool:
            raise AssertionError('TeX refactoring revision or dirty inspection is unavailable')
    return (state['phase'] == 'reviewing' and refactor_source(document, owner, before)
            and document.get('revision') == owner.get('revision')
            and document.get('dirty') is owner.get('dirty')
            and state['comparison']['before'] == fingerprint(before)
            and state['comparison']['after'] == fingerprint(after)
            and state['comparison']['occurrences'] == occurrences)


def refactor_controls(snapshot, *, accepted):
    """Diff/footer controls must belong to one fresh native child pass."""
    names = ('tex_refactor.accept', 'tex_refactor.cancel')
    if accepted:
        names += ('tex_refactor.diff',)
    controls = [snapshot.get('targets', {}).get(name) for name in names]
    if any(not isinstance(target, dict) for target in controls):
        return False
    for target in controls:
        if (type(target.get('age_ms')) not in (int, float) or not 0 <= target['age_ms'] <= 500
                or type(target.get('frame')) is not int or target['frame'] < 0
                or not isinstance(target.get('viewport'), str) or not target['viewport']
                or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                       for key in ('x', 'y', 'width', 'height'))
                or target['width'] <= 0 or target['height'] <= 0):
            return False
    accept = controls[0]
    return (accept.get('enabled') is accepted and controls[1].get('enabled') is True
            and all(target['frame'] == accept['frame'] and target['viewport'] == accept['viewport']
                    for target in controls)
            and accept['viewport'] != snapshot.get('document', {}).get('viewport')
            and (not accepted or controls[2].get('enabled') is True))


def refactor_selection_point(snapshot, start, end):
    """Use geometry from the exact keyboard-created source selection."""
    document = snapshot['document']
    if sorted(document['cursor']) != [start, end] or not document['editor_keyboard_focused']:
        raise AssertionError('TeX refactoring menu needs its exact focused selection')
    caret, editor = [snapshot.get('targets', {}).get(name)
                     for name in ('editor.ime_cursor', 'editor.source')]
    if any(not target or target.get('enabled') is not True
           or target.get('age_ms', 501) > 500 or target.get('frame') != document['frame']
           for target in (caret, editor)):
        raise AssertionError('TeX selection hit geometry is missing or stale')
    if caret.get('viewport') != editor.get('viewport'):
        raise AssertionError('TeX selection belongs to another viewport')
    if any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
           for target, keys in ((caret, ('x', 'y')), (editor, ('x', 'y', 'width', 'height')))
            for key in keys) or editor['width'] <= 0 or editor['height'] <= 0:
        raise AssertionError('TeX selection hit geometry is invalid')
    x, y = caret['x'] - 5, caret['y']
    if not (editor['x'] - editor['width'] / 2 + 1 <= x <= editor['x'] + editor['width'] / 2 - 1
            and editor['y'] - editor['height'] / 2 <= y <= editor['y'] + editor['height'] / 2):
        raise AssertionError('TeX selection point is outside the visible editor')
    return x, y


class TexRefactorJourneys:
    def refactor_cursor(self, source, start, end=None):
        self.click('editor.source')
        self.key(126, 'cmd')
        prefix = source[:start]
        line, column = prefix.count('\n'), len(prefix.rsplit('\n', 1)[-1])
        if line:
            self.native('key-burst', 125, 'none', line, 12)
        self.key(123, 'cmd')
        if column:
            self.native('key-burst', 124, 'none', column, 12)
        self.wait('Native keys reach the exact TeX refactor caret', lambda d:
                  d['cursor'] == [start, start] and d['editor_keyboard_focused'])
        if end is not None and end != start:
            remaining = end - start
            while remaining:
                count = min(remaining, 60)
                self.native('key-burst', 124, 'shift', count, 12)
                remaining -= count
            self.wait('Native keys select the complete TeX fragment', lambda d:
                      sorted(d['cursor']) == [start, end] and d['editor_keyboard_focused'])

    def refactor_menu(self, action, source, start, end=None):
        self.refactor_cursor(source, start, end)
        owner = self.snapshot()['document']
        self.wait_target('editor.ime_cursor' if end is not None else 'editor.caret')
        if end is None:
            self.right_click('editor.caret')
        else:
            self.right_click_at(*refactor_selection_point(self.snapshot(), start, end))
        self.wait_target('tex_refactor.' + action)
        if self.capture_review and action == 'inline_all':
            self.capture_viewport('popup')
        self.click('tex_refactor.' + action)
        return owner

    def wait_refactor_controls(self, *, accepted):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            if refactor_controls(state, accepted=accepted):
                return state
            time.sleep(.05)
        raise AssertionError('TeX refactor controls did not belong to one fresh child frame')


    def tex_refactor(self):
        """Atomic TeX edits require exact previews and the original source owner."""
        before = self.snapshot()['document']
        auto_save = before['auto_save']
        if auto_save:
            self.tex_settings([('"auto_save": true', '"auto_save": false')])
        path = self.directory / 'refactor-native.tex'
        if path.exists():
            raise AssertionError('TeX refactor fixture must use its own new file')
        path.write_text(REFACTOR_SOURCE, encoding='utf-8')
        self.open_harper_fixture(path)
        self.source_is(REFACTOR_SOURCE)
        self.key(125, 'cmd')
        self.native('text', REFACTOR_SENTINEL)
        self.source_is(REFACTOR_WORKING)
        owner = self.snapshot()['document']

        def current(document, source=REFACTOR_WORKING):
            return refactor_source(document, owner, source)

        def closed(document, source=REFACTOR_WORKING):
            return current(document, source) and refactor_state(document)['phase'] == 'closed'

        def review(action, after, caret, occurrences):
            admitted = self.refactor_menu(action, REFACTOR_WORKING, caret)
            self.wait('TeX refactor displays the exact unchanged source and proposed full diff', lambda d:
                      refactor_review(d, admitted, REFACTOR_WORKING, after, occurrences))
            self.wait_refactor_controls(accepted=True)
            return admitted

        def accept_and_undo(after):
            self.click('tex_refactor.accept')
            self.wait('Accept applies one TeX source transaction to the original owner', lambda d:
                      closed(d, after) and d['editor_keyboard_focused'] and d['dirty'])
            self.key(6)
            self.wait('One Undo restores the full original TeX source including the earlier edit', closed)
            self.key(6, 'cmd+shift')
            self.wait('One Redo restores the complete TeX refactoring', lambda d: closed(d, after))
            self.key(6)
            self.wait('Final Undo preserves the independent sentinel edit', closed)
            if path.read_text(encoding='utf-8') != REFACTOR_SOURCE:
                raise AssertionError('TeX refactoring bypassed disabled autosave')

        try:
            caret = REFACTOR_WORKING.index(r'\sq{ab}') + 2
            admitted = review('inline', REFACTOR_INLINE, caret, 1)
            # The sentinel is a real prior edit. Cmd+Z in the review must not
            # undo it or close the source tab behind the child window.
            self.key(6)
            self.stable_state(lambda d:
                              refactor_review(d, admitted, REFACTOR_WORKING, REFACTOR_INLINE, 1))
            if getattr(self, 'capture_review', False):
                self.capture_viewport('tex-refactor')
            self.click('tex_refactor.cancel')
            self.wait('Cancel preserves source, revision and tab ownership', lambda d:
                      closed(d) and d['revision'] == admitted['revision']
                      and d['dirty'] is admitted['dirty'])

            review('inline', REFACTOR_INLINE, caret, 1)
            self.key(13)  # Cmd+W closes the review, not its source tab.
            self.wait('The close shortcut leaves the original TeX tab open', closed)

            review('inline', REFACTOR_INLINE, caret, 1)
            accept_and_undo(REFACTOR_INLINE)
            review('inline_all', REFACTOR_ALL, caret, 2)
            accept_and_undo(REFACTOR_ALL)

            boundary_caret = REFACTOR_WORKING.index(r'\term x_i') + 2
            review('inline', REFACTOR_BOUNDARY, boundary_caret, 1)
            accept_and_undo(REFACTOR_BOUNDARY)

            protected_caret = REFACTOR_WORKING.index(r'\sq{raw}') + 2
            rejected_owner = self.refactor_menu('inline', REFACTOR_WORKING, protected_caret)
            self.wait('Literal TeX code refuses an unsafe inline request', lambda d:
                      current(d) and refactor_state(d)['phase'] == 'rejected'
                      and d['revision'] == rejected_owner['revision'])
            self.wait_refactor_controls(accepted=False)
            self.click('tex_refactor.cancel')
            self.wait('Rejecting a protected refactor leaves its source unchanged', closed)

            start = REFACTOR_WORKING.index(REFACTOR_FRAGMENT)
            admitted = self.refactor_menu('extract', REFACTOR_WORKING, start, start + len(REFACTOR_FRAGMENT))
            self.wait('Extract requests a name before proposing any source edit', lambda d:
                      current(d) and refactor_state(d)['phase'] == 'naming'
                      and d['revision'] == admitted['revision'])
            self.wait_target('tex_refactor.name', focused=True)
            self.native('text', 'energy')
            self.click('tex_refactor.preview')
            self.wait('Named extraction reviews the declaration and complete script-safe use', lambda d:
                      refactor_review(d, admitted, REFACTOR_WORKING, REFACTOR_EXTRACTED, 1))
            self.wait_refactor_controls(accepted=True)
            if getattr(self, 'capture_review', False):
                self.capture_viewport('tex-refactor')
            accept_and_undo(REFACTOR_EXTRACTED)
            self.record('tex_refactor', inline_parameter=True, script_attachment_preserved=True,
                        control_word_boundary_preserved=True, all_uses_atomic=True,
                        comment_and_literal_preserved=True, unsafe_literal_rejected=True,
                        extraction_name_and_review=True, cancel_revision_unchanged=True,
                        modal_undo_and_close_owned=True, one_step_undo_redo=True,
                        source_owner_preserved=True, disk_unchanged=True)
        finally:
            if refactor_state(self.snapshot()['document'])['visible']:
                self.click('tex_refactor.cancel')
                self.wait('Refactor cleanup closes only its review', lambda d:
                          refactor_state(d)['phase'] == 'closed')
            self.close_scratch(before['tabs'])
            if auto_save:
                self.tex_settings([('"auto_save": false', '"auto_save": true')])
