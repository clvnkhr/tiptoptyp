#!/usr/bin/env python3
"""Artifact/orchestration tests only: the compiler and timings are synthetic."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import bench_tex_table_lookup as benchmark


FEATURES = """#[derive(Debug)]
pub(crate) struct SourceEdit {
    source: String,
}
fn char_to_byte() {}
struct LineMap;
"""
GUIDES = """#[derive(Debug)]
pub(crate) struct TexInlineLiteral {
    at: usize,
}
fn tex_blocks() {}
"""
FAKE_PROBE = """import json
from pathlib import Path
import sys

cases = CASES
output = Path(sys.argv[1])
output.mkdir()
strategy = 'uncached' if Path(sys.argv[0]).name.startswith('before') else 'indexed'
mode = (Path(__file__).parent / 'mode').read_text()
for case in cases:
    source = 'α fixture\\n'.encode()
    (output / (case + '.tex')).write_bytes(source)
    (output / (case + '.carets.json')).write_text('[[0,0],[1,2]]\\n')
    samples = [9, 1, 8, 2, 7, 3, 6, 4, 5]
    row = dict(case=case, strategy=strategy, source_bytes=len(source), source_chars=10,
               carets_per_batch=2, golden_checks=2, warm_median_ns=5, warm_p90_ns=9,
               cold_index_and_first_target_median_ns=5, cold_p90_ns=9,
               warm_samples_ns_per_query=samples, cold_samples_ns=samples)
    if mode == 'bad-summary':
        row['warm_median_ns'] = 1
    print(json.dumps(row))
    if mode == 'incomplete':
        break
    if mode == 'failed':
        sys.exit(1)
""".replace("CASES", repr(benchmark.CASES))


@unittest.skipUnless(os.name == "posix", "fake executable driver requires POSIX")
class BenchmarkArtifactTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="tex-lookup-contract-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)
        self.repo = self.base / "repo"
        self.repo.mkdir()
        self.output = self.base / "results"
        files = {
            "src/editor_features.rs": FEATURES,
            "src/editor_math_guides.rs": GUIDES,
            "src/editor_features/tex_table.rs": "// synthetic capture only\n",
            "src/editor_data.rs": "// synthetic owner capture only\n",
            "rust-toolchain.toml": '[toolchain]\nchannel = "1.98.1"\n',
        }
        for name, text in files.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=Benchmark Test", "-c", "user.email=benchmark@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "-qm", "synthetic sources")
        self.compiler = self.base / "fake-rustc"
        self.compiler.write_text(
            f"#!{sys.executable}\n"
            "from pathlib import Path\nimport sys\n"
            "if sys.argv[1:] == ['-vV']:\n"
            "    print('rustc fake\\nrelease: 1.98.1')\n"
            "else:\n"
            "    output = Path(sys.argv[sys.argv.index('-o') + 1])\n"
            f"    output.write_text({('#!' + sys.executable + chr(10) + FAKE_PROBE)!r})\n"
            "    output.chmod(0o700)\n"
        )
        self.compiler.chmod(0o700)

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    def run_tool(self, mode="ok", extra=()):
        # The fake executable reads this from its output directory. The tool
        # must create that directory itself, so the compiler prepares the mode.
        compiler_text = self.compiler.read_text()
        self.compiler.write_text(compiler_text + f"\nPath(sys.argv[sys.argv.index('-o') + 1]).parent.joinpath('mode').write_text({mode!r}) if '-o' in sys.argv else None\n")
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = benchmark.main([
                "--repo", str(self.repo), "--before-ref", "HEAD", "--rustc", str(self.compiler),
                "--output", str(self.output), *extra,
            ])
        return result, stdout.getvalue(), stderr.getvalue()

    def test_existing_artifacts_are_refused_before_compiler_or_git(self):
        self.output.mkdir()
        sentinel = self.output / "report.json"
        sentinel.write_text("retain exactly")
        self.compiler.unlink()
        result, _, error = self.run_tool_without_compiler()
        self.assertEqual(result, 1)
        self.assertIn("refusing to overwrite", error)
        self.assertEqual(sentinel.read_text(), "retain exactly")
        self.assertEqual(sorted(path.name for path in self.output.iterdir()), ["report.json"])

    def run_tool_without_compiler(self):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = benchmark.main([
                "--repo", str(self.repo), "--rustc", str(self.compiler), "--output", str(self.output),
            ])
        return result, "", stderr.getvalue()

    def test_dangling_output_symlink_is_not_followed_or_replaced(self):
        target = self.base / "must-not-create"
        self.output.symlink_to(target, target_is_directory=True)
        result, _, error = self.run_tool_without_compiler()
        self.assertEqual(result, 1)
        self.assertIn("refusing to overwrite", error)
        self.assertTrue(self.output.is_symlink())
        self.assertFalse(target.exists())

    def test_counterbalanced_capture_retains_all_samples_and_provenance(self):
        result, _, error = self.run_tool()
        self.assertEqual(result, 0, error)
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(report["status"], "complete")
        self.assertTrue(report["complete"])
        self.assertEqual([(run["round"], run["label"]) for run in report["runs"]],
                         [(1, "before"), (1, "after"), (2, "after"), (2, "before"), (3, "before"), (3, "after")])
        self.assertEqual(report["compiler"]["pinned_channel"], "1.98.1")
        for label in ("before", "after"):
            for name, digest in report[label]["source_sha256"].items():
                captured = self.output / label / "sources" / name
                self.assertEqual(benchmark.digest(captured.read_bytes()), digest)
                self.assertEqual(captured.read_bytes(), (self.repo / name).read_bytes())
            binary = report["binaries"][label]
            self.assertEqual(binary["executable_sha256"], benchmark.digest(Path(binary["executable"]).read_bytes()))
            self.assertIn("opt-level=3", binary["command"])
        for run in report["runs"]:
            self.assertEqual(run["rows"][0]["warm_samples_ns_per_query"], [9, 1, 8, 2, 7, 3, 6, 4, 5])
            for fixture in run["fixtures"].values():
                self.assertEqual(fixture["source_sha256"], benchmark.digest(Path(fixture["source_path"]).read_bytes()))
                self.assertEqual(fixture["carets_sha256"], benchmark.digest(Path(fixture["carets_path"]).read_bytes()))
        self.assertEqual(report["summary"][benchmark.CASES[0]]["after"]["warm_median_ns"], 5)

    def test_failed_or_incomplete_runs_retain_artifacts_without_success(self):
        for mode in ("failed", "incomplete", "bad-summary"):
            with self.subTest(mode=mode):
                self.output = self.base / ("results-" + mode)
                result, _, error = self.run_tool(mode)
                self.assertEqual(result, 1, error)
                report = json.loads((self.output / "report.json").read_text())
                self.assertEqual(report["status"], "failed")
                self.assertFalse(report["complete"])
                self.assertNotIn("summary", report)
                self.assertTrue((self.output / "runs/round-01-before.jsonl").exists())
                self.assertTrue((self.output / "before/sources/src/editor_data.rs").exists())

    def test_ref_is_an_argument_and_invalid_refs_cannot_become_commands(self):
        sentinel = self.base / "injected"
        result, _, error = self.run_tool(extra=("--before-ref", f"HEAD; touch {sentinel}"))
        self.assertEqual(result, 1, error)
        self.assertFalse(sentinel.exists())
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertNotIn("binaries", report)


if __name__ == "__main__":
    unittest.main()
