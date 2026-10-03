"""Failure-path regressions for the native startup measurement runner."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import sys
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location("measure_startup", Path(__file__).with_name("measure-startup.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class StartupRunnerTests(unittest.TestCase):
    def exercise_failure(self, sampler=None, spawn_error=None, quit_error=None):
        process = Mock(pid=123)
        process.poll.return_value = None
        spawned = [process, spawn_error or sampler] if sampler is not None or spawn_error else [process]
        with tempfile.TemporaryDirectory() as evidence, \
                patch.object(runner.subprocess, "Popen", side_effect=spawned), \
                patch.object(runner.subprocess, "run", side_effect=quit_error) as quit_app, \
                patch.object(runner.time, "monotonic", side_effect=[0, 21]):
            with self.assertRaises((RuntimeError, OSError, subprocess.TimeoutExpired)):
                runner.measure(Path("/tmp/fake-tiptoptyp"), "empty", Path(evidence), "failure", sample=len(spawned) == 2)
        return process, quit_app

    def test_unresponsive_app_is_killed_after_bounded_waits(self):
        process = Mock(pid=123)
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("app", 8),
                                    subprocess.TimeoutExpired("app", 6), 0]
        with patch.object(runner.subprocess, "run"):
            runner.close_app(process)
        process.terminate.assert_called_once()
        process.kill.assert_called_once()
        self.assertEqual(process.wait.call_count, 3)

    def test_exited_app_does_not_receive_native_keys(self):
        process = Mock()
        process.poll.return_value = 0
        with patch.object(runner.subprocess, "run") as quit_app:
            runner.close_app(process)
        quit_app.assert_not_called()

    def test_driver_build_creates_output_directory_on_fresh_checkout(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(runner, "ROOT", Path(directory)), \
                patch.object(runner.subprocess, "run") as compile_driver:
            driver = runner.prepare_driver()
            self.assertTrue(driver.parent.is_dir())
            self.assertEqual(compile_driver.call_args.args[0][-1], str(driver))

    def test_completed_measurements_are_saved_before_next_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            result = {"binaries": [{"runs": [{"milliseconds": 100}]}]}
            runner.save_measurements(output, result)
            self.assertEqual(runner.json.loads((output / "measurement.json").read_text()), result)
            self.assertFalse((output / "measurement.json.tmp").exists())

    def test_later_failure_preserves_results_and_launch_order_is_counterbalanced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "Cargo.lock").write_text("fixture")
            before, after = root / "before", root / "after"
            before.write_bytes(b"before")
            after.write_bytes(b"after")
            output = root / "results"
            observation = {"milliseconds": 100}
            with patch.object(runner, "ROOT", root), \
                    patch.object(sys, "argv", ["measure-startup.py", "--binary", str(before),
                                               "--compare", str(after), "--runs", "1", "--output", str(output)]), \
                    patch.object(runner.platform, "system", return_value="Darwin"), \
                    patch.object(runner.platform, "platform", return_value="macOS fixture"), \
                    patch.object(runner, "prepare_driver", return_value=root / "driver"), \
                    patch.object(runner.subprocess, "check_output", side_effect=["{}", "rustc fixture", "revision", "version", "version"]), \
                    patch.object(runner, "measure", side_effect=[observation, observation, observation, RuntimeError("launch failed")]) as measure, \
                    patch("builtins.print"):
                with self.assertRaisesRegex(RuntimeError, "launch failed"):
                    runner.main()
            self.assertEqual([call.args[0] for call in measure.call_args_list], [before, after, after, before])
            result = runner.json.loads((output / "measurement.json").read_text())
            self.assertEqual([len(binary["runs"]) for binary in result["binaries"]], [1, 2])
            self.assertNotIn("median_ms_excluding_first", result["binaries"][0])

    def test_sampler_spawn_failure_still_closes_app(self):
        process, quit_app = self.exercise_failure(spawn_error=OSError("sampler unavailable"))
        quit_app.assert_called_once()
        process.wait.assert_called()

    def test_sampler_timeout_still_closes_app(self):
        sampler = Mock()
        sampler.wait.side_effect = [subprocess.TimeoutExpired("sample", 8), 0]
        process, quit_app = self.exercise_failure(sampler=sampler)
        quit_app.assert_called_once()
        sampler.kill.assert_called_once()
        process.wait.assert_called()

    def test_input_driver_failure_still_terminates_app(self):
        process, _ = self.exercise_failure(quit_error=OSError("driver unavailable"))
        process.terminate.assert_called_once()
        process.wait.assert_called()


if __name__ == "__main__":
    unittest.main()
