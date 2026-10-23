"""Real Return keys, literal whitespace, paste and undo in named source files."""
from copy import deepcopy
from pathlib import Path

from desktop_ui_settings_links_journeys import (
    SOURCE_FIELDS, assert_preferences_preserved,
)

INDENTATION_JOURNEYS = ('indentation',)
INDENT_PREFERENCES = ('auto_save', 'auto_pair_delimiters', 'indent_spaces')


def indentation_cases(suffix, unit='  '):
    """Literal expected outputs; source is loaded from disk, never typed setup."""
    def case(name, source, expected, cursor=None, expected_cursor=None):
        return dict(name=name, source=source, expected=expected,
                    cursor=len(source) if cursor is None else cursor,
                    expected_cursor=len(expected) if expected_cursor is None else expected_cursor)
    if suffix == 'typ':
        pair = '#let f() = {}'
        pair_prefix = '#let f() = {\n' + unit
        return [
            case('block', '\t #let f() = {', '\t #let f() = {\n\t ' + unit),
            case('literal-prefix', ' \t  α', ' \t  α\n \t  '),
            case('paired-block', pair, pair_prefix + '\n}', len(pair) - 1, len(pair_prefix)),
            case('closed-block', '#let f() = {\n  1\n}', '#let f() = {\n  1\n}\n'),
        ]
    if suffix == 'tex':
        return [
            case('align', '\t \\begin{align}', '\t \\begin{align}\n\t ' + unit),
            case('document', '\\begin{document}', '\\begin{document}\n'),
            case('literal-prefix', ' \t  \\alpha', ' \t  \\alpha\n \t  '),
            case('crlf', '  first\r\n\t second', '  first\r\n\t second\r\n\t '),
        ]
    raise AssertionError('Indentation journey requires a real .typ or .tex document')


def assert_indentation_result(document, path, expected, cursor, dirty):
    from desktop_ui_journeys import fingerprint
    if (document['path'] != str(path)
            or document['source_fingerprint'] != fingerprint(expected)
            or document['source_bytes'] != len(expected.encode('utf-8'))
            or document['cursor'] != [cursor, cursor]
            or document['dirty'] is not dirty
            or document['editor_keyboard_focused'] is not True):
        raise AssertionError('Return/paste did not preserve exact source, caret and keyboard ownership')


def assert_original_document_restored(document, before):
    # Tab reactivation deliberately advances the revision clock to reject old
    # background results (DocumentSession.reactivate_after). Unlike a reference
    # click, this journey switches tabs; identity/content must remain exact,
    # while its clock must advance rather than be rewound to the old revision.
    if any(document[key] != before[key] for key in SOURCE_FIELDS if key != 'revision'):
        raise AssertionError('Indentation journey changed the original source or preview owner')
    revisions = (before['revision'], document['revision'])
    if (any(not isinstance(value, str) or not value.isascii() or not value.isdecimal()
            or not 0 <= int(value) < 2 ** 64 for value in revisions)
            or int(revisions[1]) <= int(revisions[0])):
        raise AssertionError('Returning to the original tab did not advance its revision clock')


def indentation_preference_edits(document, desired):
    """Reject missing/invalid observations before posting settings input."""
    edits = []
    for name, value in desired.items():
        if name not in INDENT_PREFERENCES:
            raise AssertionError('Unknown indentation preference')
        old = document.get(name)
        expected_type = int if name == 'indent_spaces' else bool
        if type(old) is not expected_type or type(value) is not expected_type:
            raise AssertionError('Missing or invalid indentation-preference observation')
        if name == 'indent_spaces' and not (0 <= old <= 16 and 0 <= value <= 16):
            raise AssertionError('Unsupported indentation size')
        if old != value:
            literal = lambda item: str(item).lower() if type(item) is bool else str(item)
            edits.append(('"' + name + '": ' + literal(old),
                          '"' + name + '": ' + literal(value)))
    return edits


class IndentationJourneys:
    def indentation_preferences(self, desired):
        """Use the real validated editor, preserving arbitrary incoming values."""
        replacements = indentation_preference_edits(self.snapshot()['document'], desired)
        if replacements:
            self.tex_settings(replacements)
        self.wait('Indentation preferences apply through Settings', lambda d:
                  all(d[name] == value for name, value in desired.items()))

    def indentation_case(self, suffix, case, phase):
        count = self.snapshot()['document']['tabs']
        path = self.directory / ('indent-' + phase + '-' + case['name'] + '.' + suffix)
        disk = case['source'].encode('utf-8')
        path.write_bytes(disk)
        self.open_harper_fixture(path)
        self.source_is(case['source'])
        self.key(125)  # Cmd+Down: place a real caret at the end of the file.
        for _ in range(len(case['source']) - case['cursor']):
            self.key(123, 'none')
        document = self.wait('Named ' + suffix + ' fixture has the exact pre-Return caret', lambda d:
                             d['path'] == str(path) and d['cursor'] == [case['cursor']] * 2
                             and d['editor_keyboard_focused'] and not d['dirty'])['document']
        assert_indentation_result(document, path, case['source'], case['cursor'], False)
        self.key(36, 'none')  # One native Return, not a pasted newline.
        self.source_is(case['expected'])
        document = self.wait('Return leaves the caret after its exact indentation', lambda d:
                             d['cursor'] == [case['expected_cursor']] * 2)['document']
        assert_indentation_result(document, path, case['expected'], case['expected_cursor'], True)
        self.key(6)  # Cmd+Z must undo the entire Return and its generated spaces.
        self.source_is(case['source'])
        self.wait('Undo restores the saved fixture without partial indentation', lambda d:
                  not d['dirty'] and d['editor_keyboard_focused'])
        self.key(6, 'cmd+shift')
        self.source_is(case['expected'])
        document = self.wait('Redo restores Return and its generated caret together', lambda d:
                             d['cursor'] == [case['expected_cursor']] * 2)['document']
        assert_indentation_result(document, path, case['expected'], case['expected_cursor'], True)
        self.key(6)
        self.source_is(case['source'])
        self.wait('Fixture is clean before closing', lambda d: not d['dirty'])
        if path.read_bytes() != disk:
            raise AssertionError('Indentation journey wrote a changed fixture despite disabled autosave')
        self.record('editor.indentation.return', syntax=suffix, case=case['name'], phase=phase,
                    cursor=case['expected_cursor'], undo_redo=True, native_return=True)
        self.close_scratch(count)

    def indentation_paste(self, suffix):
        count = self.snapshot()['document']['tabs']
        path = self.directory / ('indent-paste.' + suffix)
        path.write_bytes(b'')
        self.open_harper_fixture(path)
        pasted = '\t  pasted {\n \t    literal $value$\n  }\n'
        self.paste_source(pasted)
        document = self.wait('Multiline paste stays verbatim with automatic pairing enabled', lambda d:
                             d['cursor'] == [len(pasted)] * 2)['document']
        assert_indentation_result(document, path, pasted, len(pasted), True)
        self.key(6)
        self.source_is('')
        self.wait('Undo removes the whole multiline paste', lambda d: not d['dirty'])
        self.key(6, 'cmd+shift')
        self.source_is(pasted)
        document = self.wait('Redo restores the verbatim paste caret', lambda d:
                             d['cursor'] == [len(pasted)] * 2)['document']
        assert_indentation_result(document, path, pasted, len(pasted), True)
        self.key(6)
        self.source_is('')
        self.wait('Paste fixture is clean before closing', lambda d: not d['dirty'])
        if path.read_bytes() != b'':
            raise AssertionError('Pasting changed the source file despite disabled autosave')
        self.record('editor.indentation.paste', syntax=suffix, verbatim=True, undo_redo=True)
        self.close_scratch(count)

    def indentation(self):
        self.focus_document_window()
        before = deepcopy(self.snapshot()['document'])
        original = {name: before[name] for name in INDENT_PREFERENCES}
        original_path = Path(before['path'])
        original_bytes = original_path.read_bytes()
        self.settings()
        preferences = self.documentation_settings_fingerprint()
        self.close_settings()
        self.indentation_preferences(dict(auto_save=False, auto_pair_delimiters=False, indent_spaces=2))
        for suffix in ('typ', 'tex'):
            for case in indentation_cases(suffix):
                self.indentation_case(suffix, case, 'pairs-off')
        self.indentation_preferences(dict(auto_pair_delimiters=True))
        for suffix, index in (('typ', 2), ('tex', 0)):
            self.indentation_case(suffix, indentation_cases(suffix)[index], 'pairs-on')
            self.indentation_paste(suffix)
        self.indentation_preferences(dict(indent_spaces=0))
        for suffix in ('typ', 'tex'):
            self.indentation_case(suffix, indentation_cases(suffix, unit='\t')[0], 'tab-unit')
        self.indentation_preferences(original)
        self.wait('Indentation journey restores its original source tab and preview owner', lambda d:
                  d['active_tab_id'] == before['active_tab_id']
                  and d['preview_tab_id'] == before['preview_tab_id']
                  and d['preview_path'] == before['preview_path'])
        assert_original_document_restored(self.snapshot()['document'], before)
        # Opening named fixtures legitimately changes last_opened_files. A real
        # Save of the untouched original restores that history too; reselecting
        # an existing tab does not. Keep the complete settings comparison below.
        # Identical-byte saves may skip a disk write, so require the fresh
        # native Save completion receipt rather than a changed modification time.
        previous_notice = self.snapshot()['document']['completion_notice']
        self.key(1)
        self.wait('Saving the original fixture restores its file history', lambda d:
                  d['completion_notice'] != previous_notice
                  and isinstance(d['completion_notice'], str)
                  and d['completion_notice'].startswith('Saved ')
                  and original_path.name in d['completion_notice'], timeout=15)
        if original_path.read_bytes() != original_bytes:
            raise AssertionError('Restoring file history changed the original source on disk')
        assert_original_document_restored(self.snapshot()['document'], before)
        self.settings()
        assert_preferences_preserved(preferences, self.documentation_settings_fingerprint())
        self.close_settings()
        self.wait('Indentation journey restores its original source tab and preview owner', lambda d:
                  d['active_tab_id'] == before['active_tab_id']
                  and d['preview_tab_id'] == before['preview_tab_id']
                  and d['preview_path'] == before['preview_path'])
        assert_original_document_restored(self.snapshot()['document'], before)
        self.record('editor.indentation.complete', preferences_restored=True, source_unchanged=True,
                    syntax=['typ', 'tex'], automatic_pairing=[False, True], indent_units=['spaces', 'tabs'])
