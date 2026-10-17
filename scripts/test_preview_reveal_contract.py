"""Failure-detector tests for reveal observations; no native interaction claimed."""
import copy
import importlib.util
import itertools
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
from desktop_ui_jump_journeys import assert_reveal, reveal_retired


class PreviewRevealContract(unittest.TestCase):
    def visible(self, elapsed=300):
        return {"active": True, "pending": False, "page": 2, "center": [200, 150],
                "viewport": [20, 40, 420, 600], "elapsed_ms": elapsed}

    def test_accepts_actual_tinymist_and_source_pdfium_observations(self):
        self.assertIs(assert_reveal(value := self.visible(), page=2), value)
        value.update(target=[199, 141, 201, 159], char_index=25)
        self.assertIs(assert_reveal(value, region=(4, 20, 40)), value)
        value["target"] = [200, 141, 200, 159]  # Actual text caret may have zero width.
        assert_reveal(value, region=(4, 20, 40))

    def test_rejects_pending_missing_expired_or_wrong_destination(self):
        for value in (None, {}, {**self.visible(), "pending": True},
                      {**self.visible(), "active": False},
                      {**self.visible(), "elapsed_ms": 900}):
            with self.subTest(value=value), self.assertRaises(AssertionError):
                assert_reveal(value)
        with self.assertRaisesRegex(AssertionError, "another paper"):
            assert_reveal(self.visible(), page=0)
        for at in (None, True, 19, 40):
            with self.subTest(at=at), self.assertRaisesRegex(AssertionError, "source location"):
                assert_reveal({**self.visible(), "char_index": at}, region=(4, 20, 40))

    def test_rejects_offscreen_nonfinite_or_miscentered_feedback(self):
        mutations = [
            {"center": [0, 0]}, {"center": [float("nan"), 150]},
            {"viewport": [20, 40, 20, 600]}, {"elapsed_ms": True},
            {"target": [200, 100, 220, 120]}, {"target": [200, 150, 201, 149]},
            {"target": [False, 141, 201, 159]},
        ]
        for patch in mutations:
            with self.subTest(patch=patch), self.assertRaises(AssertionError):
                assert_reveal({**self.visible(), **copy.deepcopy(patch)})

    def test_default_duration_is_strict_and_custom_options_keep_their_full_lifetime(self):
        assert_reveal(self.visible(899))
        with self.assertRaises(AssertionError):
            assert_reveal(self.visible(900))
        assert_reveal(self.visible(1499), duration_ms=1500)
        with self.assertRaises(AssertionError):
            assert_reveal(self.visible(1500), duration_ms=1500)
        for duration in (True, 0, 249, 2001, float("inf"), float("nan")):
            with self.subTest(duration=duration), self.assertRaises(AssertionError):
                assert_reveal(self.visible(), duration_ms=duration)

    def test_retirement_rejects_pending_and_active_feedback(self):
        self.assertTrue(reveal_retired(None))
        self.assertTrue(reveal_retired({"active": False, "pending": False}))
        self.assertFalse(reveal_retired(self.visible()))
        self.assertFalse(reveal_retired({"active": False, "pending": True}))
        self.assertFalse(reveal_retired({"active": True, "pending": True}))
        self.assertFalse(reveal_retired({"active": 0}))
        self.assertFalse(reveal_retired({}))


class RevealNumericInputReadiness(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location(
            "preview_reveal_desktop_ui", Path(__file__).with_name("test-desktop-ui.py"))
        cls.runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.runner)

    def journey(self, geometry):
        journey = object.__new__(self.runner.Journey)
        samples = iter(geometry)
        frames = itertools.count(1)
        def snapshot():
            target = {"x": 808, "y": 231, "width": 57, "height": 21.5,
                      "age_ms": 0, "enabled": True, "hovered": True,
                      "frame": next(frames), **next(samples)}
            return {"targets": {"duration": target}}
        journey.snapshot = snapshot
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.record = lambda *args, **kwargs: None
        return journey, actions

    def test_animated_search_scroll_settles_before_one_double_click(self):
        # The failure clicked at y270 while the source row moved to y231;
        # the preview row then received the text intended for the source.
        geometry = [{"y": y} for y in (425, 360, 270, 231, 231, 231, 231, 231, 231)]
        journey, actions = self.journey(geometry)
        with patch.object(self.runner.time, "sleep"), patch.object(
                self.runner.time, "monotonic", side_effect=itertools.count(0, 0.06)):
            journey.double_click("duration", minimum_height=18)
        self.assertEqual(actions, [("move", 808, 425), ("move", 808, 360),
                                   ("move", 808, 270), ("move", 808, 231),
                                   ("double-click", 808, 231)])

    def test_clipped_or_unhovered_rows_never_receive_a_double_click(self):
        for change in ({"height": 7.5}, {"hovered": False}):
            journey, actions = self.journey([change] * 8)
            with self.subTest(change=change), patch.object(self.runner.time, "sleep"), patch.object(
                    self.runner.time, "monotonic", side_effect=itertools.count(0, 1)):
                with self.assertRaisesRegex(AssertionError, "did not settle"):
                    journey.double_click("duration", minimum_height=18)
            self.assertFalse(any(action[0] == "double-click" for action in actions))

    def test_slow_drift_is_not_mistaken_for_stable_geometry(self):
        journey, actions = self.journey([{"y": 231 + 0.4 * index} for index in range(12)])
        with patch.object(self.runner.time, "sleep"), patch.object(
                self.runner.time, "monotonic", side_effect=itertools.count(0, 0.6)):
            with self.assertRaisesRegex(AssertionError, "did not settle"):
                journey.double_click("duration", minimum_height=18)
        self.assertFalse(any(action[0] == "double-click" for action in actions))


if __name__ == "__main__":
    unittest.main()
