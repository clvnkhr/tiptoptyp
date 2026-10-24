"""Failure-detector tests for reveal observations; no native interaction claimed."""
import copy
import importlib.util
import itertools
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
from desktop_ui_jump_journeys import (
    assert_automatic_pdf_follow, assert_pdfium_scroll_preserved, assert_reveal, reveal_retired,
)


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


class TexAutomaticPreviewContract(unittest.TestCase):
    def document(self):
        return {
            "frame": 40, "preview_backend": "PDFium", "preview_page": 1,
            "preview_pdfium_ready": True, "compile_indicator_spinning": False,
            "preview_reveal": {"active": False, "pending": False},
            "pdfium_render": {
                "requested_scale": 200, "desired_scale": 200, "active_scale": 200,
                "resident_count": 1, "requests": 3, "admissions": 3, "settling": False,
                "revision": 2, "active_revision": 2,
                "source_position": {"revision": 2, "page": 1, "point": [200, 400]},
                "page_geometry": {"page": 1, "rect": [20, -100, 400, 1500],
                                  "interact": [20, 40, 400, 560], "zoom": 1, "frame": 40},
            },
        }

    def baseline(self):
        document = self.document()
        render = document["pdfium_render"]
        render["revision"] = render["active_revision"] = 1
        render["source_position"] = None
        return render

    def test_scroll_regression_is_a_standalone_tex_journey(self):
        from desktop_ui_journeys import EditorJourneys
        from desktop_ui_tex_journeys import TEX_JOURNEYS
        self.assertEqual(TEX_JOURNEYS.count("tex_preview_scroll"), 1)
        self.assertTrue(callable(EditorJourneys.tex_preview_scroll))

    def test_accepts_new_pdf_with_visible_silent_follow(self):
        result = assert_automatic_pdf_follow(self.document(), 1, 1)
        self.assertEqual(result["center"], [220, 300])
        self.assertEqual(result["top_inset"], 260)

    def test_detects_original_bug_baseline_exactly_at_upper_clip(self):
        document = self.document()
        document["pdfium_render"]["page_geometry"]["rect"][1] = -360
        with self.assertRaisesRegex(AssertionError, "baseline is clipped"):
            assert_automatic_pdf_follow(document, 1, 1)

    def test_old_or_unadmitted_pdf_cannot_satisfy_edit_follow(self):
        for field, value in (("revision", 1), ("active_revision", 1), ("revision", True)):
            document = self.document()
            document["pdfium_render"][field] = value
            with self.subTest(field=field, value=value), self.assertRaisesRegex(
                    AssertionError, "newly compiled PDF"):
                assert_automatic_pdf_follow(document, 1, 1)
        for field, value in (("preview_pdfium_ready", False), ("compile_indicator_spinning", True)):
            document = self.document()
            document[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "newly compiled PDF"):
                assert_automatic_pdf_follow(document, 1, 1)

    def test_stale_geometry_wrong_source_and_invalid_numbers_are_rejected(self):
        for field, value in (("frame", 39), ("page", 2), ("rect", [20, float("nan"), 400, 1500]),
                             ("interact", [20, 40, 400, 0]), ("zoom", True)):
            document = self.document()
            document["pdfium_render"]["page_geometry"][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_automatic_pdf_follow(document, 1, 1)
        for position in (None, {}, {"revision": 1, "page": 1, "point": [200, 400]},
                         {"revision": 2, "page": 0, "point": [200, 400]},
                         {"revision": 2, "page": 1, "point": [200, float("inf")]}):
            document = self.document()
            document["pdfium_render"]["source_position"] = position
            with self.subTest(position=position), self.assertRaises(AssertionError):
                assert_automatic_pdf_follow(document, 1, 1)

    def test_default_scroll_preservation_rejects_any_source_follow_or_jump(self):
        document = self.document()
        document["pdfium_render"]["source_position"] = None
        baseline = self.baseline()
        result = assert_pdfium_scroll_preserved(document, baseline)
        self.assertEqual(result["page_origin"], [20, -100])
        for axis in (0, 1):
            moved = copy.deepcopy(document)
            moved["pdfium_render"]["page_geometry"]["rect"][axis] += 25
            with self.subTest(axis=axis), self.assertRaisesRegex(AssertionError, "scroll position"):
                assert_pdfium_scroll_preserved(moved, baseline)
        changed_page = copy.deepcopy(document)
        changed_page["preview_page"] = changed_page["pdfium_render"]["page_geometry"]["page"] = 0
        with self.assertRaisesRegex(AssertionError, "scroll position"):
            assert_pdfium_scroll_preserved(changed_page, baseline)
        with self.assertRaisesRegex(AssertionError, "still applied a source destination"):
            assert_pdfium_scroll_preserved(self.document(), baseline)

    def test_auto_compile_checks_require_an_observed_absent_animation(self):
        for cue in (None, {}, {"active": True, "pending": False},
                    {"active": False, "pending": True}):
            document = self.document()
            document["preview_reveal"] = cue
            with self.subTest(cue=cue), self.assertRaisesRegex(AssertionError, "reveal animation"):
                assert_automatic_pdf_follow(document, 1, 1)


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
