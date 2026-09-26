"""Fast checks for the desktop runner's failure detectors; no desktop claimed."""
import importlib.util
import math
from pathlib import Path
import unittest
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("desktop_ui", Path(__file__).with_name("test-desktop-ui.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


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


if __name__ == "__main__":
    unittest.main()
