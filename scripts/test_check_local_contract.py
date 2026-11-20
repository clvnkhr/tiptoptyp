"""Execute private runner copies with fake tools; no recursive suite or builds."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LocalCheckRunnerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='local checks ')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repository = self.root / 'repository with spaces'
        scripts = self.repository / 'scripts'
        scripts.mkdir(parents=True)
        self.runner = scripts / 'check-local.sh'
        shutil.copy2(ROOT / 'scripts/check-local.sh', self.runner)
        commands = self.root / 'commands'
        commands.mkdir()
        self.log = self.root / 'commands.jsonl'
        stub = '''import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
if name == 'uname':
    print(os.environ.get('CHECK_TEST_HOST', 'Darwin'))
else:
    with open(os.environ['CHECK_TEST_LOG'], 'a') as log:
        log.write(json.dumps({'tool': name, 'args': sys.argv[1:], 'cwd': os.getcwd()}) + '\\n')
    if os.environ.get('CHECK_TEST_FAIL') == name + ':' + sys.argv[1]:
        sys.exit(23)
'''
        for name in ('python3', 'node', 'cargo', 'uname'):
            executable = commands / name
            executable.write_text('#!' + sys.executable + '\n' + stub)
            executable.chmod(0o755)
        self.environment = os.environ | {
            'PATH': str(commands) + os.pathsep + '/usr/bin:/bin',
            'CHECK_TEST_LOG': str(self.log),
        }

    def run_checks(self, *arguments, **environment):
        result = subprocess.run(['sh', str(self.runner), *arguments], cwd=self.root,
                                env=self.environment | environment, text=True,
                                capture_output=True, timeout=10)
        commands = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return result, commands

    def test_fast_is_default_runs_complete_script_and_browser_lanes_without_cargo(self):
        result, commands = self.run_checks()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([command['tool'] for command in commands], ['python3', 'node', 'node', 'python3'])
        self.assertEqual(commands[0]['args'], ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_*.py'])
        self.assertEqual(commands[1]['args'], ['--test', 'scripts/test_overleaf_bridge.cjs',
                         'scripts/test-preview-navigation.mjs', 'scripts/test-preview-snapshot-buffer.mjs',
                         'scripts/test-preview-zoom-snapshot.mjs', 'scripts/test-desktop-preview-probe.mjs',
                         'scripts/test-preview-native-probe.mjs'])
        self.assertEqual(commands[2]['args'], ['scripts/test-preview-e2e.cjs', '--large', '--self-test'])
        self.assertEqual(commands[3]['args'], ['scripts/test-preview-partial-native.py', '--self-test'])
        self.assertTrue(all(Path(command['cwd']) == self.repository.resolve() for command in commands))

    def test_rust_runs_required_checks_in_order_without_models_or_gui(self):
        result, commands = self.run_checks('rust')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all(command['tool'] == 'cargo' for command in commands))
        self.assertEqual([command['args'] for command in commands], [
            ['fmt', '--all', '--', '--check'],
            ['clippy', '--locked', '--all-targets', '--', '-D', 'warnings'],
            ['test', '--locked', '--no-fail-fast'],
            ['test', '--locked', '--manifest-path', 'xtask/Cargo.toml'],
        ])

    def test_feature_contracts_compile_all_targets_and_run_both_macos_owners(self):
        result, commands = self.run_checks('features')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([command['args'] for command in commands], [
            ['fmt', '--manifest-path', 'xtask/Cargo.toml', '--', '--check'],
            ['clippy', '--locked', '--all-targets', '--features', 'profiling,desktop-ui-tests', '--', '-D', 'warnings'],
            ['test', '--locked', '--features', 'profiling,desktop-ui-tests', '--bin', 'tiptoptyp', '--', 'performance::', 'desktop_test::tests'],
        ])

    def test_linux_features_use_profiling_without_mac_only_inspection(self):
        result, commands = self.run_checks('features', CHECK_TEST_HOST='Linux')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands[-1]['args'], ['test', '--locked', '--features', 'profiling', '--bin', 'tiptoptyp', 'performance::'])
        self.assertNotIn('desktop-ui-tests', str(commands))

    def test_all_runs_each_local_lane_once(self):
        result, commands = self.run_checks('all')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(commands), 11)
        self.assertEqual([command['args'][0] for command in commands[4:]],
                         ['fmt', 'clippy', 'test', 'test', 'fmt', 'clippy', 'test'])

    def test_failures_stop_later_lanes_and_preserve_the_error(self):
        result, commands = self.run_checks('all', CHECK_TEST_FAIL='cargo:clippy')
        self.assertEqual(result.returncode, 23, result.stderr)
        self.assertEqual(len(commands), 6)
        self.assertFalse(any(command['args'][0] == 'test' for command in commands))

    def test_help_and_invalid_modes_never_run_tools(self):
        for arguments, expected in [(('--help',), 0), (('',), 64), (('unknown',), 64), (('fast', 'extra'), 64)]:
            with self.subTest(arguments=arguments):
                result, commands = self.run_checks(*arguments)
                self.assertEqual(result.returncode, expected)
                self.assertEqual(commands, [])


if __name__ == '__main__':
    unittest.main()
