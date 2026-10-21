"""Settings edit ordering contracts; these do not replace native interaction checks."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_tex_journeys import TexJourneys, settings_json_query_selected


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


if __name__ == '__main__': unittest.main()
