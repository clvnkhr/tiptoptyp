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
    tex_source_owner_preserved, harper_problem_index, TEX_PARITY_BIBLIOGRAPHY,
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




class HarperProblemActionContracts(unittest.TestCase):
    def fixture(self):
        from desktop_ui_journeys import fingerprint
        source = "This is the the result.\nThis is an useful result.\n"
        path = Path("/private/tmp/harper-actions.typ")
        locations = [[1, 9], [2, 9]]
        rows = [{"index": 0, "provider": "Tinymist", "line": 1, "column": 9, "suppressed": False},
                {"index": 1, "provider": "Harper", "line": 2, "column": 9, "suppressed": False},
                {"index": 2, "provider": "LTeX+", "line": 1, "column": 9, "suppressed": False},
                {"index": 3, "provider": "Harper", "line": 1, "column": 9, "suppressed": False}]
        document = {"path": str(path), "source_fingerprint": fingerprint(source),
                    "source_bytes": len(source.encode("utf-8")), "writing_current": True,
                    "writing_pending": False, "writing_count": 2,
                    "writing_locations": locations, "writing_lines": [1, 2],
                    "writing_paths": [str(path)] * 2,
                    "raw_diagnostic_count": len(rows), "diagnostic_rows": rows}
        return source, path, locations, document

    def test_provider_and_source_location_determine_row_after_reordering(self):
        source, path, locations, document = self.fixture()
        self.assertEqual(harper_problem_index(document, source, path, locations, [1, 9]), 3)
        self.assertEqual(harper_problem_index(document, source, path, locations, [2, 9]), 1)
        document["diagnostic_rows"].reverse()
        for index, row in enumerate(document["diagnostic_rows"]):
            row["index"] = index
        self.assertEqual(harper_problem_index(document, source, path, locations, [1, 9]), 0)
        self.assertEqual(harper_problem_index(document, source, path, locations, [2, 9]), 2)

    def test_stale_missing_or_suppressed_findings_cannot_authorize_a_click(self):
        source, path, locations, document = self.fixture()
        for change in ({"writing_current": False}, {"writing_pending": True},
                       {"path": "/private/tmp/other.typ"}, {"source_fingerprint": "stale"},
                       {"source_bytes": 1}, {"writing_locations": [[1, 8], [2, 9]]}):
            with self.subTest(change=change):
                self.assertIsNone(harper_problem_index({**document, **change}, source, path, locations, [1, 9]))
        self.assertIsNone(harper_problem_index(document, source, path, locations, [1, 8]))
        document["diagnostic_rows"][3]["suppressed"] = True
        self.assertIsNone(harper_problem_index(document, source, path, locations, [1, 9]))
        document["diagnostic_rows"][3]["provider"] = "Tinymist"
        document["diagnostic_rows"][3]["suppressed"] = False
        self.assertIsNone(harper_problem_index(document, source, path, locations, [1, 9]))

    def test_ambiguous_or_malformed_row_identity_is_an_evidence_failure(self):
        source, path, locations, document = self.fixture()
        duplicate = deepcopy(document)
        duplicate["diagnostic_rows"][0]["provider"] = "Harper"
        with self.assertRaisesRegex(AssertionError, "ambiguous"):
            harper_problem_index(duplicate, source, path, locations, [1, 9])
        for field, value in (("index", True), ("index", 4), ("line", 0),
                             ("column", True), ("suppressed", 0), ("provider", 1)):
            changed = deepcopy(document)
            changed["diagnostic_rows"][3][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                harper_problem_index(changed, source, path, locations, [1, 9])
        for change in ({"diagnostic_rows": None}, {"raw_diagnostic_count": True},
                       {"raw_diagnostic_count": 3}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                harper_problem_index({**document, **change}, source, path, locations, [1, 9])

    def test_action_refuses_a_reordered_row_after_waiting_for_target(self):
        source, path, locations, document = self.fixture()
        case = self
        class Recorder(TexJourneys):
            def wait(self, description, predicate, timeout):
                case.assertTrue(predicate(document))
                return {"document": document}
            def wait_target(self, name):
                case.assertEqual(name, "problem.3.fix")
            def snapshot(self):
                changed = deepcopy(document)
                changed["diagnostic_rows"][3]["provider"] = "LTeX+"
                return {"document": changed}
            def click(self, name):
                case.fail("A row lost its source identity before the action")
        with self.assertRaisesRegex(AssertionError, "ownership"):
            Recorder().click_harper_problem_action(source, path, locations, [1, 9], "fix")

    def test_row_zero_remains_a_valid_action_target(self):
        source, path, locations, document = self.fixture()
        document["diagnostic_rows"].reverse()
        for index, row in enumerate(document["diagnostic_rows"]):
            row["index"] = index
        case = self
        class Recorder(TexJourneys):
            def wait(self, description, predicate, timeout):
                case.assertTrue(predicate(document))
                return {"document": document}
            def wait_target(self, name):
                case.assertEqual(name, "problem.0.dictionary")
            def snapshot(self): return {"document": document}
            def click(self, name): case.assertEqual(name, "problem.0.dictionary")
        before = Recorder().click_harper_problem_action(source, path, locations, [1, 9], "dictionary")
        self.assertIs(before, document)


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
            'subfiles': [fingerprint(str(child.resolve())),
                         fingerprint(str(child.with_name('parity-references.bib').resolve()))],
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

    def test_bibliography_is_required_once_with_its_real_file_identity(self):
        from desktop_ui_journeys import fingerprint
        main, child, document = self.fixture()
        expected = document['project_index']['subfiles']
        for subfiles in (expected[:1], expected[1:], expected + expected[1:],
                         [expected[0], fingerprint('/private/tmp/unrelated.bib')]):
            changed = deepcopy(document)
            changed['project_index']['subfiles'] = subfiles
            with self.subTest(subfiles=subfiles):
                self.assertFalse(tex_parity_index_ready(changed, main, child))

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


class TexBibliographyNavigationContracts(unittest.TestCase):
    def test_real_subfiles_row_opens_and_closes_bibliography_and_restores_each_layout(self):
        import tempfile
        from desktop_ui_journeys import fingerprint
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'chapter.tex'
            bibliography = Path(temporary) / 'parity-references.bib'
            bibliography.write_text(TEX_PARITY_BIBLIOGRAPHY)
            before = {'path': str(source), 'active_tab_id': 3,
                      'tabs': 2, 'preview_tab_id': 2,
                      'document_kind': 'Tex', 'source_fingerprint': fingerprint('unsaved source'),
                      'source_bytes': len('unsaved source'), 'cursor': [8, 4], 'dirty': True,
                      'preview_path': str(Path(temporary) / 'main.tex')}
            case = self
            class Recorder(TexJourneys):
                def __init__(self, section, adjacent):
                    self.before = {**before, 'explorer_maximized': section}
                    self.state = deepcopy(self.before)
                    self.events = []
                    self.adjacent = adjacent
                def snapshot(self): return {'document': deepcopy(self.state)}
                def click(self, target):
                    self.events.append(('click', target))
                    if target.startswith('explorer.maximize.'):
                        section = target.removeprefix('explorer.maximize.')
                        self.state['explorer_maximized'] = (
                            None if self.state['explorer_maximized'] == section else section)
                    elif target == 'tab.' + str(self.before['active_tab_id']):
                        self.state = {**self.before, 'explorer_maximized': 'Subfiles'}
                    else:
                        case.assertEqual(target, 'explorer.subfile.parity-references.bib')
                        case.assertEqual(self.state['explorer_maximized'], 'Subfiles')
                        self.state.update(path=str(bibliography), active_tab_id=4, tabs=3,
                                          source_fingerprint=fingerprint(TEX_PARITY_BIBLIOGRAPHY),
                                          source_bytes=len(TEX_PARITY_BIBLIOGRAPHY.encode('utf-8')),
                                          dirty=False)
                def key(self, code):
                    case.assertEqual(code, 13)
                    self.events.append(('close', code))
                    self.state = {**self.before, 'explorer_maximized': 'Subfiles'}
                    if self.adjacent:
                        self.state.update(path=self.before['preview_path'], active_tab_id=2,
                                          cursor=[0, 0], dirty=False)
                def wait(self, description, predicate):
                    case.assertTrue(predicate(self.state), description)
                    return {'document': deepcopy(self.state)}
                def record(self, action, **details): self.events.append((action, details))
            for section in (None, 'Subfiles', 'Files'):
                for adjacent in (False, True):
                    with self.subTest(section=section, adjacent=adjacent):
                        recorder = Recorder(section, adjacent)
                        recorder.tex_parity_bibliography(source, bibliography)
                        self.assertEqual(recorder.state, recorder.before)
                        self.assertIn(('click', 'explorer.subfile.parity-references.bib'), recorder.events)
                        self.assertIn(('close', 13), recorder.events)
                        self.assertEqual(('click', 'tab.3') in recorder.events, adjacent)
                        self.assertEqual(recorder.events[-1][0], 'tex.bibliography_subfile')
                        self.assertEqual(bibliography.read_text(), TEX_PARITY_BIBLIOGRAPHY)


if __name__ == '__main__': unittest.main()
