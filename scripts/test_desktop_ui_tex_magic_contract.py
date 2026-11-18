"""TeX magic journey failure detectors; these are not native interaction evidence."""
from copy import deepcopy
import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_tex_magic_journeys import (
    MAGIC_SNIPPETS, tex_magic_catalog, tex_magic_completed, tex_magic_invoked,
    tex_magic_pdf, tex_magic_settings, tex_magic_typst_excluded,
)
from tex_magic_fixture import (
    MAGIC_MAIN, prepare_tex_magic_fixture, tex_magic_environment,
    tex_magic_invocations, tex_magic_paths,
)


class TexMagicJourneyContracts(unittest.TestCase):
    def invocation(self, **changes):
        return dict(engine='xelatex', input_name='main.tex',
                    source_fingerprint=fingerprint(MAGIC_MAIN),
                    source_bytes=len(MAGIC_MAIN.encode('utf-8')),
                    arguments=['-no-shell-escape'], **changes)

    def test_compiler_evidence_requires_a_new_exact_source_and_safe_engine(self):
        record = self.invocation()
        def observed(records, after=0):
            return tex_magic_invoked(records, after=after, engine='xelatex',
                                     entry=Path('/project/main.tex'), source=MAGIC_MAIN)
        self.assertTrue(observed([record]))
        self.assertFalse(observed([record], after=1))
        self.assertFalse(observed([]))
        for field, value in [('engine', 'pdflatex'), ('input_name', 'chapter.tex'),
                             ('source_fingerprint', fingerprint('old source')),
                             ('source_bytes', 1), ('arguments', ['-shell-escape'])]:
            with self.subTest(field=field):
                self.assertFalse(observed([{**record, field: value}]))
        self.assertTrue(observed([{**record, 'engine': 'pdflatex'}, record], after=1))

    def catalog(self):
        return {'snippet_presets': [dict(prefix=prefix, language='tex',
                    body=fingerprint(body), enabled=True, line_start=True, scope='text')
                    for prefix, body in MAGIC_SNIPPETS.items()]}

    def test_all_five_saved_tex_snippets_are_required_without_duplicates(self):
        good = self.catalog()
        self.assertTrue(tex_magic_catalog(good))
        self.assertFalse(tex_magic_catalog({'snippet_presets': good['snippet_presets'][:-1]}))
        self.assertFalse(tex_magic_catalog({'snippet_presets': good['snippet_presets'] * 2}))
        for field, value in [('language', 'typst'), ('body', fingerprint('wrong')),
                             ('enabled', False), ('line_start', False), ('scope', 'math')]:
            changed = deepcopy(good)
            changed['snippet_presets'][0][field] = value
            with self.subTest(field=field):
                self.assertFalse(tex_magic_catalog(changed))
        for bad in ({}, {'snippet_presets': None}):
            with self.assertRaises(AssertionError):
                tex_magic_catalog(bad)

    def test_typst_exclusion_requires_current_real_owner_and_completion_evidence(self):
        path = Path('/project/snippet.typ')
        good = dict(document_kind='Typst', path=str(path),
                    source_fingerprint=fingerprint('texroot'), source_bytes=7,
                    completion_pending=False, completion_labels=['text'],
                    completion_origins=['Tinymist'])
        self.assertTrue(tex_magic_typst_excluded(good, path))
        self.assertTrue(tex_magic_typst_excluded({**good, 'completion_labels': ['texroot'],
                                                'completion_origins': ['Tinymist']}, path))
        for change in ({'document_kind': 'Tex'}, {'path': '/other.typ'},
                       {'source_fingerprint': fingerprint('stale')}, {'source_bytes': 8},
                       {'completion_pending': True},
                       {'completion_labels': ['texroot'], 'completion_origins': ['Snippet']}):
            with self.subTest(change=change):
                self.assertFalse(tex_magic_typst_excluded({**good, **change}, path))
        for change in ({'completion_labels': None}, {'completion_origins': []},
                       {'completion_labels': [42]}, {'completion_pending': 0}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                tex_magic_typst_excluded({**good, **change}, path)

    def test_global_settings_evidence_requires_a_real_fingerprint(self):
        good = dict(tex_settings_fingerprint='0123456789abcdef', tex_build_engine='pdfLaTeX')
        self.assertEqual(tex_magic_settings(good), good['tex_settings_fingerprint'])
        for change in ({'tex_settings_fingerprint': None}, {'tex_settings_fingerprint': 'old'},
                       {'tex_settings_fingerprint': '0123456789abcdeg'}, {'tex_build_engine': None}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                tex_magic_settings({**good, **change})

    def engine(self, paths, *, output=None, source=None):
        output = paths['project'] / 'build' if output is None else output
        return subprocess.run([str(paths['engines'] / 'xelatex'), '-no-shell-escape',
                               '-output-directory=' + str(output),
                               str(paths['main'] if source is None else source)],
                              capture_output=True, text=True, timeout=10)

    def test_complete_requires_root_engine_pinned_owner_rendered_pdf_and_unchanged_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = prepare_tex_magic_fixture(directory)
            self.assertEqual(self.engine(paths, output=paths['project']).returncode, 0)
            records = tex_magic_invocations(directory)
            settings = '0123456789abcdef'
            good = dict(tex_settings_fingerprint=settings, tex_build_engine='pdfLaTeX',
                        tex_magic=dict(entry=str(paths['main']), engine='XeLaTeX'),
                        path=str(paths['child']), preview_path=str(paths['child']),
                        preview_backend='PDFium', preview_pdfium_ready=True,
                        compile_indicator_spinning=False, pdf_output_pending=False,
                        pdf_output_state='Saved')
            def completed(document, after=0):
                return tex_magic_completed(document, records, after=after, engine='xelatex',
                        entry=paths['main'], source=MAGIC_MAIN, active=paths['child'],
                        preview=paths['child'], settings=settings)
            self.assertTrue(completed(good))
            self.assertFalse(completed(good, after=len(records)))
            for change in ({'tex_magic': None},
                           {'tex_magic': dict(entry=str(paths['alternate']), engine='XeLaTeX')},
                           {'tex_magic': dict(entry=str(paths['main']), engine='pdfLaTeX')},
                           {'path': str(paths['main'])}, {'preview_path': str(paths['main'])},
                           {'preview_backend': 'WebKit'}, {'preview_pdfium_ready': False},
                           {'compile_indicator_spinning': True}, {'pdf_output_pending': True},
                           {'pdf_output_state': 'Building'}):
                with self.subTest(change=change):
                    self.assertFalse(completed({**good, **change}))
            for change in ({'tex_settings_fingerprint': fingerprint('changed')},
                           {'tex_build_engine': 'XeLaTeX'}):
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    completed({**good, **change})
            paths['main'].with_suffix('.pdf').unlink()
            self.assertFalse(completed(good))

    def test_fixture_engines_are_private_and_reject_source_or_output_outside_owned_root(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as outside:
            paths = prepare_tex_magic_fixture(directory)
            before = {key: paths[key].read_bytes() for key in ('main', 'alternate', 'child')}
            for name in ('pdflatex', 'xelatex'):
                self.assertEqual(stat.S_IMODE((paths['engines'] / name).stat().st_mode), 0o700)
            self.assertEqual(self.engine(paths).returncode, 0)
            self.assertTrue(tex_magic_pdf(paths['project'] / 'build/main.pdf', 'xelatex', MAGIC_MAIN))
            self.assertFalse(tex_magic_pdf(paths['project'] / 'build/main.pdf', 'pdflatex', MAGIC_MAIN))
            self.assertFalse(tex_magic_pdf(paths['project'] / 'build/main.pdf', 'xelatex', 'old source'))
            self.assertEqual({key: paths[key].read_bytes() for key in before}, before)
            escaped_source = Path(outside) / 'outside.tex'
            escaped_source.write_text('outside source')
            for changes in (dict(source=escaped_source), dict(output=Path(outside))):
                result = self.engine(paths, **changes)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('escaped its owned fixture', result.stderr)
            self.assertEqual(len(tex_magic_invocations(directory)), 1)
            self.assertFalse((Path(outside) / 'main.pdf').exists())

    def test_pdf_must_be_complete_bounded_and_match_engine_and_source(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = prepare_tex_magic_fixture(directory)
            pdf = paths['project'] / 'build/main.pdf'
            self.assertFalse(tex_magic_pdf(pdf, 'xelatex', MAGIC_MAIN))
            self.assertEqual(self.engine(paths).returncode, 0)
            original = pdf.read_bytes()
            for data in (original[:-6], original.replace(b'%PDF-', b'WRONG'),
                         original.replace(fingerprint(MAGIC_MAIN).encode(), b'0123456789abcdef')):
                pdf.write_bytes(data)
                self.assertFalse(tex_magic_pdf(pdf, 'xelatex', MAGIC_MAIN))
            for data in (b'', b'x' * (16 * 1024)):
                pdf.write_bytes(data)
                with self.assertRaises(AssertionError):
                    tex_magic_pdf(pdf, 'xelatex', MAGIC_MAIN)

    def test_invocation_log_ignores_incomplete_final_line_and_bounds_work(self):
        with tempfile.TemporaryDirectory() as directory:
            path = tex_magic_paths(directory)['log']
            self.assertEqual(tex_magic_invocations(directory), [])
            complete = {'engine': 'xelatex'}
            path.write_bytes(json.dumps(complete).encode() + b'\n{"incomplete":')
            self.assertEqual(tex_magic_invocations(directory), [complete])
            path.write_bytes(b'x' * (256 * 1024 + 1))
            with self.assertRaises(AssertionError):
                tex_magic_invocations(directory)

    def test_fixture_path_override_preserves_input_environment_and_other_preferences(self):
        environment = dict(PATH='/usr/bin:/bin', JAVA_HOME='/original/java', CUSTOM='retained')
        before = environment.copy()
        result = tex_magic_environment('/private/tmp/fixture', environment)
        self.assertEqual(environment, before)
        self.assertEqual(result['JAVA_HOME'], before['JAVA_HOME'])
        self.assertEqual(result['CUSTOM'], before['CUSTOM'])
        self.assertEqual(result['PATH'], '/private/tmp/fixture/tex-magic-engines:/usr/bin:/bin')


if __name__ == '__main__':
    unittest.main()
