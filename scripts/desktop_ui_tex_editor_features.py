"""Native TeX editor parity; observations never substitute for actual input."""
from copy import deepcopy
from pathlib import Path
import re


TEX_EDITOR_SOURCE = (
    "\\section{Editor parity}\n"
    "\\newcommand{\\parity}[1]{\n"
    "  {#1}\n"
    "}\n"
    "\\begin{align}\n"
    "  α+(β) &= γ\n"
    "\\end{align}\n"
    "\\href{https://example.test/tex-parity}{site}\n"
    "% Native pair insertion follows on the next physical line.\n"
)

TEX_ENCLOSING_SOURCE = (
    "\\begin{quote}\n"
    "Outer prose.\n"
    "  \\begin{align}\n"
    "    α + (β + γ)\n"
    "  \\end{align}\n"
    "  % Fake \\begin{ignored} (pair)\n"
    "More outer prose.\n"
    "\\end{quote}\n"
)
TYPST_ENCLOSING_SOURCE = (
    "#let enclosing(value) = {\n"
    "  let nested = (value, (2, 3))\n"
    "  // Fake (pair)\n"
    "  nested\n"
    "}\n"
)

TEX_EDITOR_PREFERENCES = {
    'auto_save': 'auto_save',
    'auto_pair_delimiters': 'auto_pair_delimiters',
    'line_numbers': 'line_numbers',
    'line_wrap': 'line_wrap',
    'sticky_context': 'sticky_context_rows',
}


def tex_editor_preference_edits(document, desired):
    """Use actual incoming values, including disabled editor features."""
    edits = []
    for name, value in desired.items():
        if (name not in TEX_EDITOR_PREFERENCES or type(value) is not bool
                or type(document.get(name)) is not bool):
            raise AssertionError('Complete TeX editor preferences are required')
        if document[name] is not value:
            field = TEX_EDITOR_PREFERENCES[name]
            edits.append(('"' + field + '": ' + str(document[name]).lower(),
                          '"' + field + '": ' + str(value).lower()))
    return edits


def tex_editor_match(document, source):
    """Require scalar source positions from the current cached render query."""
    if 'matching_delimiters' not in document:
        raise AssertionError('Cached matching-delimiter inspection is unavailable')
    ranges = document['matching_delimiters']
    if ranges is None:
        return None
    if (not isinstance(ranges, list) or len(ranges) != 2
            or any(not isinstance(pair, list) or len(pair) != 2
                   or any(type(value) is not int for value in pair)
                   or not 0 <= pair[0] < pair[1] <= len(source) for pair in ranges)
            or ranges[0][1] > ranges[1][0]):
        raise AssertionError('Matching delimiters are not ordered source-scalar ranges')
    return ranges


def tex_editor_render_ready(document):
    if type(document.get('rainbow_brackets')) is not bool:
        raise AssertionError('Effective rainbow-bracket preference is unavailable')
    syntax = document.get('highlight_syntax')
    if syntax is not None and not isinstance(syntax, str):
        raise AssertionError('Actual cached syntax inspection is malformed')
    return document.get('document_kind') == 'Tex' and syntax in ('TeX', 'LaTeX')


def editor_pair_result(document, source, position, expected, kind):
    """A matching range alone cannot certify the current source or caret."""
    from desktop_ui_journeys import fingerprint
    cursor = document.get('cursor')
    return (document.get('document_kind') == kind
            and document.get('source_fingerprint') == fingerprint(source)
            and type(document.get('source_bytes')) is int
            and document['source_bytes'] == len(source.encode('utf-8'))
            and isinstance(cursor, list) and len(cursor) == 2
            and all(type(value) is int for value in cursor)
            and cursor == [position, position]
            and document.get('editor_keyboard_focused') is True
            and tex_editor_match(document, source) == expected)


def tex_editor_folds(document):
    headers, collapsed = document.get('fold_headers'), document.get('fold_collapsed')
    if (not isinstance(headers, list) or not isinstance(collapsed, list)
            or any(type(line) is not int or line < 0 for line in headers + collapsed)
            or len(headers) != len(set(headers)) or len(collapsed) != len(set(collapsed))
            or not set(collapsed).issubset(headers)):
        raise AssertionError('Complete TeX fold ownership inspection is required')
    return headers, collapsed


def tex_editor_owner_restored(document, before):
    """Switching tabs may advance the revision; source and ownership stay exact."""
    fields = ('path', 'tabs', 'active_tab_id', 'preview_tab_id', 'preview_path',
              'source_fingerprint', 'source_bytes', 'cursor', 'dirty', 'view_mode', 'revision')
    if any(field not in state for state in (document, before) for field in fields):
        raise AssertionError('Source and preview ownership inspection is incomplete')
    for state in (document, before):
        if (not isinstance(state['path'], str)
                or not isinstance(state['source_fingerprint'], str)
                or re.fullmatch(r'[0-9a-f]{16}', state['source_fingerprint']) is None
                or type(state['source_bytes']) is not int or state['source_bytes'] < 0
                or type(state['tabs']) is not int or state['tabs'] < 1
                or type(state['active_tab_id']) is not int or state['active_tab_id'] < 0
                or state['preview_tab_id'] is not None and (type(state['preview_tab_id']) is not int or state['preview_tab_id'] < 0)
                or state['preview_path'] is not None and not isinstance(state['preview_path'], str)
                or type(state['dirty']) is not bool
                or state['view_mode'] not in ('Code', 'Split', 'Preview')
                or not isinstance(state['revision'], str) or not state['revision'].isascii()
                or not state['revision'].isdecimal() or not 0 <= int(state['revision']) < 2 ** 64
                or not isinstance(state['cursor'], list) or len(state['cursor']) != 2
                or any(type(at) is not int or at < 0 for at in state['cursor'])):
            raise AssertionError('Source and preview ownership inspection is malformed')
    return (int(document['revision']) > int(before['revision'])
            and all(document[field] == before[field] for field in fields if field != 'revision'))


def tex_editor_disk_receipt(path):
    """Observe a new atomic write even when its saved bytes are unchanged."""
    stat = path.stat()
    return stat.st_dev, stat.st_ino, stat.st_mtime_ns, stat.st_ctime_ns


class TexEditorFeaturesJourneys:
    def tex_editor_replace_fixture(self, source):
        self.click('editor.source')
        self.key(0)
        self.paste_source(source)
        self.source_is(source)

    def tex_editor_expect_pair(self, source, position, expected, kind):
        before = self.snapshot()['document']
        self.tex_editor_caret(source, position)
        current = lambda d: (editor_pair_result(d, source, position, expected, kind)
                             and d['path'] == before['path']
                             and d['active_tab_id'] == before['active_tab_id']
                             and d['revision'] == before['revision'])
        self.wait('The cached innermost pair follows the caret without editing source', current)
        self.stable_state(current, frames=4)

    def tex_editor_math_commands(self):
        from desktop_ui_journeys import fingerprint
        end = len(TEX_EDITOR_SOURCE)
        for opening, closing in [('[', ']'), ('(', ')')]:
            self.key(125, 'cmd')
            self.native('text', '\\')
            self.native('text', opening)
            paired = TEX_EDITOR_SOURCE + '\\' + opening + '\\' + closing
            pair = [[end, end + 2], [end + 2, end + 4]]
            self.wait('Typing a TeX math command inserts its complete closer', lambda d:
                      editor_pair_result(d, paired, end + 2, pair, 'Tex'))
            self.key(51, 'none')
            self.source_is(TEX_EDITOR_SOURCE)
            self.key(6)
            self.wait('One Undo restores both deleted TeX math commands', lambda d:
                      editor_pair_result(d, paired, end + 2, pair, 'Tex'))
            self.key(6, 'cmd+shift')
            self.source_is(TEX_EDITOR_SOURCE)
            self.native('text', '\\')
            self.native('text', opening)
            self.native('text', 'alpha + beta')
            working = TEX_EDITOR_SOURCE + '\\' + opening + 'alpha + beta' + '\\' + closing
            close = len(working) - 2
            pair = [[end, end + 2], [close, close + 2]]
            self.source_is(working)
            self.tex_editor_expect_pair(working, end + 2 + len('alpha + '), pair, 'Tex')
            self.tex_editor_caret(working, close)
            revision = self.snapshot()['document']['revision']
            for character, position in [('\\', close + 1), (closing, close + 2)]:
                self.native('text', character)
                self.wait('Typing a generated TeX closer skips without a source edit', lambda d:
                          d['source_fingerprint'] == fingerprint(working)
                          and d['cursor'] == [position, position] and d['revision'] == revision)
            self.tex_editor_caret(working, close)
            self.native('text', closing)
            self.wait('Typing only the closing symbol skips the whole TeX command', lambda d:
                      d['source_fingerprint'] == fingerprint(working)
                      and d['cursor'] == [close + 2, close + 2] and d['revision'] == revision)
            self.tex_editor_replace_fixture(TEX_EDITOR_SOURCE)
        pasted = TEX_EDITOR_SOURCE + '\\['
        self.tex_editor_replace_fixture(pasted)
        self.stable_state(lambda d: d['source_fingerprint'] == fingerprint(pasted), frames=4)
        self.tex_editor_replace_fixture(TEX_EDITOR_SOURCE)
        self.record('tex.editor.math_commands', display_and_inline=True,
                    complete_closer=True, skip_without_edit=True,
                    delete_atomic=True, undo_redo=True, paste_literal=True)

    def tex_editor_environment_enter(self):
        from desktop_ui_journeys import fingerprint
        spaces = self.snapshot()['document']['indent_spaces']
        if type(spaces) is not int or not 0 <= spaces <= 16:
            raise AssertionError('Effective native indentation width is unavailable')
        unit = '\t' if spaces == 0 else ' ' * spaces
        for name, base, body_unit in [('align', '', unit), ('document', '', ''),
                                      ('verbatim', '', unit), ('aligned', '  ', unit)]:
            context = TEX_EDITOR_SOURCE + ('\\begin{align}\n' if name == 'aligned' else '')
            initial = context + base + '\\begin{' + name + '}'
            expected = initial + '\n' + base + body_unit + '\n' + base + '\\end{' + name + '}'
            position = len(initial) + 1 + len(base + body_unit)
            self.tex_editor_replace_fixture(initial)
            self.key(125, 'cmd')
            self.key(36, 'none')
            self.wait('Enter inserts one matching environment end at its opening indentation', lambda d:
                      d['source_fingerprint'] == fingerprint(expected)
                      and d['source_bytes'] == len(expected.encode('utf-8'))
                      and d['cursor'] == [position, position])
            self.key(6)
            self.source_is(initial)
            self.key(6, 'cmd+shift')
            self.source_is(expected)
        initial = TEX_EDITOR_SOURCE + '\\begin{align}\n  x\n\\end{align}'
        position = len(TEX_EDITOR_SOURCE + '\\begin{align}')
        expected = initial[:position] + '\n' + unit + initial[position:]
        self.tex_editor_replace_fixture(initial)
        self.tex_editor_caret(initial, position)
        self.key(36, 'none')
        self.source_is(expected)
        self.tex_editor_preferences(dict(auto_pair_delimiters=False))
        initial = TEX_EDITOR_SOURCE + '\\begin{align}'
        self.tex_editor_replace_fixture(initial)
        self.key(125, 'cmd')
        self.key(36, 'none')
        self.source_is(initial + '\n' + unit)
        self.native('text', '\\')
        self.native('text', '[')
        self.source_is(initial + '\n' + unit + '\\[')
        self.tex_editor_preferences(dict(auto_pair_delimiters=True))
        self.tex_editor_replace_fixture(TEX_EDITOR_SOURCE)
        self.record('tex.editor.environment_enter', exact_indent=True,
                    document_zero_extra_indent=True, literal_environment=True,
                    nested_environment=True, existing_end_not_duplicated=True, undo_redo=True,
                    disabled_preference_respected=True)

    def tex_editor_enclosing_pairs(self):
        count = self.snapshot()['document']['tabs']
        for suffix, kind, source in [('tex', 'Tex', TEX_ENCLOSING_SOURCE),
                                     ('typ', 'Typst', TYPST_ENCLOSING_SOURCE)]:
            path = self.directory / ('enclosing-pairs.' + suffix)
            path.write_text(source, encoding='utf-8')
            self.open_harper_fixture(path)
            self.source_is(source)
            if kind == 'Tex':
                self.wait('Environment match capture uses the actual TeX renderer', tex_editor_render_ready)
                outer = [[source.index('\\begin{quote}'), source.index('\\begin{quote}') + len('\\begin{quote}')],
                         [source.index('\\end{quote}'), source.index('\\end{quote}') + len('\\end{quote}')]]
                inner = [[source.index('\\begin{align}'), source.index('\\begin{align}') + len('\\begin{align}')],
                         [source.index('\\end{align}'), source.index('\\end{align}') + len('\\end{align}')]]
                left, right = source.index('(β'), source.index('γ)') + 1
                cases = [(source.index('β') + 2, [[left, left + 1], [right, right + 1]]),
                         (right + 1, [[left, left + 1], [right, right + 1]]),
                         (right + 2, inner), (source.index('α'), inner),
                         (source.index('align') + 2, inner),
                         (source.index('More outer') + 4, outer),
                         (source.index('ignored') + 3, outer)]
            else:
                left, right = source.index('(2'), source.index('3)') + 1
                outside_left, outside_right = source.index('(value,'), source.index('3))') + 2
                outer = [[source.index('{'), source.index('{') + 1],
                         [source.rindex('}'), source.rindex('}') + 1]]
                cases = [(source.index('2, 3') + 3, [[left, left + 1], [right, right + 1]]),
                         (source.index('value,') + 3, [[outside_left, outside_left + 1],
                                                     [outside_right, outside_right + 1]]),
                         (source.index('nested\n') + 3, outer),
                         (source.index('Fake (pair)') + 8, outer)]
            for index, (position, pair) in enumerate(cases):
                self.tex_editor_expect_pair(source, position, pair, kind)
                if kind == 'Tex' and index in (0, 3) and self.capture_review:
                    self.capture_viewport('main')
            if path.read_text(encoding='utf-8') != source:
                raise AssertionError('Enclosing-pair navigation changed its saved source')
            self.close_scratch(count)
        self.record('editor.innermost_pairs', tex_environments=True, typst_blocks=True,
                    persistent_inside_body=True, nested_pair_priority=True,
                    comment_tokens_ignored=True, stable_frames=True, source_unchanged=True)

    def tex_editor_restore_file_history(self, path, before):
        # Saving also starts formatting, which may replace its completion notice
        # immediately. Observe the actual private fixture write instead.
        receipt = tex_editor_disk_receipt(path)
        self.key(1)
        self.wait('Saving the unchanged original restores file history', lambda d:
                  tex_editor_disk_receipt(path) != receipt
                  and d['path'] == str(path) and d['dirty'] is False
                  and d['source_fingerprint'] == before['source_fingerprint']
                  and d['source_bytes'] == before['source_bytes'], timeout=15)
        self.record('tex.editor.original_save', durable_write_observed=True,
                    original_source_preserved=True)
        if before.get('tex_formatter') == 'badness':
            self.wait('Badness no-edit formatting after Save completes successfully', lambda d:
                      d['formatting_phase'] == 'idle'
                      and d['save_operation'] is None
                      and d['completion_notice'] == 'No formatting changes'
                      and d['path'] == str(path) and d['dirty'] is False
                      and d['source_fingerprint'] == before['source_fingerprint'], timeout=20)
            self.record('tex.editor.format_noop', provider='Badness',
                        successful_no_edit_result=True, source_unchanged=True)

    def tex_editor_preferences(self, desired):
        edits = tex_editor_preference_edits(self.snapshot()['document'], desired)
        if edits:
            self.tex_settings(edits)
        self.wait('TeX editor preferences are effective', lambda d:
                  all(d[name] is value for name, value in desired.items()))

    def tex_editor_caret(self, source, position):
        """Move a native caret without assuming bytes or a particular wrap width."""
        if type(position) is not int or not 0 <= position <= len(source):
            raise AssertionError('Invalid native TeX fixture caret')
        self.click('editor.source')
        if self.snapshot()['document']['line_wrap']:
            forward = position <= len(source) // 2
            self.key(126 if forward else 125, 'cmd')
            for _ in range(position if forward else len(source) - position):
                self.key(124 if forward else 123, 'none')
            self.wait('Wrapped TeX fixture caret has exact scalar ownership', lambda d:
                      d['cursor'] == [position, position] and d['editor_keyboard_focused'])
            return
        self.key(126, 'cmd')
        prefix = source[:position]
        for _ in range(prefix.count('\n')):
            self.key(125, 'none')
        self.key(123, 'cmd')
        for _ in range(len(prefix.rsplit('\n', 1)[-1])):
            self.key(124, 'none')
        self.wait('TeX fixture caret has exact scalar ownership', lambda d:
                  d['cursor'] == [position, position] and d['editor_keyboard_focused'])

    def tex_editor_features(self):
        from desktop_ui_journeys import fingerprint
        from desktop_ui_settings_links_journeys import assert_preferences_preserved

        self.focus_document_window()
        before = deepcopy(self.snapshot()['document'])
        if before.get('document_kind') != 'Tex' or before.get('dirty') is not False:
            raise AssertionError('TeX editor parity starts on its clean named journey fixture')
        original_path = Path(before['path'])
        original_bytes = original_path.read_bytes()
        original_source = original_bytes.decode('utf-8')
        if before['source_fingerprint'] != fingerprint(original_source):
            raise AssertionError('Original source fixture does not match its saved bytes')
        original_preferences = {field: before[field] for field in TEX_EDITOR_PREFERENCES}
        original_pause = before.get('compilation_paused')
        if type(original_pause) is not bool:
            raise AssertionError('Automatic-compilation state is unavailable')
        self.settings()
        preferences = self.documentation_settings_fingerprint()
        self.close_settings()
        self.tex_editor_preferences(dict(auto_save=False, auto_pair_delimiters=True,
                                         line_numbers=True, line_wrap=False, sticky_context=True))
        if not original_pause:
            self.key(15, 'cmd+shift')
            self.wait('Compilation pauses for the source-only TeX fixture', lambda d: d['compilation_paused'])
        self.key(19)
        self.wait('The private TeX editor fixture has the full code viewport', lambda d: d['view_mode'] == 'Code')

        path = self.directory / 'tex-editor-features.tex'
        path.write_text(TEX_EDITOR_SOURCE, encoding='utf-8')
        count = before['tabs']
        self.open_harper_fixture(path)
        self.source_is(TEX_EDITOR_SOURCE)
        self.wait('Native TeX uses the actual TeX highlighter', tex_editor_render_ready)
        if self.snapshot()['document']['preview_tab_id'] != before['preview_tab_id']:
            raise AssertionError('Opening the editor fixture changed the pinned preview')

        end = len(TEX_EDITOR_SOURCE)
        self.key(125, 'cmd')
        self.wait('TeX pair insertion starts after the final line', lambda d: d['cursor'] == [end, end])
        self.native('text', '{')
        paired = TEX_EDITOR_SOURCE + '{}'
        expected_pair = [[end, end + 1], [end + 1, end + 2]]
        paired_state = self.wait('TeX typing creates an atomic pair and cached match', lambda d:
                                 d['source_fingerprint'] == fingerprint(paired)
                                 and d['source_bytes'] == len(paired.encode('utf-8'))
                                 and d['cursor'] == [end + 1] * 2
                                 and tex_editor_match(d, paired) == expected_pair)['document']
        if self.capture_review:
            self.capture_viewport('main')
        self.native('text', '}')
        self.wait('TeX closer skips without adding text or advancing revision', lambda d:
                  d['source_fingerprint'] == fingerprint(paired)
                  and d['cursor'] == [end + 2] * 2 and d['revision'] == paired_state['revision'])
        self.key(123, 'none')
        self.wait('TeX empty pair has a collapsed inner caret', lambda d: d['cursor'] == [end + 1] * 2)
        self.key(51, 'none')
        self.source_is(TEX_EDITOR_SOURCE)
        self.wait('Deleting a TeX pair restores clean saved contents', lambda d: not d['dirty'])
        self.key(6)
        self.source_is(paired)
        self.wait('One Undo restores the deleted pair and its inner caret', lambda d: d['cursor'] == [end + 1] * 2)
        self.key(6)
        self.source_is(TEX_EDITOR_SOURCE)
        self.key(6, 'cmd+shift')
        self.source_is(paired)
        self.key(6, 'cmd+shift')
        self.source_is(TEX_EDITOR_SOURCE)
        self.record('tex.editor.pairs', automatic=True, skip_without_edit=True,
                    delete_atomic=True, undo_redo=True, cached_match=expected_pair)

        self.key(125, 'cmd')
        self.native('text', '$')
        self.source_is(TEX_EDITOR_SOURCE + '$$')
        self.native('text', '$')
        self.source_is(TEX_EDITOR_SOURCE + '$$$$')
        self.wait('The second dollar creates an empty TeX display pair', lambda d:
                  d['cursor'] == [end + 2] * 2)
        self.key(6)
        self.source_is(TEX_EDITOR_SOURCE + '$$')
        self.key(6)
        self.source_is(TEX_EDITOR_SOURCE)

        self.tex_editor_math_commands()
        self.tex_editor_environment_enter()
        self.tex_editor_enclosing_pairs()

        self.tex_editor_caret(TEX_EDITOR_SOURCE, 0)
        headers = self.wait('TeX sections, definitions and environments have real fold headers', lambda d:
                            tex_editor_folds(d)[0] == [0, 1, 4])['document']['fold_headers']
        for _ in range(2):
            self.key(37, 'cmd+alt')
            self.wait('The TeX fold shortcut conceals its current section', lambda d:
                      tex_editor_folds(d) == (headers, [0]))
            self.stable_state(lambda d: tex_editor_folds(d) == (headers, [0]), frames=4)
            self.key(37, 'cmd+alt')
            self.wait('The TeX fold shortcut restores the same section', lambda d:
                      tex_editor_folds(d) == (headers, []))
        self.tex_parity_command('Collapse all folds', 'editor.collapse_all')
        self.wait('Palette collapse applies to every real TeX fold', lambda d:
                  tex_editor_folds(d) == (headers, headers))
        self.wait('TeX editor focus returns after collapse', lambda d: d['editor_keyboard_focused'])
        self.tex_parity_command('Expand all folds', 'editor.expand_all')
        self.wait('Palette expand restores every TeX fold', lambda d:
                  tex_editor_folds(d) == (headers, []))
        self.wait('TeX editor focus returns after expand', lambda d: d['editor_keyboard_focused'])
        self.key(17, 'cmd+alt')
        self.wait('TeX sticky context can be disabled from its shortcut', lambda d: d['sticky_context'] is False)
        self.tex_parity_command('Toggle sticky context', 'editor.sticky_context')
        self.wait('TeX sticky context can be restored from the palette', lambda d:
                  d['sticky_context'] is True and d['editor_keyboard_focused'])
        self.source_is(TEX_EDITOR_SOURCE)
        self.record('tex.editor.structure', folds=headers, repeated_shortcut=True,
                    palette_collapse_expand=True, sticky_shortcut_palette=True)

        link = TEX_EDITOR_SOURCE.index('https://') + len('https://example')
        self.tex_editor_caret(TEX_EDITOR_SOURCE, link)
        self.wait_target('editor.caret')
        target = self.snapshot()['targets']['editor.caret']
        self.right_click_at(target['x'], target['y'])
        self.wait_target('menu.Open Link in Browser')
        self.key(53, 'none')  # Inspect the real action; do not launch an external browser.
        self.wait('Dismissing the TeX link menu preserves its source and focus', lambda d:
                  d['editor_keyboard_focused'] and d['source_fingerprint'] == fingerprint(TEX_EDITOR_SOURCE))
        self.record('tex.editor.link', native_context_menu=True, browser_not_opened=True)

        if path.read_bytes() != TEX_EDITOR_SOURCE.encode('utf-8'):
            raise AssertionError('TeX editor parity wrote edits despite disabled autosave')
        self.close_scratch(count)
        self.tex_editor_preferences(original_preferences)
        if self.snapshot()['document']['compilation_paused'] is not original_pause:
            self.key(15, 'cmd+shift')
            self.wait('The original compilation pause is restored', lambda d:
                      d['compilation_paused'] is original_pause)
        # A real identical-byte Save restores last_opened_files, as in the
        # existing indentation journey. Never save a user draft.
        self.tex_editor_restore_file_history(original_path, before)
        self.settings()
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.close_settings()
        self.tex_editor_caret(original_source, before['cursor'][1])
        direction = 124 if before['cursor'][0] >= before['cursor'][1] else 123
        for _ in range(abs(before['cursor'][0] - before['cursor'][1])):
            self.key(direction, 'shift')
        self.key({'Code': 19, 'Split': 20, 'Preview': 21}[before['view_mode']])
        self.wait('TeX parity restores the original layout', lambda d: d['view_mode'] == before['view_mode'])
        self.wait('TeX parity restores exact source, caret and pinned preview ownership', lambda d:
                  tex_editor_owner_restored(d, before))
        if original_path.read_bytes() != original_bytes:
            raise AssertionError('Restoring the TeX editor fixture changed its original saved source')
        self.record('tex.editor.complete', preferences_restored=True, source_restored=True,
                    pinned_preview_restored=True, native_input=True)
