"""Pure native-zoom runner admission checks; no desktop or sidecars are launched."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "preview_zoom_runner", Path(__file__).with_name("test-preview-zoom-native.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class NativeZoomRunnerContract(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ttt-zoom-runner-contract-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.enterContext(patch.object(runner, "ROOT", self.root))
        self.enterContext(patch.object(runner.sys, "platform", "darwin"))
        self.enterContext(patch.dict(os.environ, {}, clear=True))

    def executable(self, relative):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fake executable; never launched")
        path.chmod(0o700)
        return path

    def sidecar_at_launch(self):
        fixture = self.root / "fixture.typ"
        fixture.write_text("hello")
        with patch.object(runner.subprocess, "Popen", side_effect=RuntimeError(
                "intentional launch boundary")) as launch:
            with self.assertRaisesRegex(RuntimeError, "intentional launch boundary"):
                runner.run(self.root / "native-test", "full", self.root / "output", fixture=fixture)
        return Path(launch.call_args.args[0][0])

    def test_arm_and_intel_select_their_matching_pinned_sidecar(self):
        for host, target in (("arm64", "aarch64"), ("x86_64", "x86_64")):
            expected = self.executable(f"toolchain/bin/tinymist-{target}-apple-darwin")
            with self.subTest(host=host), patch.object(runner.platform, "machine", return_value=host):
                self.assertEqual(self.sidecar_at_launch(), expected)

    def test_explicit_sidecar_override_is_honored_and_expands_home(self):
        expected = self.executable("custom helper/tinymist")
        with patch.object(runner.platform, "machine", return_value="x86_64"), patch.dict(
                os.environ, {"HOME": str(self.root),
                             "TIPTOPTYP_TEST_TINYMIST": "~/custom helper/tinymist"}):
            self.assertEqual(self.sidecar_at_launch(), expected)

    def test_unsupported_hosts_fail_before_any_sidecar_launch_or_output(self):
        fixture = self.root / "fixture.typ"
        fixture.write_text("hello")
        output = self.root / "output"
        with patch.object(runner.sys, "platform", "linux"), patch.object(
                runner.subprocess, "Popen") as launch:
            with self.assertRaisesRegex(RuntimeError, "requires macOS"):
                runner.run(self.root / "native-test", "full", output, fixture=fixture)
        launch.assert_not_called()
        self.assertFalse(output.exists())

    def test_unsupported_macos_architecture_fails_clearly(self):
        with patch.object(runner.platform, "machine", return_value="unsupported"):
            with self.assertRaisesRegex(RuntimeError, "Unsupported macOS architecture"):
                runner.resolve_tinymist()

    def test_missing_override_never_falls_back_to_another_sidecar(self):
        self.executable("toolchain/bin/tinymist-aarch64-apple-darwin")
        with patch.dict(os.environ, {"TIPTOPTYP_TEST_TINYMIST": str(self.root / "missing")}):
            with self.assertRaisesRegex(FileNotFoundError, "Missing Tinymist sidecar"):
                runner.resolve_tinymist()

    def test_nonexecutable_sidecar_is_rejected_before_launch(self):
        program = self.executable("toolchain/bin/tinymist-aarch64-apple-darwin")
        program.chmod(0o600)
        with patch.object(runner.platform, "machine", return_value="arm64"):
            with self.assertRaisesRegex(PermissionError, "not executable"):
                runner.resolve_tinymist()


if __name__ == "__main__":
    unittest.main()
