"""Fast checks for the desktop runner's failure detectors; no desktop claimed."""
import importlib.util
import math
from pathlib import Path
import unittest
from unittest.mock import patch
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("desktop_ui", Path(__file__).with_name("test-desktop-ui.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class FixtureFingerprintContract(unittest.TestCase):
    def test_matches_rust_probe_vectors_without_logging_source(self):
        from desktop_ui_journeys import fingerprint
        self.assertEqual(fingerprint(""), "cbf29ce484222325")
        self.assertEqual(fingerprint("hello"), "a430d84680aabd0b")
        self.assertEqual(fingerprint("é🙂z"), "3a046d85bff8a56f")


class ClickReadinessContract(unittest.TestCase):
    def test_settles_moving_geometry_before_one_click(self):
        journey = object.__new__(runner.Journey)
        samples = iter([10, 11, 11])
        journey.snapshot = lambda: {"document": {}, "targets": {"button": {
            "x": 20, "y": next(samples), "enabled": True, "age_ms": 0}}}
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.record = lambda *args, **kwargs: None
        journey.click("button")
        self.assertEqual(actions, [("move", 20, 10), ("move", 20, 11), ("click", 20, 11)])

    def test_unsettled_target_times_out_without_clicking(self):
        journey = object.__new__(runner.Journey)
        samples = iter([10, 11])
        journey.snapshot = lambda: {"document": {}, "targets": {"button": {
            "x": 20, "y": next(samples), "enabled": True, "age_ms": 0}}}
        actions = []
        journey.native = lambda *args: actions.append(args)
        with patch.object(runner.time, "monotonic", side_effect=[0, 4]):
            with self.assertRaisesRegex(AssertionError, "did not settle"):
                journey.click("button")
        self.assertEqual(actions, [("move", 20, 10)])

    def test_disappearing_target_never_receives_a_click(self):
        journey = object.__new__(runner.Journey)
        samples = iter([{"button": {"x": 20, "y": 10, "enabled": True, "age_ms": 0}}, {}])
        journey.snapshot = lambda: {"document": {}, "targets": next(samples)}
        actions = []
        journey.native = lambda *args: actions.append(args)
        with self.assertRaisesRegex(AssertionError, "missing"):
            journey.click("button")
        self.assertEqual(actions, [("move", 20, 10)])


class HeightContract(unittest.TestCase):
    def test_accepts_constant_height_when_window_moves(self):
        for top in [0, 100, -200]:
            runner.assert_stable_height(220, {"panel_rect": [0, top, 800, top + 220]})

    def test_rejects_the_observed_three_point_per_frame_regression(self):
        for frame in range(1, 31):
            with self.assertRaisesRegex(AssertionError, "panel drift"):
                runner.assert_stable_height(232, {"panel_rect": [0, 0, 800, 232 + 3 * frame]})

    def test_missing_or_nonfinite_geometry_cannot_pass(self):
        for rectangle in [None, [0, 0, 800, math.nan], [0, 0, 800, math.inf]]:
            with self.assertRaises(AssertionError):
                runner.assert_stable_height(220, {"panel_rect": rectangle})


class PreviewContract(unittest.TestCase):
    def observation(self):
        return {"age_ms": 0, "value": {"ready": True, "filters": ["url(#tiptoptyp-palette)"],
                "background": "rgb(0, 0, 0)", "slopes": [-1, -1, -1], "intercepts": [1, 1, 1]}}

    def test_single_dark_transform(self):
        runner.assert_tinymist_palette(self.observation(), [[0, 0, 0, 255], [255, 255, 255, 255]])

    def test_double_inversion_cannot_pass(self):
        state = self.observation()
        state["value"]["filters"].append("invert(1)")
        with self.assertRaisesRegex(AssertionError, "exactly one"):
            runner.assert_tinymist_palette(state, [[0, 0, 0, 255], [255, 255, 255, 255]])

    def test_stale_or_wrong_palette_cannot_pass(self):
        state = self.observation()
        state["age_ms"] = 501
        with self.assertRaisesRegex(AssertionError, "stale"):
            runner.assert_tinymist_palette(state, [[0, 0, 0, 255], [255, 255, 255, 255]])
        with self.assertRaisesRegex(AssertionError, "paper"):
            runner.assert_tinymist_palette(self.observation(), [[255, 255, 255, 255], [0, 0, 0, 255]])


if __name__ == "__main__":
    unittest.main()
