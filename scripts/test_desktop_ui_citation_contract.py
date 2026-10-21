"""Deterministic citation journey failure detectors; not native interaction evidence."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import EditorJourneys, fingerprint
from desktop_ui_citation_journeys import (
    CITATION_DOI, CITATION_EDITED, CITATION_EXISTING, CITATION_JOURNEYS, CITATION_KEY,
    CitationJourneys, citation_artifact, citation_owner_unchanged, citation_review, citation_state,
)


class CitationJourneyContracts(unittest.TestCase):
    def document(self, **changes):
        state = dict(visible=True, busy=False, review=True, completed=False,
                     query_fingerprint=fingerprint(CITATION_DOI), destination_fingerprint=fingerprint('/references.bib'),
                     key_fingerprint=fingerprint('Desktop2026'), bibtex_fingerprint=fingerprint(CITATION_EDITED),
                     insert=True, added=None, error=None)
        state.update(changes)
        return {'citation': state}

    def test_malformed_observations_cannot_look_like_success(self):
        for document in ({}, {'citation': None}, {'citation': {}}, self.document(busy=1),
                         self.document(query_fingerprint='invalid'), self.document(error=''),
                         self.document(completed=True, review=False), self.document(completed=True),
                         self.document(visible=False, busy=True), self.document(added=1)):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                citation_state(document)
        closed = {key: None for key in ('query_fingerprint', 'destination_fingerprint', 'key_fingerprint', 'bibtex_fingerprint')}
        closed.update(visible=False, busy=False, review=False, completed=False, insert=False, added=None, error=None)
        self.assertFalse(citation_state({'citation': closed})['visible'])

    def test_review_must_be_idle_incomplete_and_successful(self):
        self.assertTrue(citation_review(self.document()))
        self.assertFalse(citation_review(self.document(busy=True)))
        self.assertFalse(citation_review(self.document(review=False)))
        self.assertFalse(citation_review(self.document(completed=True, added=True)))
        with self.assertRaisesRegex(AssertionError, 'lookup failed'):
            citation_review(self.document(error='Service failed'))

    def test_bibliography_proof_requires_both_preservation_and_reviewed_fields(self):
        good = CITATION_EXISTING + '\n' + CITATION_EDITED.replace('Desktop2026', CITATION_KEY)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'references.bib'
            with self.assertRaises(AssertionError): citation_artifact(path, CITATION_EXISTING)
            path.write_text(good, encoding='utf-8')
            self.assertEqual(citation_artifact(path, CITATION_EXISTING), good)
            for bad in (CITATION_EXISTING, good.replace('The reviewed title', 'Original title'),
                        good.replace(CITATION_KEY, 'DifferentKey'), good.replace(CITATION_EXISTING, ''),
                        good + good):
                path.write_text(bad, encoding='utf-8')
                with self.assertRaises(AssertionError): citation_artifact(path, CITATION_EXISTING)

    def test_multiline_review_uses_acknowledged_paste_and_checks_received_text(self):
        contract = self
        class Input:
            def __init__(self): self.actions = []; self.value = ''
            def click(self, target): self.actions.append(('click', target))
            def wait_target(self, target, focused): contract.assertTrue(focused)
            def key(self, key): contract.assertEqual(key, 0)
            def native(self, command, value):
                contract.assertNotIn('\n', value)
                contract.assertNotIn('\r', value)
                self.actions.append((command, value)); self.value = value
            def paste_source(self, value, check):
                self.actions.append(('paste', value)); self.value = value; check()
            def wait(self, description, predicate):
                state = contract.document(bibtex_fingerprint=fingerprint(self.value))
                contract.assertTrue(predicate(state))
        for value, method in [(CITATION_EDITED, 'paste'), ('single line', 'text')]:
            with self.subTest(method=method):
                input = Input()
                CitationJourneys.citation_type(input, 'citation.bibtex', value, 'bibtex_fingerprint')
                self.assertIn((method, value), input.actions)

    def test_owner_and_preview_are_bound_and_journey_is_registered(self):
        before = dict(path='/paper.typ', source_fingerprint=fingerprint('source'), source_bytes=6,
                      tabs=1, active_tab_id=3, preview_tab_id=3, preview_path='/paper.typ')
        self.assertTrue(citation_owner_unchanged(before, before))
        for key in before:
            self.assertFalse(citation_owner_unchanged({**before, key: None}, before))
        spec = importlib.util.spec_from_file_location('desktop_ui_runner_citation_test',
                                                     Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec); spec.loader.exec_module(runner)
        self.assertTrue(issubclass(EditorJourneys, CitationJourneys))
        for journey in CITATION_JOURNEYS:
            self.assertIn(journey, runner.JOURNEY_NAMES)
            self.assertEqual(runner.selected_journeys(journey), [journey])
            self.assertNotIn(journey, runner.selected_journeys('all'))


if __name__ == '__main__': unittest.main()
