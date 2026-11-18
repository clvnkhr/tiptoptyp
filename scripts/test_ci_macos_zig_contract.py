"""Fail early when the pinned Zig build runner cannot link the macOS SDK."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/check-macos-zig.sh"


class MacosZigPreflight(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ttt zig sdk ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.scratch = self.root / "scratch space"
        self.scratch.mkdir()
        self.receipt = self.root / "zig.json"
        self.environment = os.environ | {
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "TMPDIR": str(self.scratch),
            "DEVELOPER_DIR": "/fake/Xcode 26.3/Contents/Developer",
            "TTT_ZIG_TEST_RECEIPT": str(self.receipt),
        }
        self.write_tool("sw_vers", 'print("macOS test fixture")')
        self.write_tool("xcode-select", 'import os; print(os.environ["DEVELOPER_DIR"])')
        self.write_tool("xcrun", """import os, sys
if os.environ.get("TTT_ZIG_TEST_SDK_FAILURE"):
    sys.exit(23)
print("/fake/MacOSX26.2.sdk" if sys.argv[-1] == "--show-sdk-path" else "26.2")
""")
        self.write_tool("zig", """import json, os
from pathlib import Path
import sys
if sys.argv[1:] == ["version"]:
    print("0.15.2")
    sys.exit(0)
directory = Path(sys.argv[sys.argv.index("--build-file") + 1]).parent
Path(os.environ["TTT_ZIG_TEST_RECEIPT"]).write_text(json.dumps({
    "arguments": sys.argv[1:],
    "directory": str(directory),
    "developer_dir": os.environ.get("DEVELOPER_DIR"),
    "build": (directory / "build.zig").read_text(),
    "main": (directory / "main.zig").read_text(),
}))
sys.exit(int(os.environ.get("TTT_ZIG_TEST_BUILD_FAILURE", "0")))
""")

    def write_tool(self, name, code):
        path = self.bin / name
        path.write_text("#!" + sys.executable + "\n" + code + "\n")
        path.chmod(0o755)

    def run_preflight(self):
        return subprocess.run(["sh", str(SCRIPT)], env=self.environment,
                              capture_output=True, text=True, check=False)

    def test_builds_and_runs_a_libc_linked_executable_with_uncached_paths(self):
        result = self.run_preflight()
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(self.receipt.read_text())
        directory = Path(receipt["directory"])
        self.assertEqual(receipt["arguments"], [
            "build", "--build-file", str(directory / "build.zig"),
            "--cache-dir", str(directory / "local-cache"),
            "--global-cache-dir", str(directory / "global-cache"), "--summary", "all",
        ])
        self.assertEqual(directory.parent.resolve(), self.scratch.resolve())
        self.assertFalse(directory.exists(), "temporary build/cache must be removed")
        self.assertEqual(receipt["developer_dir"], self.environment["DEVELOPER_DIR"])
        self.assertIn(".link_libc = true", receipt["build"])
        self.assertIn("addRunArtifact(executable)", receipt["build"])
        self.assertIn('extern "c" fn puts', receipt["main"])
        self.assertIn("/fake/MacOSX26.2.sdk", result.stdout)
        self.assertIn("0.15.2", result.stdout)

    def test_link_failure_is_fatal_and_cleans_only_its_temporary_build(self):
        preserved = self.scratch / "preserved"
        preserved.write_text("other build")
        self.environment["TTT_ZIG_TEST_BUILD_FAILURE"] = "19"
        result = self.run_preflight()
        self.assertEqual(result.returncode, 19)
        receipt = json.loads(self.receipt.read_text())
        self.assertFalse(Path(receipt["directory"]).exists())
        self.assertEqual(preserved.read_text(), "other build")

    def test_missing_sdk_fails_before_building(self):
        self.environment["TTT_ZIG_TEST_SDK_FAILURE"] = "1"
        result = self.run_preflight()
        self.assertEqual(result.returncode, 23)
        self.assertFalse(self.receipt.exists())
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_coverage_retains_output_without_hiding_native_or_coverage_failure(self):
        workflow = (ROOT / ".github/workflows/checks.yml").read_text()
        match = re.search(
            r"- name: Run real-tool integration tests and measure coverage\n"
            r"        shell: bash\n        run: \|\n((?:          [^\n]*\n)+)", workflow)
        self.assertIsNotNone(match)
        script = "\n".join(line[10:] for line in match.group(1).splitlines())
        project = self.root / "project"
        sidecars = project / "toolchain/bin"
        sidecars.mkdir(parents=True)
        for name in ("typst", "tinymist"):
            path = sidecars / (name + "-aarch64-apple-darwin")
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        self.write_tool("rustc", 'print("host: aarch64-apple-darwin")')
        self.write_tool("cargo", 'import os, sys; print("native context output", file=sys.stderr); '
                        'sys.exit(int(os.environ.get("TTT_NATIVE_STATUS", "0")))')
        self.write_tool("python3", 'import sys; print("coverage output", file=sys.stderr); sys.exit(31)')
        for native_status, expected, expected_coverage in [(29, 29, False), (0, 31, True)]:
            with self.subTest(native_status=native_status):
                environment = self.environment | {"TTT_NATIVE_STATUS": str(native_status)}
                result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", script],
                                        cwd=project, env=environment, capture_output=True,
                                        text=True, check=False)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                logs = project / ".tiptoptyp/coverage/latest"
                self.assertIn("native context output", (logs / "native-context.log").read_text())
                self.assertEqual((logs / "coverage-run.log").exists(), expected_coverage)
                if expected_coverage:
                    self.assertIn("coverage output", (logs / "coverage-run.log").read_text())

    def test_every_hosted_macos_job_selects_sdk_before_preflight_and_cargo(self):
        workflow = (ROOT / ".github/workflows/checks.yml").read_text()
        starts = list(re.finditer(r"^  ([a-z][a-z-]*):\n", workflow, re.MULTILINE))
        jobs = {match.group(1): workflow[match.end():
                starts[index + 1].start() if index + 1 < len(starts) else len(workflow)]
                for index, match in enumerate(starts)}
        for name in ("rust", "real-tools", "native-windows"):
            with self.subTest(job=name):
                job = jobs[name]
                selection = job.index("- name: Select Xcode 26.3 for Zig")
                zig = job.index("mlugg/setup-zig@v2")
                preflight = job.index("run: sh scripts/check-macos-zig.sh")
                cargo = re.search(r"^ +run: (?:\|\n +)?cargo|^ +cargo ", job,
                                  re.MULTILINE)
                self.assertIsNotNone(cargo)
                self.assertLess(selection, zig)
                self.assertLess(zig, preflight)
                self.assertLess(preflight, cargo.start())
                selection_step = job[selection:zig]
                self.assertIn("if: runner.os == 'macOS'", selection_step)
                self.assertIn("/Applications/Xcode_26.3.app/Contents/Developer", selection_step)
                self.assertIn('"DEVELOPER_DIR=$developer_dir" >> "$GITHUB_ENV"', selection_step)
                preflight_step = job[job.rfind("- name:", 0, preflight):preflight]
                self.assertIn("if: runner.os == 'macOS'", preflight_step)
                self.assertNotIn("sudo xcode-select", job)


if __name__ == "__main__":
    unittest.main()
