"""Settings edit ordering contracts; these do not replace native interaction checks."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_tex_journeys import (
    TexJourneys, settings_json_query_selected, tex_parity_index_ready,
    tex_source_owner_preserved,
)


def selected_query(query='earlier query'):
    return {'document': {'settings_json': {'query': query}},
            'targets': {'settings.json.find': {'enabled': True, 'focused': True,
                        'age_ms': 0, 'selection': [len(query), 0]}}}


class SettingsSelectionContracts(unittest.TestCase):
    def test_selection_uses_unicode_scalar_positions_in_either_direction(self):
        self.assertTrue(settings_json_query_selected(selected_query('Tao’s α🙂')))
        self.assertTrue(settings_json_query_selected(selected_query('')))

    def test_partial_stale_unfocused_or_malformed_selection_cannot_acknowledge(self):
        good = selected_query()
        for change in ({'selection': [4, 4]}, {'selection': [0, 100]},
                       {'selection': [True, 13]}, {'selection': [0]},
                       {'selection': None}, {'selection': '0,13'},
                       {'focused': False}, {'enabled': False},
                       {'age_ms': 501}, {'age_ms': -1}, {'age_ms': True}):
            state = deepcopy(good)
            state['targets']['settings.json.find'].update(change)
            with self.subTest(change=change):
                self.assertFalse(settings_json_query_selected(state))
        self.assertFalse(settings_json_query_selected({}))

    def test_edit_waits_for_selection_and_editor_focus_before_typing_or_pasting(self):
        class Recorder(TexJourneys):
            def __init__(self, initial):
                self.state = selected_query(initial)
                self.state['document']['settings_json'].update(
                    visible=True, fingerprint='before', valid=True, saved=False)
                self.state['targets']['settings.json.find']['selection'] = [len(initial)] * 2
                self.events, self.pending, self.observations = [], False, 0
                self.editor_acknowledged = False

            def settings(self): pass
            def close_settings(self): pass
            def click(self, name):
                if name == 'settings.json.save':
                    self.state['document']['settings_json']['saved'] = True
            def snapshot(self):
                if self.pending:
                    self.observations += 1
                    if self.observations == 2:
                        query = self.state['document']['settings_json']['query']
                        self.state['targets']['settings.json.find']['selection'] = [0, len(query)]
                        self.pending = False
                        self.events.append('selection.acknowledged')
                return deepcopy(self.state)
            def key(self, code, flags='cmd'):
                if code == 0:
                    self.pending, self.observations = True, 0
                    self.events.append('select-all')
                elif code == 36:
                    self.editor_acknowledged = False
                    self.events.append('enter')
            def wait_target(self, name, focused=False):
                self.events.append(('focus.acknowledged', name, focused))
                if name == 'settings.json':
                    self.editor_acknowledged = True
            def native(self, command, text):
                self.assert_command(command)
                query = self.state['document']['settings_json']['query']
                if query and not settings_json_query_selected(self.state):
                    raise AssertionError('text arrived before Select All was consumed')
                self.events.append(('text', text))
                self.state['document']['settings_json']['query'] = text
                self.state['targets']['settings.json.find']['selection'] = [len(text)] * 2
            @staticmethod
            def assert_command(command):
                if command != 'text': raise AssertionError(command)
            def wait(self, description, predicate):
                if not predicate(self.snapshot()['document']):
                    raise AssertionError(description)
            def paste_source(self, text, check):
                if not self.editor_acknowledged:
                    raise AssertionError('paste preceded editor focus acknowledgement')
                self.events.append(('paste', text))
                self.state['document']['settings_json']['fingerprint'] += '+'
                check()

        for initial in ('', 'earlier query'):
            recorder = Recorder(initial)
            with patch('time.sleep'):
                recorder.tex_settings([('first', 'replacement1'), ('second', 'replacement2')])
            expected = 1 if not initial else 2
            self.assertEqual(recorder.events.count('select-all'), expected)
            self.assertEqual(recorder.events.count('selection.acknowledged'), expected)
            self.assertEqual([event for event in recorder.events if isinstance(event, tuple)
                              and event[0] == 'text'], [('text', 'first'), ('text', 'second')])




class TexSourceOwnershipContracts(unittest.TestCase):
    def fixture(self):
        return {'path': '/private/tmp/parity.tex', 'active_tab_id': 1,
                'document_kind': 'Tex', 'source_fingerprint': '0123456789abcdef',
                'source_bytes': 123, 'cursor': [8, 4], 'dirty': True,
                'revision': '37'}

    def test_reactivation_rekey_does_not_mean_visible_source_changed(self):
        before = self.fixture()
        after = deepcopy(before)
        after['revision'] = '40'
        self.assertTrue(tex_source_owner_preserved(after, before))
        self.assertEqual(before, self.fixture())

    def test_each_visible_source_or_owner_mismatch_is_rejected(self):
        before = self.fixture()
        for field, value in (('path', '/private/tmp/other.tex'), ('active_tab_id', 2),
                             ('document_kind', 'Typst'), ('source_fingerprint', 'fedcba9876543210'),
                             ('source_bytes', 124), ('cursor', [4, 8]), ('dirty', False)):
            after = deepcopy(before)
            after[field] = value
            with self.subTest(field=field):
                self.assertFalse(tex_source_owner_preserved(after, before))

    def test_missing_or_malformed_source_observation_cannot_pass(self):
        before = self.fixture()
        for field in before.keys() - {'revision'}:
            after = deepcopy(before)
            del after[field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                tex_source_owner_preserved(after, before)
        for field, value in (('active_tab_id', True), ('source_bytes', True), ('dirty', 1),
                             ('cursor', None), ('cursor', [False, 0]),
                             ('source_fingerprint', 'unavailable')):
            after = deepcopy(before)
            after[field] = value
            with self.subTest(malformed=field, value=value), self.assertRaises(AssertionError):
                tex_source_owner_preserved(after, before)


class TexParityIndexContracts(unittest.TestCase):
    def fixture(self):
        from desktop_ui_journeys import fingerprint
        main = Path('/private/tmp/tex-parity-main.tex')
        child = main.with_name('parity-child.tex')
        def row(name, path, line, **extra):
            return {'name': fingerprint(name), 'path': fingerprint(str(path.resolve())),
                    'line': line, **extra}
        return main, child, {'project_index': {
            'pending': False, 'warning': None,
            'outline': [row('Native parity section', main, 5, level=1),
                        row('Included parity section', child, 1, level=2)],
            'symbols': [row('\\parityname', main, 3)],
            'labels': [row('sec:parity', main, 6), row('sec:child', child, 2)],
            'references': [row('sec:parity', main, 8), row('sec:child', main, 8)],
            'packages': [fingerprint('amsmath')],
            'subfiles': [fingerprint(str(child.resolve()))],
        }}

    def test_completed_graph_requires_expected_macros_sections_and_included_source(self):
        main, child, document = self.fixture()
        self.assertTrue(tex_parity_index_ready(document, main, child))
        self.assertFalse(tex_parity_index_ready(document, main.with_name('other.tex'), child))
        self.assertFalse(tex_parity_index_ready(document, main, child.with_name('other-child.tex')))
        for field in ('outline', 'symbols', 'labels', 'references', 'packages', 'subfiles'):
            changed = deepcopy(document)
            changed['project_index'][field].clear()
            with self.subTest(field=field):
                self.assertFalse(tex_parity_index_ready(changed, main, child))
        document['project_index']['pending'] = True
        self.assertFalse(tex_parity_index_ready(document, main, child))

    def test_stale_or_malformed_index_evidence_cannot_count_as_native_parity(self):
        from desktop_ui_journeys import fingerprint
        main, child, document = self.fixture()
        for change in ({}, {'project_index': None}, {'project_index': {}},
                       {'project_index': {**document['project_index'], 'pending': 0}},
                       {'project_index': {**document['project_index'], 'packages': ['raw package name']}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                tex_parity_index_ready(change, main, child)
        for field in ('outline', 'symbols', 'labels', 'references'):
            changed = deepcopy(document)
            changed['project_index'][field][0]['line'] = True
            with self.subTest(field=field), self.assertRaises(AssertionError):
                tex_parity_index_ready(changed, main, child)
        for field in ('name', 'path'):
            changed = deepcopy(document)
            changed['project_index']['outline'][0][field] = fingerprint('unrelated')
            with self.subTest(field=field):
                self.assertFalse(tex_parity_index_ready(changed, main, child))
        changed = deepcopy(document)
        changed['project_index']['outline'][0]['level'] = 3
        self.assertFalse(tex_parity_index_ready(changed, main, child))
        document['project_index']['warning'] = fingerprint('Unreadable included source')
        self.assertFalse(tex_parity_index_ready(document, main, child))


if __name__ == '__main__': unittest.main()
