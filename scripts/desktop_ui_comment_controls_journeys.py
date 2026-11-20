"""Editable comment controls through real Settings, formatters and diagnostics."""
import re
from pathlib import Path

COMMENT_CONTROLS_JOURNEYS = ('comment_controls', 'comment_controls_settings')
DEFAULT_COMMENT_MARKERS = {
    'both': ('ttt: off', 'ttt: on'),
    'formatting': ('fmt: off', 'fmt: on'),
    'diagnostics': ('diag: off', 'diag: on'),
}
CUSTOM_COMMENT_MARKERS = {
    'both': ('all :  pause', 'all : resume'),
    'formatting': ('layout : pause', 'layout:  resume'),
    'diagnostics': ('checks :pause', 'checks :  resume'),
}
# The parser and diagnostic projection tests own the full language/channel
# matrix. Native input retains both language backends, all custom Settings
# fields, and a positive/negative witness for each independent channel.
CUSTOM_COMMENT_CHANNELS = {'typ': ('formatting', 'diagnostics'), 'tex': ('both',)}
COMMENT_MARKER_TARGETS = tuple(
    'settings.comments.' + group + '.' + switch
    for group in DEFAULT_COMMENT_MARKERS for switch in ('off', 'on')
)
COMMENT_SETTINGS_QUERIES = ('ttt:off', 'diag: off/on', '%fmt:on', '//diag:off')


def comment_settings_fields_ready(snapshot):
    targets = snapshot.get('targets')
    if not isinstance(targets, dict):
        return False
    for name in COMMENT_MARKER_TARGETS:
        target = targets.get(name)
        if not isinstance(target, dict) or target.get('enabled') is not True:
            return False
        age = target.get('age_ms')
        if type(age) not in (int, float) or not 0 <= age <= 500:
            return False
        if any(type(target.get(dimension)) not in (int, float)
               or not 0 < target[dimension] < float('inf') for dimension in ('width', 'height')):
            return False
    return True


def assert_comment_settings_preserved(document, before):
    if comment_source_owner(document) != comment_source_owner(before):
        raise AssertionError('Finding comment controls changed the document or preview owner')
    for field in ('source_fingerprint', 'source_bytes', 'revision', 'dirty', 'cursor'):
        if field not in document or field not in before or document[field] != before[field]:
            raise AssertionError('Finding comment controls changed source state: ' + field)
    if comment_marker_fingerprints(document) != comment_marker_fingerprints(before):
        raise AssertionError('Finding comment controls changed the configured markers')


def comment_marker_body(value):
    """Match fixture comments while keeping the actual configured text intact."""
    return ''.join(character for character in value if not character.isspace())


def comment_marker_lines(suffix, markers):
    """Exercise compact prefixes and whitespace inside the marker, in both languages."""
    if suffix not in ('typ', 'tex'):
        raise ValueError('Comment controls require Typst or TeX')
    prefix = '//' if suffix == 'typ' else '%'
    off, on = (comment_marker_body(value) for value in markers)
    return prefix + off, prefix + ' \t' + ' '.join(on) + '\t'


def comment_marker_fingerprints(document):
    state = document.get('comment_controls')
    if not isinstance(state, dict) or set(state) != set(DEFAULT_COMMENT_MARKERS):
        raise AssertionError('Effective comment-control preferences are unavailable')
    for pair in state.values():
        if (not isinstance(pair, dict) or set(pair) != {'off_fingerprint', 'on_fingerprint'}
                or any(not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{16}', value) is None
                       for value in pair.values())):
            raise AssertionError('Effective comment-control marker identity is incomplete')
    if len({value for pair in state.values() for value in pair.values()}) != 6:
        raise AssertionError('Effective comment-control markers are ambiguous')
    return state


def comment_markers_applied(document, expected):
    from desktop_ui_journeys import fingerprint
    state = comment_marker_fingerprints(document)
    return all(state[group][switch + '_fingerprint'] == fingerprint(value)
               for group, values in expected.items()
               for switch, value in zip(('off', 'on'), values))


def comment_format_fixture(suffix, markers, protected=True):
    """Adjacent real edits stress full-document formatter replies and boundary lines."""
    off, on = comment_marker_lines(suffix, markers)
    if suffix == 'typ':
        original = ('#let before=(1,2,3)\n' + off
                    + '\n#let protected=(4,  5,6)\n' + on + '\n#let after=(7,8,9)\n')
        expected = ('#let before = (1, 2, 3)\n' + off + '\n'
                    + ('#let protected=(4,  5,6)' if protected else '#let protected = (4, 5, 6)')
                    + '\n' + on + '\n#let after = (7, 8, 9)\n')
    elif suffix == 'tex':
        header = '\\documentclass{article}\n\\begin{document}\n\\begin{itemize}\n'
        footer = '\\end{itemize}\n\\end{document}\n'
        original = (header + '\\item Before.\n' + off
                    + '\n       \\item Protected.\n' + on + '\n\\item After.\n' + footer)
        middle = (off + '\n       \\item Protected.\n' + on if protected
                  else off + '\n  \\item Protected.\n' + on)
        expected = header + '  \\item Before.\n' + middle + '\n  \\item After.\n' + footer
    else:
        raise ValueError('Comment format fixture requires Typst or TeX')
    return original, expected


def comment_diagnostic_fixture(suffix, markers):
    from desktop_ui_tex_journeys import diagnostic_suppression_fixture
    original, active, locations = diagnostic_suppression_fixture(suffix)
    prefix = '//' if suffix == 'typ' else '%'
    off, on = comment_marker_lines(suffix, markers)
    replace = lambda source: source.replace(prefix + ' ttt: off', off).replace(prefix + ' ttt: on', on)
    return replace(original), replace(active), locations


def comment_diagnostic_result(document, source, path, locations, visible_locations, *, markers):
    """Require both real providers and their independently suppressed body rows."""
    from desktop_ui_tex_journeys import diagnostic_control_marker_lines, harper_suppression_result
    if not harper_suppression_result(document, source, path, locations, visible_locations,
                                     markers=markers):
        return False
    ltex = document['ltex']
    if ltex.get('state') == 'Failed':
        raise AssertionError('LTeX failed while checking comment controls')
    if not ltex['enabled'] or not ltex['current']:
        return False
    raw = ltex.get('locations')
    if (not isinstance(raw, list) or any(not isinstance(location, list) or len(location) != 2
            or any(type(value) is not int or value < 1 for value in location) for location in raw)
            or type(ltex.get('count')) is not int or ltex['count'] != len(raw)
            or not isinstance(ltex.get('rules'), list) or len(ltex['rules']) != len(raw)):
        raise AssertionError('LTeX raw comment-control diagnostics are incomplete')
    metadata = diagnostic_control_marker_lines(document, source, path, markers)
    if ([location for location in raw if location[0] not in metadata] != locations
            or ltex.get('paths') != [str(path)] * len(raw)):
        return False
    rows = [row for row in document['diagnostic_rows'] if row['provider'] == 'LTeX+']
    return ([[row['line'], row['column']] for row in rows] == raw
            and all(row['suppressed'] is (row['line'] in metadata
                                           or [row['line'], row['column']] not in visible_locations)
                    for row in rows))


def comment_source_owner(document):
    from desktop_ui_workspace_switch_journeys import workspace_tab_ids
    ids = workspace_tab_ids(document)
    for name in ('active_tab_id', 'preview_tab_id'):
        if type(document.get(name)) is not int or document[name] not in ids:
            raise AssertionError('Comment control source ownership is incomplete')
    if (not isinstance(document.get('path'), str) or not Path(document['path']).is_absolute()
            or not isinstance(document.get('workspace_root_fingerprint'), str)
            or re.fullmatch(r'[0-9a-f]{16}', document['workspace_root_fingerprint']) is None):
        raise AssertionError('Comment control source identity is incomplete')
    return (ids, document['active_tab_id'], document['preview_tab_id'], document['path'],
            document['workspace_root_fingerprint'])


def comment_formatted(document, before, expected):
    from desktop_ui_journeys import fingerprint
    if (type(document.get('source_bytes')) is not int or document['source_bytes'] < 0
            or not isinstance(document.get('source_fingerprint'), str)
            or re.fullmatch(r'[0-9a-f]{16}', document['source_fingerprint']) is None
            or type(document.get('modal_open')) is not bool):
        raise AssertionError('Comment formatter result inspection is incomplete')
    return (comment_source_owner(document) == comment_source_owner(before)
            and not document['modal_open']
            and document['source_fingerprint'] == fingerprint(expected)
            and document['source_bytes'] == len(expected.encode('utf-8')))


class CommentControlsJourneys:
    def comment_controls_settings(self):
        """Find the dedicated category and literal-marker searches without edits."""
        from desktop_ui_journeys import fingerprint
        from desktop_ui_settings_links_journeys import assert_preferences_preserved
        self.focus_document_window()
        before = self.snapshot()['document']
        path = Path(before['path'])
        disk_source = path.read_bytes()
        if not comment_markers_applied(before, DEFAULT_COMMENT_MARKERS):
            raise AssertionError('The isolated Settings journey must start with default markers')
        self.settings()
        self.native('resize-focused', 1100, 900)
        preferences = self.documentation_settings_fingerprint()
        self.wait_target('settings.category.Comment controls')
        self.click('settings.category.Comment controls')

        def observe_fields():
            for target in COMMENT_MARKER_TARGETS:
                self.wait_target(target)
            snapshot = self.snapshot()
            if not comment_settings_fields_ready(snapshot):
                raise AssertionError('All six comment marker fields must be visible and fresh')
            assert_comment_settings_preserved(snapshot['document'], before)

        observe_fields()
        for group, pair in DEFAULT_COMMENT_MARKERS.items():
            for switch, value in zip(('off', 'on'), pair):
                target = 'settings.comments.' + group + '.' + switch
                self.wait_target(target)
                self.click(target)
                self.wait_target(target, focused=True)
                self.key(0)
                self.copy_with_receipt(value, lambda: self.key(8))
        self.record('comment_controls.settings_category', dedicated_category=True,
                    six_native_copy_receipts=True, source_unchanged=True)
        for query in COMMENT_SETTINGS_QUERIES:
            self.setting_section(query, 'Comment controls')
            observe_fields()
            self.record('comment_controls.settings_search', query_fingerprint=fingerprint(query),
                        six_fields_fresh=True, source_unchanged=True)
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.close_settings()
        assert_comment_settings_preserved(self.snapshot()['document'], before)
        if path.read_bytes() != disk_source:
            raise AssertionError('Finding comment controls changed the source file on disk')
        self.record('comment_controls.settings_discovery_complete', preferences_unchanged=True,
                    source_unchanged=True, formatter_network_model_actions=False)

    def comment_marker_settings(self, markers, *, reset=False, verify_only=False):
        """Use the visible form and native Copy receipts, then reopen to prove persistence."""
        self.settings()
        self.native('resize-focused', 700, 900)
        self.setting_section('comment controls', 'Comment controls')
        if reset:
            self.wait_target('settings.comments.reset')
            self.click('settings.comments.reset')
        for group, pair in markers.items():
            for switch, value in zip(('off', 'on'), pair):
                target = 'settings.comments.' + group + '.' + switch
                self.wait_target(target)
                self.click(target)
                self.wait_target(target, focused=True)
                self.key(0)
                if not verify_only and not reset:
                    self.native('text', value)
                    self.key(0)
                self.copy_with_receipt(value, lambda: self.key(8))
        self.wait('The six comment markers are applied through Settings',
                  lambda d: comment_markers_applied(d, markers))
        self.close_settings()

    def comment_marker_invalid_draft(self):
        self.settings()
        self.native('resize-focused', 700, 900)
        self.setting_section('comment controls', 'Comment controls')
        target = 'settings.comments.formatting.off'
        self.wait_target(target)
        self.click(target)
        self.wait_target(target, focused=True)
        self.key(0)
        self.key(51, 'none')
        self.wait_target('settings.comments.error')
        self.stable_state(lambda d: comment_markers_applied(d, DEFAULT_COMMENT_MARKERS))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('settings')
        self.native('text', 't t t : o f f')
        self.wait_target('settings.comments.error')
        self.stable_state(lambda d: comment_markers_applied(d, DEFAULT_COMMENT_MARKERS))
        self.key(0)
        self.copy_with_receipt('t t t : o f f', lambda: self.key(8))
        self.native('text', CUSTOM_COMMENT_MARKERS['formatting'][0])
        expected = {**DEFAULT_COMMENT_MARKERS,
                    'formatting': (CUSTOM_COMMENT_MARKERS['formatting'][0],
                                   DEFAULT_COMMENT_MARKERS['formatting'][1])}
        self.wait('A corrected marker draft applies without losing its field text',
                  lambda d: comment_markers_applied(d, expected))
        self.key(0)
        self.copy_with_receipt(expected['formatting'][0], lambda: self.key(8))
        self.click('settings.comments.reset')
        self.wait('Reset restores all six effective markers after an invalid draft',
                  lambda d: comment_markers_applied(d, DEFAULT_COMMENT_MARKERS))
        self.close_settings()
        self.record('comment_controls.invalid_draft', invalid_not_applied=True,
                    whitespace_equivalent_duplicate_not_applied=True,
                    corrected_text_preserved=True, reset=True)

    def comment_format_case(self, path, suffix, markers, *, protected, channel):
        original, expected = comment_format_fixture(suffix, markers, protected)
        self.click('editor.source')
        self.key(0)
        self.paste_source(original)
        before = self.source_is(original)['document']
        self.palette_open()
        command = self.palette_query('Format document')
        if command['palette_selected'] != 'edit.format' or 'edit.format' not in command['palette_enabled']:
            raise AssertionError('Comment controls need the real Format document command')
        self.key(36, 'none')
        predicate = lambda d: comment_formatted(d, before, expected)
        self.wait('Real ' + suffix + ' formatter honors the ' + channel + ' comment channel',
                  predicate, timeout=30)
        self.stable_state(predicate)
        self.click('editor.source')
        self.key(6)
        self.source_is(original)
        if path.read_text(encoding='utf-8') != self.comment_controls_disk_sources[path]:
            raise AssertionError('Comment formatting wrote the private original despite disabled autosave')
        self.record('comment_controls.format', language=suffix, channel=channel,
                    protected=protected, outside_formatted=True, one_step_undo=True,
                    compact_and_spaced_markers=True)

    def comment_diagnostic_case(self, path, suffix, markers, *, protected, channel, configured):
        original, active, locations = comment_diagnostic_fixture(suffix, markers)
        if self.snapshot()['document']['panel'] is None:
            self.key(23)
        self.click('panel.problems')
        self.key(3)
        self.wait_target('problems.search', focused=True)
        layout = self.snapshot()['document']['problems']
        if layout['query'] or any(layout[key] is not None for key in ('source', 'type', 'category')):
            self.click('problems.reset')
        if layout['grouped']:
            self.click('problems.grouping')
        for phase, source, visible in (
                ('off', active, locations[:2] + locations[4:] if protected else locations),
                ('on', original, locations)):
            self.click('editor.source')
            self.key(0)
            self.paste_source(source)
            predicate = lambda d: comment_diagnostic_result(
                d, source, path, locations, visible, markers=configured)
            self.wait('Real ' + suffix + ' diagnostics honor ' + channel + ' ' + phase,
                      predicate, timeout=30)
            self.stable_state(predicate)
        if path.read_text(encoding='utf-8') != self.comment_controls_disk_sources[path]:
            raise AssertionError('Diagnostic comment controls changed the private saved original')
        self.record('comment_controls.diagnostics', language=suffix, channel=channel,
                    protected=protected, raw_findings_preserved=True,
                    actual_ltex_current=True, ltex_paths_and_body_suppression=True,
                    metadata_findings_hidden=True, restored=True,
                    compact_and_spaced_markers=True)

    def comment_controls(self):
        self.focus_document_window()
        before = self.snapshot()['document']
        original_owner_source = Path(before['path']).read_text(encoding='utf-8')
        count, panel = before['tabs'], before['panel']
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false'),
                           ('"formatter": "badness"', '"formatter": "tex-fmt"')])
        self.comment_marker_settings(DEFAULT_COMMENT_MARKERS, reset=True)
        self.comment_marker_invalid_draft()
        self.comment_controls_disk_sources = {}
        # Defaults exercise both native language backends before Settings edits.
        for suffix in ('typ', 'tex'):
            path = self.directory / ('comment-controls.' + suffix)
            original, _ = comment_format_fixture(suffix, DEFAULT_COMMENT_MARKERS['both'])
            path.write_text(original, encoding='utf-8')
            self.comment_controls_disk_sources[path] = original
            self.open_native_file(path)
            if suffix == 'typ':
                self.wait('Typst language server is ready for comment formatting',
                          lambda d: d['editor_lsp_ready'], timeout=30)
            self.comment_format_case(path, suffix, DEFAULT_COMMENT_MARKERS['both'],
                                     protected=True, channel='default-both')
            self.comment_diagnostic_case(path, suffix, DEFAULT_COMMENT_MARKERS['both'],
                                         protected=True, channel='default-both', configured=DEFAULT_COMMENT_MARKERS)
            self.click('editor.source')
            self.key(0)
            self.paste_source(original)
            self.close_scratch(count)
        self.comment_marker_settings(CUSTOM_COMMENT_MARKERS)
        self.comment_marker_settings(CUSTOM_COMMENT_MARKERS, verify_only=True)
        self.record('comment_controls.settings', all_six_custom_markers_persisted=True,
                    entered_whitespace_preserved=True)
        for suffix in ('typ', 'tex'):
            path = self.directory / ('comment-controls.' + suffix)
            self.open_native_file(path)
            if suffix == 'typ':
                self.wait('Typst language server returns for custom comment controls',
                          lambda d: d['editor_lsp_ready'], timeout=30)
            for channel in CUSTOM_COMMENT_CHANNELS[suffix]:
                pair = CUSTOM_COMMENT_MARKERS[channel]
                self.comment_format_case(path, suffix, pair, protected=channel != 'diagnostics',
                                         channel='custom-' + channel)
                self.comment_diagnostic_case(path, suffix, pair, protected=channel != 'formatting',
                                             channel='custom-' + channel, configured=CUSTOM_COMMENT_MARKERS)
            self.click('editor.source')
            self.key(0)
            self.paste_source(self.comment_controls_disk_sources[path])
            self.close_scratch(count)
        self.comment_marker_settings(DEFAULT_COMMENT_MARKERS, reset=True)
        self.tex_settings([('"english_grammar": true', '"english_grammar": false'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true'),
                           ('"formatter": "tex-fmt"', '"formatter": "badness"')])
        if panel is None and self.snapshot()['document']['panel'] is not None:
            self.key(23)
        elif panel is not None:
            self.click('panel.' + panel.lower())
        self.wait('Comment controls restore the exact original source owner',
                  lambda d: comment_formatted(d, before, original_owner_source)
                  and d['dirty'] == before['dirty'])
        self.record('comment_controls.complete', settings_restored=True, original_source_preserved=True)
