"""The optional LTeX journey must require actual fresh, mapped server results."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

from desktop_ui_journeys import fingerprint
from desktop_ui_ltex_journeys import ltex_program, ltex_results


class LtexContract(unittest.TestCase):
    def test_results_require_suggestions_source_identity_and_real_paths(self):
        source = 'This are a result.\n'
        path = Path('/tmp/ltex-fixture.tex')
        state = dict(path=str(path), source_fingerprint=fingerprint(source), ltex=dict(
            state='Up to date / idle', enabled=True, current=True, count=1,
            suggestions=1, rules=['AGREEMENT'], paths=[str(path)]))
        self.assertTrue(ltex_results(state, path, source))
        for key, value in [('current', False), ('suggestions', 0), ('rules', []),
                           ('paths', ['/tmp/.tiptoptyp/projection.tex'])]:
            saved = state['ltex'][key]
            state['ltex'][key] = value
            self.assertFalse(ltex_results(state, path, source), key)
            state['ltex'][key] = saved
        self.assertFalse(ltex_results(state, path, source + ' '))
        state['ltex']['state'] = 'Failed'
        with self.assertRaises(AssertionError):
            ltex_results(state, path, source)

    def test_missing_or_nonexecutable_server_is_unavailable(self):
        with tempfile.TemporaryDirectory() as directory:
            program = Path(directory) / 'ltex-ls-plus'
            with self.assertRaises(ValueError):
                ltex_program(program)
            program.write_text('#!/bin/sh\n')
            with self.assertRaises(ValueError):
                ltex_program(program)
            program.chmod(0o700)
            self.assertEqual(ltex_program(program), program.resolve())

    def test_all_has_no_hidden_ltex_dependency(self):
        file = Path(__file__).with_name('test-desktop-ui.py')
        spec = importlib.util.spec_from_file_location('desktop_ltex_contract_runner', file)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertNotIn('ltex', runner.selected_journeys('all'))
        self.assertEqual(runner.selected_journeys('ltex'), ['ltex'])


if __name__ == '__main__':
    unittest.main()
