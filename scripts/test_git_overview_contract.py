"""Failure-detector checks for the native Git overview journey; no desktop claimed."""
import sys
import unittest

sys.dont_write_bytecode = True
from desktop_ui_journeys import (EditorJourneys, assert_git_overview,
                                 git_diff_selection_popup_point)


class GitOverviewContract(unittest.TestCase):
    def document(self):
        return {"editor_panel_width": 400, "editor_panel_rect": [30, 20, 430, 670], "git_overview": {
            "viewport": [30, 50, 430, 650], "track": [425, 50, 429, 650],
            "content_height": 9000, "offset": 0, "minimum_thumb_height": 12,
            "leading_occlusion": 0,
            "thumb": [50, 90],
            "rendered_changes": [{"kind": "Modified", "content": extent}
                                 for extent in ([5300, 5320], [7000, 7020], [8800, 8820])],
            "ticks": [{"kind": "Modified", "rect": [425, y, 429, y + 2]}
                      for y in (402.6333, 515.9667, 608)]}}

    def test_accepts_all_global_marks_including_the_offscreen_last_hunk(self):
        value = assert_git_overview(self.document())
        self.assertEqual(len(value["ticks"]), 3)
        self.assertGreater(value["ticks"][-1][1], 590)

    def test_rejects_missing_or_viewport_relative_hunks(self):
        for changed in (None, {"viewport": [30, 50, 430, 650], "track": [425, 50, 429, 650], "ticks": []}):
            with self.subTest(value=changed), self.assertRaises(AssertionError):
                assert_git_overview({**self.document(), "git_overview": changed})
        for count in (1, 2):
            document = self.document()
            document["git_overview"]["ticks"] = document["git_overview"]["ticks"][:count]
            with self.subTest(count=count), self.assertRaises(AssertionError):
                assert_git_overview(document)
        document = self.document()
        document["git_overview"]["ticks"][-1]["rect"] = [425, 90, 429, 92]
        with self.assertRaisesRegex(AssertionError, "rendered hunk is offscreen"):
            assert_git_overview(document)

    def test_rejects_old_logical_line_percentages_when_rendered_rows_wrap(self):
        document = self.document()
        document["git_overview"]["ticks"] = [
            {"kind": "Modified", "rect": [425, 50 + 600 * line / 181, 429,
                                           52 + 600 * line / 181]}
            for line in (5, 85, 165)]
        with self.assertRaisesRegex(AssertionError, "rendered hunk is offscreen"):
            assert_git_overview(document)

    def test_rejects_wrong_thumb_geometry_or_missing_rendered_ranges(self):
        mutations = [
            lambda d: d["git_overview"].update(thumb=[50, 100]),
            lambda d: d["git_overview"].update(offset=100),
            lambda d: d["git_overview"].update(content_height=float("nan")),
            lambda d: d["git_overview"].update(rendered_changes=[]),
            lambda d: d["git_overview"]["rendered_changes"][0].update(content=[4, 3]),
        ]
        for mutate in mutations:
            document = self.document()
            mutate(document)
            with self.subTest(document=document), self.assertRaises(AssertionError):
                assert_git_overview(document)

    def test_contact_must_reveal_the_hunk_below_the_leading_sticky_cover(self):
        document = self.document()
        document["git_overview"]["leading_occlusion"] = 271
        with self.assertRaisesRegex(AssertionError, "rendered hunk is offscreen"):
            assert_git_overview(document)
        for tick, y in zip(document["git_overview"]["ticks"], (384.5667, 497.9, 608)):
            tick["rect"][1] = y
            tick["rect"][3] = y + 2
        assert_git_overview(document)

    def test_rejects_a_shifted_viewport_even_when_its_width_matches_the_pane(self):
        document = self.document()
        overview = document["git_overview"]
        for rect in [overview["viewport"], overview["track"], *(tick["rect"] for tick in overview["ticks"])]:
            rect[0] += 400
            rect[2] += 400
        with self.assertRaisesRegex(AssertionError, "beyond its code pane"):
            assert_git_overview(document)

    def test_rejects_wrong_color_semantics_and_oversized_or_escaped_strips(self):
        mutations = [
            lambda d: d["git_overview"]["ticks"][0].update(kind="Added"),
            lambda d: d["git_overview"].update(track=[424, 50, 429, 650]),
            lambda d: d["git_overview"].update(track=[428, 50, 432, 650]),
            lambda d: d["git_overview"]["ticks"][0].update(rect=[420, 66, 424, 68]),
            lambda d: d.update(editor_panel_width=399 - 2),
        ]
        for mutate in mutations:
            document = self.document()
            mutate(document)
            with self.subTest(document=document), self.assertRaises(AssertionError):
                assert_git_overview(document)

    def test_rejects_nonfinite_boolean_or_degenerate_geometry(self):
        for invalid in (float("nan"), float("inf"), True, None):
            document = self.document()
            document["git_overview"]["track"][0] = invalid
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                assert_git_overview(document)
        document = self.document()
        document["git_overview"]["viewport"] = [30, 50, 430, 50]
        with self.assertRaises(AssertionError):
            assert_git_overview(document)


class GitSelectionPopupContract(unittest.TestCase):
    def state(self, frame=10, selection=(100, 100)):
        return {"document": {
            "frame": frame, "cursor": list(selection), "git_diff_open": True,
            "editor_keyboard_focused": True, "active_tab_id": 4,
            "path": "/fixture/hunks.typ", "revision": "7", "source_fingerprint": "same",
            "git_overview": {"offset": 5994.5, "content_height": 18939},
        }, "targets": {
            "editor.source": {"x": 725, "y": 500, "width": 240, "height": 800,
                              "frame": frame, "selection": list(selection), "viewport": "root",
                              "age_ms": 0, "enabled": True, "focused": True},
            "editor.caret": {"x": 606, "y": 545, "frame": frame,
                             "viewport": "root", "age_ms": 0, "enabled": True},
        }}

    def test_uses_an_unchanged_visible_source_anchor_with_current_selection_receipt(self):
        before = self.state()
        selected = self.state(11, (122, 100))
        # A selected diff deliberately has no current collapsed-caret target.
        selected["targets"].pop("editor.caret")
        self.assertEqual(git_diff_selection_popup_point(before, selected), (607, 545))

    def test_rejects_the_observed_offscreen_caret_failure_and_stale_anchors(self):
        for mutate in (
            lambda s: s["targets"]["editor.caret"].update(y=3772.75),
            lambda s: s["targets"]["editor.caret"].update(frame=9),
            lambda s: s["targets"]["editor.source"].update(frame=9),
        ):
            before = self.state()
            mutate(before)
            with self.subTest(before=before), self.assertRaises(AssertionError):
                git_diff_selection_popup_point(before, self.state(11, (122, 100)))

    def test_refuses_changed_owner_selection_viewport_scroll_or_layout(self):
        mutations = (
            lambda s: s["document"].update(active_tab_id=5),
            lambda s: s["document"].update(revision="8"),
            lambda s: s["document"].update(source_fingerprint="edited"),
            lambda s: s["document"].update(cursor=[122, 101]),
            lambda s: s["targets"]["editor.source"].update(frame=10),
            lambda s: s["targets"]["editor.source"].update(selection=[100, 100]),
            lambda s: s["targets"]["editor.source"].update(viewport="other"),
            lambda s: s["targets"]["editor.source"].update(y=501),
            lambda s: s["targets"]["editor.source"].update(x=float("nan")),
            lambda s: s["document"]["git_overview"].update(offset=5996),
            lambda s: s["document"]["git_overview"].update(content_height=18941),
        )
        for mutate in mutations:
            selected = self.state(11, (122, 100))
            mutate(selected)
            with self.subTest(selected=selected), self.assertRaises(AssertionError):
                git_diff_selection_popup_point(self.state(), selected)

    def test_settling_requires_three_fresh_visible_stable_caret_frames(self):
        from unittest.mock import patch
        journey = EditorJourneys()
        offscreen = self.state()
        offscreen["targets"]["editor.caret"]["y"] = 3772.75
        stale = self.state(11)
        stale["targets"]["editor.caret"]["frame"] = 10
        frames = iter((offscreen, stale, self.state(12), self.state(13), self.state(14)))
        journey.snapshot = lambda: next(frames)
        with patch("time.sleep"):
            settled = journey.settled_git_diff_caret()
        self.assertEqual(settled["document"]["frame"], 14)

    def test_context_gesture_revalidates_selection_after_moving_the_pointer(self):
        from unittest.mock import patch
        journey = EditorJourneys()
        frames = iter((self.state(11, (122, 100)), self.state(12, (122, 100))))
        journey.snapshot = lambda: next(frames)
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.record = lambda *args, **kwargs: None
        with patch("time.sleep"):
            journey.right_click_git_diff_selection(self.state(), (100, 122))
        self.assertEqual(actions, [("move", 607, 545), ("right-click", 607, 545)])

    def test_hover_motion_cannot_click_after_a_selection_change(self):
        from unittest.mock import patch
        journey = EditorJourneys()
        frames = iter((self.state(11, (122, 100)), self.state(12, (121, 100))))
        journey.snapshot = lambda: next(frames)
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.record = lambda *args, **kwargs: None
        with patch("time.sleep"), self.assertRaisesRegex(AssertionError, "hover changed"):
            journey.right_click_git_diff_selection(self.state(), (100, 122))
        self.assertEqual(actions, [("move", 607, 545)])


if __name__ == "__main__":
    unittest.main()
