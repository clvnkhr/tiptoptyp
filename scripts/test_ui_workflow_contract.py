"""Keep CI selectors aligned with the maintained native and detector lanes."""
import importlib.util
from contextlib import redirect_stdout
from io import StringIO
import json
import os
import subprocess
import tempfile
from unittest.mock import patch
import re
from pathlib import Path
import shlex
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("workflow_desktop_ui", ROOT / "scripts/test-desktop-ui.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class UiWorkflowContract(unittest.TestCase):
    # Focused checks deliberately require local models, external tools, native
    # input modes, or a separate stress/measurement/workspace-replacement workload. Every other leaf
    # belongs to the shared smoke session. This is policy, not an expansion of
    # selected_journeys() copied back into the assertion.
    FOCUSED_ONLY = {
        'liquid_selection', 'liquid_streaming', 'diagnostic_fix', 'diagnostic_fix_setup', 'ltex', 'settings_links',
        'tex_documentation', 'citation_import', 'latexdiff',
        'partial_zoom_stress', 'partial_cache_edges', 'partial_dividers',
        'rectangle', 'startup_preview', 'typing_particles', 'effects_pacing',
        'inactive_preview', 'workspace_switch', 'comment_controls', 'comment_controls_settings',
        'tex_magic', 'git_github', 'git_setup', 'overleaf_session',
    }

    def test_every_selectable_leaf_resolves_to_one_real_implementation(self):
        leaves = set(runner.JOURNEY_NAMES) - {'all', 'tex'}
        self.assertEqual(len(runner.JOURNEY_NAMES), len(set(runner.JOURNEY_NAMES)))
        for name in sorted(leaves):
            with self.subTest(journey=name):
                self.assertEqual(runner.selected_journeys(name), [name])
                method = getattr(runner.Journey, name, None)
                self.assertTrue(callable(method), 'registered but not executable')
                self.assertEqual(method.__name__, name)

    def test_aggregate_has_no_duplicates_aliases_or_hidden_external_dependencies(self):
        expanded = runner.selected_journeys('all')
        self.assertEqual(len(expanded), len(set(expanded)))
        expected = set(runner.JOURNEY_NAMES) - {'all', 'tex'} - self.FOCUSED_ONLY
        self.assertSetEqual(set(expanded), expected)
        self.assertLessEqual(self.FOCUSED_ONLY, set(runner.JOURNEY_NAMES))
        for group in (runner.LIQUID_JOURNEYS, runner.DIAGNOSTIC_FIX_JOURNEYS,
                      runner.DIAGNOSTIC_FIX_SETUP_JOURNEYS, runner.LTEX_JOURNEYS,
                      runner.SETTINGS_LINK_JOURNEYS, runner.LATEXDIFF_JOURNEYS,
                      runner.TEXDOC_JOURNEYS, runner.CITATION_JOURNEYS):
            self.assertLessEqual(set(group), self.FOCUSED_ONLY)

    def test_tex_alias_executes_exactly_the_registered_tex_group(self):
        self.assertEqual(runner.selected_journeys('tex'), list(runner.TEX_JOURNEYS))
        self.assertEqual(len(runner.TEX_JOURNEYS), len(set(runner.TEX_JOURNEYS)))
        self.assertLessEqual(set(runner.TEX_JOURNEYS), set(runner.selected_journeys('all')))

    def test_real_cli_prepares_exact_fixtures_before_the_native_launch_boundary(self):
        from desktop_ui_explorer_journeys import EXPLORER_FOLDER, EXPLORER_SEED, EXPLORER_NESTED

        class LaunchObserved(BaseException):
            # main catches Exception to retain failure evidence. A boundary
            # sentinel exits without pretending a native process ever existed.
            pass

        for name, suffix, source, explorer, only in (
                ('all', '.typ', runner.PREVIEW_SOURCE, True, None),
                ('all', '.typ', runner.PREVIEW_SOURCE, True, ['images', 'dialogs']),
                ('explorer_files', '.txt', 'Desktop journey fixture.\n', True, None),
                ('diagnostic_fix_setup', '.txt', 'Desktop journey fixture.\n', False, None),
                ('overleaf', '.typ', runner.PREVIEW_SOURCE, False, None),
                ('startup_preview', '.typ', runner.PREVIEW_SOURCE, False, None),
                ('tex', '.tex', runner.TEX_SOURCE, False, None)):
            with self.subTest(journey=name, only=only), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                executable = root / 'preserved-test-binary'
                executable.write_bytes(b'not executable app code')
                executable.chmod(0o700)
                launches = []

                def prepare_only(command, **kwargs):
                    if command[0] == 'swiftc' or command[0] == 'git':
                        return subprocess.CompletedProcess(command, 0)
                    if command[-1] == 'preflight':
                        return subprocess.CompletedProcess(command, 0, '{"desktop":true}')
                    self.fail('unexpected external preparation: ' + repr(command))

                def read_metadata(command, **kwargs):
                    if command[-1] == '--version':
                        return 'tiptoptyp fixture-build\n'
                    if command == ['git', 'rev-parse', 'HEAD']:
                        return '0' * 40 + '\n'
                    if command == ['git', 'diff', '--stat']:
                        return ''
                    self.fail('unexpected metadata command: ' + repr(command))

                def launch(command, **kwargs):
                    launches.append(command)
                    directory = Path(kwargs['cwd'])
                    if name == 'startup_preview':
                        self.assertEqual(len(command), 1, 'startup must open no implicit document')
                        fixture = directory / 'journey.typ'
                    else:
                        fixture = Path(command[1])
                    self.assertEqual(fixture.parent, directory)
                    self.assertEqual(fixture.name, 'journey' + suffix)
                    self.assertEqual(fixture.read_text(), source)
                    seed = directory / EXPLORER_FOLDER
                    self.assertEqual(seed.exists(), explorer)
                    if explorer:
                        self.assertEqual((seed / 'seed.txt').read_bytes(), EXPLORER_SEED)
                        self.assertEqual((seed / 'nested/payload.bin').read_bytes(), EXPLORER_NESTED)
                    self.assertEqual(kwargs['env']['TIPTOPTYP_DESKTOP_TEST_DIR'], str(directory))
                    self.assertNotEqual(directory, ROOT)
                    raise LaunchObserved()

                argv = ['test-desktop-ui.py', '--journey', name, '--executable', str(executable)]
                for selected in only or []:
                    argv.extend(['--only', selected])
                with patch.object(runner, 'ROOT', root), patch.object(runner.sys, 'argv', argv), \
                        patch.object(runner.sys, 'platform', 'darwin'), \
                        patch.object(runner.platform, 'platform', return_value='fixture platform'), \
                        patch.dict(os.environ, {'SHELL': '/bin/zsh'}), \
                        patch.object(runner.subprocess, 'run', side_effect=prepare_only), \
                        patch.object(runner.subprocess, 'check_output', side_effect=read_metadata), \
                        patch.object(runner.subprocess, 'Popen', side_effect=launch), \
                        redirect_stdout(StringIO()), self.assertRaises(LaunchObserved):
                    runner.main()
                self.assertEqual(len(launches), 1)
                receipts = list((root / '.tiptoptyp/desktop-ui-tests').glob('run-*/result.json'))
                self.assertEqual(len(receipts), 1)
                receipt = json.loads(receipts[0].read_text())
                self.assertFalse(receipt['passed'], 'preparation must never count as native evidence')
                self.assertEqual(receipt['journeys'], [])
                self.assertEqual(receipt['selection']['only'], only)
                self.assertEqual(receipt['selection']['planned'], runner.selected_journeys(name, only=only))

    def test_native_dropdown_exposes_every_registered_choice_exactly_once(self):
        workflow = (ROOT / ".github/workflows/desktop-ui.yml").read_text()
        match = re.search(r"options:\s*(\[[^\]]*\])", workflow, re.DOTALL)
        self.assertIsNotNone(match, "native journey options must remain an inspectable flow list")
        choices = [item.strip().strip("\"'") for item in match.group(1)[1:-1].split(",")]
        self.assertEqual(len(choices), len(set(choices)), "duplicate native workflow choices")
        self.assertEqual(len(runner.JOURNEY_NAMES), len(set(runner.JOURNEY_NAMES)))
        self.assertSetEqual(set(choices), set(runner.JOURNEY_NAMES))

    def test_independent_fast_lane_collects_all_script_tests(self):
        workflow = (ROOT / ".github/workflows/checks.yml").read_text()
        lane = re.search(r"^  fast:\n(.*?)(?=^  [a-z][a-z-]*:|\Z)",
                         workflow, re.MULTILINE | re.DOTALL)
        self.assertIsNotNone(lane, "script checks must not wait for the Rust build")
        self.assertNotIn('needs:', lane.group(1))
        match = re.search(r"- name: Local fast checks\n\s+run: ([^\n]+)", lane.group(1))
        self.assertIsNotNone(match, "CI must mirror the local runner")
        self.assertEqual(shlex.split(match.group(1)), ['sh', 'scripts/check-local.sh', 'fast'])
        local = (ROOT / 'scripts/check-local.sh').read_text()
        discover = re.findall(r'^ +python3 -m unittest discover [^\n]+', local, re.MULTILINE)
        self.assertEqual(len(discover), 1)
        command = shlex.split(discover[0])
        directory = ROOT / command[command.index('-s') + 1]
        pattern = command[command.index('-p') + 1]
        collected = {path.resolve() for path in directory.glob(pattern)}
        expected = {path.resolve() for path in (ROOT / "scripts").glob("test_*.py")}
        self.assertSetEqual(collected, expected,
                            "new script tests must not become orphaned in CI")

    def test_supplemental_features_share_one_macos_build_and_keep_their_owned_contracts(self):
        workflow = (ROOT / ".github/workflows/checks.yml").read_text()
        # The local runner's executable contracts check flags, full-suite and
        # feature coverage. CI owns no competing copy of those command lists.
        self.assertEqual(workflow.count('run: sh scripts/check-local.sh rust'), 1)
        self.assertEqual(workflow.count('run: sh scripts/check-local.sh features'), 1)
        self.assertNotIn('test --locked --features profiling --no-fail-fast', workflow)


if __name__ == "__main__":
    unittest.main()
