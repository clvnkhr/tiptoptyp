"""Texdoc journey failure detectors, not native UI interaction evidence."""
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import EditorJourneys, fingerprint
from desktop_ui_texdoc_journeys import (
    TEXDOC_JOURNEYS, TexdocJourneys, texdoc_owner_unchanged, texdoc_program,
    texdoc_results, texdoc_state,
)


class TexdocJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, busy=False, query_fingerprint=fingerprint('amsmath'),
                     error=None, unavailable=False, manual_paths=['/TeX docs/amsldoc.pdf'])
        state.update(changes)
        return {'tex_documentation': state}

    def test_missing_or_malformed_observations_never_pass(self):
        for document in ({}, {'tex_documentation': None}, {'tex_documentation': {}},
                         self.document(visible=1), self.document(busy=1),
                         self.document(query_fingerprint='bad'), self.document(error=''),
                         self.document(manual_paths=['relative.pdf']), self.document(manual_paths=[7]),
                         self.document(manual_paths=['/a.pdf', '/a.pdf']),
                         self.document(visible=False, busy=True),
                         self.document(visible=False, manual_paths=['/a.pdf'])):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                texdoc_state(document)
        closed = self.document(visible=False, query_fingerprint=None, manual_paths=[])
        self.assertFalse(texdoc_state(closed)['visible'])

    def test_results_require_real_idle_success_and_never_skip_errors(self):
        self.assertTrue(texdoc_results(self.document()))
        self.assertFalse(texdoc_results(self.document(busy=True)))
        self.assertFalse(texdoc_results(self.document(manual_paths=[])))
        self.assertFalse(texdoc_results(self.document(visible=False, query_fingerprint=None, manual_paths=[])))
        for changes in ({'error': 'Tool failed'}, {'unavailable': True}):
            with self.assertRaisesRegex(AssertionError, 'unavailable'):
                texdoc_results(self.document(**changes))

    def test_source_tabs_and_designated_preview_must_remain_unchanged(self):
        before = dict(path='/paper.typ', source_fingerprint=fingerprint('source'), source_bytes=6,
                      tabs=1, active_tab_id=3, preview_tab_id=3, preview_path='/paper.typ')
        self.assertTrue(texdoc_owner_unchanged(before, before))
        for key, value in [('path', '/other.typ'), ('source_fingerprint', fingerprint('edited')),
                           ('source_bytes', 7), ('tabs', 2), ('active_tab_id', 4),
                           ('preview_tab_id', 4), ('preview_path', '/manual.pdf')]:
            self.assertFalse(texdoc_owner_unchanged({**before, key: value}, before))

    def test_missing_local_tool_is_explicitly_unavailable(self):
        with patch('desktop_ui_texdoc_journeys.os.get_exec_path', return_value=['.']), \
                patch('desktop_ui_texdoc_journeys.Path.is_file', return_value=False):
            with self.assertRaisesRegex(ValueError, 'unavailable check, not a pass'):
                texdoc_program()

    def test_journey_is_registered_and_optional(self):
        spec = importlib.util.spec_from_file_location('desktop_ui_runner_texdoc_test',
                                                     Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertTrue(issubclass(EditorJourneys, TexdocJourneys))
        for journey in TEXDOC_JOURNEYS:
            self.assertIn(journey, runner.JOURNEY_NAMES)
            self.assertEqual(runner.selected_journeys(journey), [journey])
            self.assertNotIn(journey, runner.selected_journeys('all'))


if __name__ == '__main__':
    unittest.main()
