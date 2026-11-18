"""Native exclusion evidence must preserve raw findings and exact outer rows."""

from copy import deepcopy
from pathlib import Path
import unittest

from desktop_ui_journeys import fingerprint
from desktop_ui_comment_controls_journeys import (
    CUSTOM_COMMENT_MARKERS, DEFAULT_COMMENT_MARKERS, comment_diagnostic_fixture,
)
from desktop_ui_tex_journeys import (
    TexJourneys, diagnostic_control_marker_lines, diagnostic_suppression_fixture, harper_suppression_result,
)


def observation(suffix="typ", active=True, *, metadata=False):
    source, excluded, locations = diagnostic_suppression_fixture(suffix)
    source = excluded if active else source
    visible = locations[:2] + locations[4:] if active else locations
    path = Path("/private/tmp/native-suppression/fixture." + suffix)
    offset = 2 if suffix == 'tex' else 0
    marker_lines = {3 + offset, 6 + offset}
    raw_locations = sorted(locations + ([[line, 3] for line in sorted(marker_lines)] * 2
                                        if metadata else []))
    rows = [{"index": 0, "provider": None, "line": None, "column": None, "suppressed": False}]
    rows += [{"index": index + 1, "provider": "Harper", "line": line,
              "column": column, "suppressed": line in marker_lines or [line, column] not in visible}
             for index, (line, column) in enumerate(raw_locations)]
    if metadata:
        rows.append({'index': len(rows), 'provider': 'Badness', 'line': min(marker_lines),
                     'column': 1, 'suppressed': True})
    indices = [row["index"] for row in rows if not row["suppressed"]]
    document = {
        "path": str(path), "panel": "Problems", "source_fingerprint": fingerprint(source),
        "source_bytes": len(source.encode("utf-8")), "writing_current": True,
        "writing_pending": False, "writing_count": len(raw_locations),
        "writing_locations": raw_locations, "writing_lines": [line for line, _ in raw_locations],
        "writing_paths": [str(path)] * len(raw_locations), "diagnostic_rows": rows,
        "ltex": {"enabled": False, "current": False},
        'comment_controls': {
            group: {switch + '_fingerprint': fingerprint(value)
                    for switch, value in zip(('off', 'on'), pair)}
            for group, pair in DEFAULT_COMMENT_MARKERS.items()},
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
    def test_enabled_ltex_must_settle_before_accepting_harper_only_layout(self):
        source, path, locations, visible, document = observation('tex', metadata=True)
        document['ltex'] = {'enabled': True, 'current': False}
        # Captured native sequence: Harper is already current, then LTeX adds
        # six rows while Problems still describes the preceding layout frame.
        # Accepting this first state makes the later stability check fail.
        self.assertFalse(harper_suppression_result(document, source, path, locations, visible))
        document['ltex']['current'] = True
        self.assertTrue(harper_suppression_result(document, source, path, locations, visible))

    def test_whitespace_variant_metadata_requires_exact_effective_settings_and_stays_hidden(self):
        for suffix in ('typ', 'tex'):
            for markers in (DEFAULT_COMMENT_MARKERS, CUSTOM_COMMENT_MARKERS):
                for pair in markers.values():
                    _, source, _ = comment_diagnostic_fixture(suffix, pair)
                    _, path, locations, visible, document = observation(suffix, metadata=True)
                    document.update(source_fingerprint=fingerprint(source), source_bytes=len(source.encode()),
                                    comment_controls={
                                        group: {switch + '_fingerprint': fingerprint(value)
                                                for switch, value in zip(('off', 'on'), values)}
                                        for group, values in markers.items()})
                    offset = 2 if suffix == 'tex' else 0
                    self.assertEqual(diagnostic_control_marker_lines(document, source, path, markers),
                                     {3 + offset, 6 + offset})
                    self.assertTrue(harper_suppression_result(
                        document, source, path, locations, visible, markers=markers))
                    stale = deepcopy(document)
                    stale['comment_controls']['both']['off_fingerprint'] = fingerprint('different')
                    self.assertFalse(harper_suppression_result(
                        stale, source, path, locations, visible, markers=markers))
                    marker_row = next(row for row in document['diagnostic_rows'] if row['provider'] == 'Badness')
                    marker_row['suppressed'] = False
                    self.assertFalse(harper_suppression_result(
                        document, source, path, locations, visible, markers=markers))

    def test_marker_metadata_findings_remain_raw_and_hidden_even_when_regions_are_enabled(self):
        for suffix in ('typ', 'tex'):
            for active in (False, True, False):
                source, path, locations, visible, document = observation(suffix, active, metadata=True)
                self.assertEqual(document['writing_count'], 10)
                self.assertEqual(len(locations), 6)
                self.assertTrue(harper_suppression_result(document, source, path, locations, visible))
                self.assertTrue(all(row['suppressed'] for row in document['diagnostic_rows']
                                    if row['provider'] == 'Badness'))

    def test_visible_marker_metadata_from_another_provider_cannot_pass_with_consistent_counts(self):
        source, path, locations, visible, document = observation('tex', metadata=True)
        metadata = next(row for row in document['diagnostic_rows'] if row['provider'] == 'Badness')
        metadata['suppressed'] = False
        layout = document['problems']
        layout['visible_indices'].append(metadata['index'])
        for key in ('visible_count', 'displayed_count', 'total_count'):
            layout[key] += 1
        layout['groups'].append({'provider': 'Badness', 'type': 'Warning', 'count': 1,
                                 'open': True, 'target': 'problems.group.Badness.Warning'})
        document['diagnostic_count'] += 1
        self.assertFalse(harper_suppression_result(document, source, path, locations, visible))

    def test_extra_raw_rows_on_ordinary_or_unconfigured_comments_are_not_metadata(self):
        source, path, locations, visible, document = observation('tex', metadata=True)
        unexpected = deepcopy(document)
        row = next(row for row in unexpected['diagnostic_rows']
                   if row['provider'] == 'Harper' and row['line'] == 5)
        row['line'], row['column'] = 3, 1
        raw = [[row['line'], row['column']] for row in unexpected['diagnostic_rows']
               if row['provider'] == 'Harper']
        unexpected['writing_locations'] = raw
        unexpected['writing_lines'] = [line for line, _ in raw]
        self.assertFalse(harper_suppression_result(unexpected, source, path, locations, visible))
        for replacement in ('% unknown: off', 'code % ttt: off'):
            ordinary = source.replace('% ttt: off', replacement, 1)
            updated = {**document, 'source_fingerprint': fingerprint(ordinary),
                       'source_bytes': len(ordinary.encode('utf-8'))}
            self.assertFalse(harper_suppression_result(updated, ordinary, path, locations, visible))

    def test_real_typst_and_tex_fixtures_keep_exact_before_inside_after_line_positions(self):
        for suffix, marker, offset in (("typ", "//", 0), ("tex", "%", 2)):
            source, active, locations = diagnostic_suppression_fixture(suffix)
            self.assertEqual(source.count(marker + " ttt: on"), 2)
            self.assertEqual(active.count(marker + " ttt: off"), 1)
            self.assertEqual(active.count(marker + " ttt: on"), 1)
            self.assertEqual(source.replace("ttt: on", "ttt: off", 1), active)
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
