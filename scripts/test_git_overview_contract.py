"""Failure-detector checks for the native Git overview journey; no desktop claimed."""
import sys
import unittest

sys.dont_write_bytecode = True
from desktop_ui_journeys import assert_git_overview


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


if __name__ == "__main__":
    unittest.main()
