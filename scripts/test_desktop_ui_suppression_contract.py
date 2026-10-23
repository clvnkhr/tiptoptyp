"""Native exclusion evidence must preserve raw findings and exact outer rows."""

from copy import deepcopy
from pathlib import Path
import unittest

from desktop_ui_journeys import fingerprint
from desktop_ui_tex_journeys import (
    TexJourneys, diagnostic_suppression_fixture, harper_suppression_result,
)


def observation(suffix="typ", active=True):
    source, excluded, locations = diagnostic_suppression_fixture(suffix)
    source = excluded if active else source
    visible = locations[:2] + locations[4:] if active else locations
    path = Path("/private/tmp/native-suppression/fixture." + suffix)
    rows = [{"index": 0, "provider": None, "line": None, "column": None, "suppressed": False}]
    rows += [{"index": index + 1, "provider": "Harper", "line": line,
              "column": column, "suppressed": [line, column] not in visible}
             for index, (line, column) in enumerate(locations)]
    indices = [row["index"] for row in rows if not row["suppressed"]]
    document = {
        "path": str(path), "panel": "Problems", "source_fingerprint": fingerprint(source),
        "source_bytes": len(source.encode("utf-8")), "writing_current": True,
        "writing_pending": False, "writing_count": len(locations),
        "writing_locations": locations, "writing_lines": [line for line, _ in locations],
        "writing_paths": [str(path)] * len(locations), "diagnostic_rows": rows,
        "raw_diagnostic_count": len(rows), "diagnostic_count": len(indices),
        "problems": {
            "query": "", "source": None, "type": None, "category": None,
            "grouped": False, "controls_visible": False, "sticky_filters": False,
            "visible_count": len(indices), "displayed_count": len(indices), "total_count": len(indices),
            "visible_indices": indices,
            "groups": [{"provider": "Compiler", "type": "Unknown", "count": 1,
                        "open": True, "target": "problems.group.Compiler.Unknown"},
                       {"provider": "Harper", "type": "Help", "count": len(visible),
                        "open": True, "target": "problems.group.Harper.Help"}],
        },
    }
    return source, path, locations, visible, document


class SuppressionContracts(unittest.TestCase):
    def test_real_typst_and_tex_fixtures_keep_exact_before_inside_after_line_positions(self):
        for suffix, marker, offset in (("typ", "//", 0), ("tex", "%", 2)):
            source, active, locations = diagnostic_suppression_fixture(suffix)
            self.assertEqual(source.count(marker + " ttt: off"), 2)
            self.assertEqual(active.count(marker + " ttt: on"), 1)
            self.assertEqual(active.count(marker + " ttt: off"), 1)
            self.assertEqual(source.replace("ttt: off", "ttt: on", 1), active)
            self.assertEqual(locations, [[line + offset, column] for line, column in
                             ((1, 26), (2, 9), (4, 26), (5, 9), (7, 26), (8, 9))])
            for line, column in locations:
                content = source.splitlines()[line - 1]
                self.assertIn("misspelld" if column == 26 else "the the", content)
        with self.assertRaises(ValueError):
            diagnostic_suppression_fixture("txt")
        self.assertTrue(callable(TexJourneys.harper_suppression))

    def test_all_to_excluded_to_restored_retains_raw_provider_findings(self):
        for suffix in ("typ", "tex"):
            for active in (False, True, False):
                source, path, locations, visible, document = observation(suffix, active)
                with self.subTest(language=suffix, active=active):
                    self.assertEqual(document["writing_count"], 6)
                    self.assertTrue(harper_suppression_result(document, source, path, locations, visible))

    def test_stale_wrong_source_or_provider_results_never_pass_as_suppression(self):
        source, path, locations, visible, document = observation()
        for field, value in (("writing_current", False), ("writing_pending", True),
                             ("source_fingerprint", "stale"), ("source_bytes", 0),
                             ("writing_count", 4), ("writing_locations", visible),
                             ("writing_paths", ["/tmp/generated.typ"] * 6), ("panel", "Activity")):
            broken = {**document, field: value}
            with self.subTest(field=field):
                self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))

    def test_missing_or_truncated_raw_inspection_is_unavailable_not_an_empty_pass(self):
        source, path, locations, visible, document = observation()
        for field, value in (("raw_diagnostic_count", None), ("raw_diagnostic_count", True),
                             ("raw_diagnostic_count", 5), ("diagnostic_rows", None),
                             ("diagnostic_rows", []), ("diagnostic_rows", document["diagnostic_rows"][:-1])):
            with self.subTest(field=field, value=value), self.assertRaisesRegex(AssertionError, "inspection"):
                harper_suppression_result({**document, field: value}, source, path, locations, visible)

    def test_malformed_raw_identity_or_locations_cannot_support_a_pass(self):
        source, path, locations, visible, document = observation()
        for field, value in (("index", 0), ("index", True), ("provider", False),
                             ("suppressed", 1), ("line", 0), ("line", True),
                             ("column", -1), ("column", "26")):
            broken = deepcopy(document)
            broken["diagnostic_rows"][1][field] = value
            with self.subTest(field=field, value=value), self.assertRaisesRegex(AssertionError, "identity"):
                harper_suppression_result(broken, source, path, locations, visible)
        for field in ("index", "provider", "suppressed", "line", "column"):
            broken = deepcopy(document)
            del broken["diagnostic_rows"][1][field]
            with self.subTest(missing=field), self.assertRaisesRegex(AssertionError, "identity"):
                harper_suppression_result(broken, source, path, locations, visible)

    def test_same_counts_with_the_wrong_outer_findings_are_rejected(self):
        source, path, locations, visible, document = observation()
        broken = deepcopy(document)
        # Hide the first block instead of the middle block, preserving every count.
        for row in broken["diagnostic_rows"][1:]:
            row["suppressed"] = row["line"] in (1, 2)
        broken["problems"]["visible_indices"] = [0, 3, 4, 5, 6]
        self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))

    def test_exact_row_visibility_must_match_flags_without_discarding_unrelated_results(self):
        source, path, locations, visible, document = observation()
        broken = deepcopy(document)
        broken["diagnostic_rows"][3]["suppressed"] = False
        self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))
        broken = deepcopy(document)
        broken["problems"].update(visible_count=4, displayed_count=4, total_count=4,
                                 visible_indices=[1, 2, 5, 6])
        broken["problems"]["groups"].pop(0)
        broken["diagnostic_count"] = 4
        self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))

    def test_relabelled_remapped_or_boolean_diagnostic_counts_are_rejected(self):
        source, path, locations, visible, document = observation()
        for change in ("provider", "line", "column"):
            broken = deepcopy(document)
            broken["diagnostic_rows"][1][change] = "Other" if change == "provider" else 40
            with self.subTest(change=change):
                self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))
        for value in (True, None, 7):
            self.assertFalse(harper_suppression_result(
                {**document, "diagnostic_count": value}, source, path, locations, visible))

    def test_existing_ui_filters_and_collapsed_grouping_cannot_mask_a_false_pass(self):
        source, path, locations, visible, document = observation()
        for field, value in (("query", "typo"), ("grouped", True)):
            broken = deepcopy(document)
            broken["problems"][field] = value
            with self.subTest(field=field):
                self.assertFalse(harper_suppression_result(broken, source, path, locations, visible))


if __name__ == "__main__":
    unittest.main()
