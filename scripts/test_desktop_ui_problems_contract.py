"""Problems observations must prove actual filtered membership and keyboard focus."""

from copy import deepcopy
import unittest
from unittest.mock import patch

from desktop_ui_problems_journeys import ProblemsJourneys, assert_problems_search_did_not_navigate, problems_query_selected, problems_scroll, problems_state


def inspected_problems():
    return {"query": "", "source": None, "type": None, "category": None,
            "grouped": True, "controls_visible": True, "sticky_filters": False,
            "visible_count": 3, "displayed_count": 3, "total_count": 3,
            "visible_indices": [0, 1, 2], "groups": [
                {"provider": "Typst", "type": "Error", "count": 1, "open": True,
                 "target": "problems.group.Typst.Error"},
                {"provider": "Harper", "type": "Help", "count": 2, "open": True,
                 "target": "problems.group.Harper.Help"}]}


def scrolled_problems(offset=0, *, sticky=False, controls=True):
    layout = inspected_problems()
    layout.update(grouped=False, controls_visible=controls, sticky_filters=sticky)
    return {"document": {"frame": 40, "problems": layout, "raw_diagnostic_count": 3}, "scrolls": {"problems": {
        "x": 150, "y": 400, "frame": 40, "offset": offset, "sticky_search": sticky,
        "content_height": 1000, "viewport_height": 200}}}


def selected_query(query="misspelld", selection=None):
    return {"document": {"problems": {"query": query}}, "targets": {"problems.search": {
        "enabled": True, "focused": True, "age_ms": 0,
        "selection": [0, len(query)] if selection is None else selection}}}


class ProblemsContracts(unittest.TestCase):
    def test_group_collapse_preserves_filtered_count_and_original_row_identity(self):
        state = inspected_problems()
        self.assertIs(problems_state({"problems": state, "raw_diagnostic_count": 3}), state)
        state.update(query="misspelld", source="Harper", type="Help", visible_count=1,
                     displayed_count=0, visible_indices=[2], groups=[state["groups"][1]])
        state["groups"][0].update(count=1, open=False)
        self.assertIs(problems_state({"problems": state, "raw_diagnostic_count": 3}), state)
        state.update(visible_count=0, displayed_count=0, visible_indices=[], groups=[])
        self.assertEqual(problems_state({"problems": state, "raw_diagnostic_count": 3})["total_count"], 3)

    def test_flat_view_displays_rows_without_provider_and_type_headers(self):
        state = inspected_problems()
        state["grouped"] = False
        state["groups"][1]["open"] = False
        self.assertEqual(problems_state({"problems": state, "raw_diagnostic_count": 3})["displayed_count"], 3)
        state["grouped"] = True
        with self.assertRaisesRegex(AssertionError, "group membership"):
            problems_state({"problems": state, "raw_diagnostic_count": 3})

    def test_layout_observation_requires_explicit_booleans_and_visible_active_filters(self):
        for flag in ("grouped", "controls_visible", "sticky_filters"):
            for value in (None, 1, "false"):
                state = inspected_problems()
                state[flag] = value
                with self.subTest(flag=flag, value=value), self.assertRaisesRegex(AssertionError, "layout"):
                    problems_state({"problems": state, "raw_diagnostic_count": 3})
        state = inspected_problems()
        state.update(sticky_filters=True, controls_visible=False)
        with self.assertRaisesRegex(AssertionError, "remain visible"):
            problems_state({"problems": state, "raw_diagnostic_count": 3})

    def test_native_scroll_requires_fresh_complete_geometry_and_actual_overflow(self):
        state = scrolled_problems()
        self.assertEqual(problems_scroll(state, sticky=False, overflow=True)["offset"], 0)
        for field, value in (("frame", 39), ("x", float("nan")), ("y", True),
                             ("offset", -1), ("viewport_height", 0), ("content_height", None)):
            broken = deepcopy(state)
            broken["scrolls"]["problems"][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "geometry"):
                problems_scroll(broken)
        state["scrolls"]["problems"]["sticky_search"] = True
        with self.assertRaisesRegex(AssertionError, "sticky"):
            problems_scroll(state)
        state = scrolled_problems()
        state["scrolls"]["problems"]["content_height"] = 250
        with self.assertRaisesRegex(AssertionError, "enough rows"):
            problems_scroll(state, overflow=True)

    def test_native_wheel_moves_results_then_checks_persistent_offset_and_filter_visibility(self):
        for sticky in (False, True):
            with self.subTest(sticky=sticky):
                journey = ProblemsJourneys()
                before = scrolled_problems(sticky=sticky)
                after = scrolled_problems(80, sticky=sticky, controls=sticky)
                states = iter([before, before] + [after] * 9)
                actions, records = [], []
                journey.snapshot = lambda: next(states)
                journey.native = lambda *args: actions.append(args)
                journey.record = lambda *args, **kwargs: records.append((args, kwargs))
                journey.problems_scroll_stays(sticky=sticky)
                self.assertEqual(actions, [("move", 150, 400), ("wheel", 150, 400, -8)])
                self.assertEqual(records[0][1], {"sticky": sticky, "before": 0, "after": 80})
                self.assertEqual(list(states), [], "all eight post-scroll stability observations are required")

    def test_native_scroll_reset_is_a_failure_rather_than_a_settled_pass(self):
        journey = ProblemsJourneys()
        before, moved = scrolled_problems(), scrolled_problems(80, controls=False)
        states = iter([before, before, moved, moved, scrolled_problems(0, controls=False)])
        journey.snapshot = lambda: next(states)
        journey.native = lambda *args: None
        journey.record = lambda *args, **kwargs: self.fail("cannot record reset scrolling as stable")
        with self.assertRaisesRegex(AssertionError, "fought back"):
            journey.problems_scroll_stays(sticky=False)

    def test_sticky_search_double_click_posts_once_to_fresh_target_and_checks_source_owner(self):
        state = scrolled_problems(120, sticky=True)
        state["document"].update(viewport="main", path="/tmp/fixture.typ",
            source_fingerprint="source", source_bytes=400, cursor=[7, 7], tabs=2,
            active_tab_id=3, problems_keyboard_focused=True, editor_keyboard_focused=False)
        state["document"]["problems"]["query"] = "Problem"
        state["targets"] = {"problems.search": dict(x=120, y=280, width=160, height=20,
            enabled=True, focused=True, frame=40, viewport="main", age_ms=0)}
        after = deepcopy(state["document"])
        after["frame"] += 1
        for target_change in (None, {"frame": 39}, {"age_ms": 501}, {"viewport": "other"},
                              {"focused": False}, {"x": float("nan")}):
            changed = deepcopy(state)
            if target_change:
                changed["targets"]["problems.search"].update(target_change)
            journey = ProblemsJourneys()
            actions = []
            journey.wait_target = lambda *args, **kwargs: None
            journey.snapshot = lambda: changed
            journey.native = lambda *args: actions.append(args)
            journey.record = lambda *args, **kwargs: None
            journey.wait = lambda description, predicate: self.assertTrue(predicate(after), description)
            journey.stable_state = lambda predicate, frames: [self.assertTrue(predicate(after)) for _ in range(frames)]
            if target_change:
                with self.subTest(change=target_change), self.assertRaisesRegex(AssertionError, "stale"):
                    journey.double_click_sticky_problems_search()
                self.assertEqual(actions, [], "stale coordinates must not receive native input")
            else:
                journey.double_click_sticky_problems_search()
                self.assertEqual(actions, [("double-click", 120, 280)])

    def test_double_click_guard_rejects_navigation_even_when_problem_counts_stay_the_same(self):
        before = dict(path="/tmp/fixture.typ", source_fingerprint="source", source_bytes=400,
                      cursor=[7, 7], tabs=2, active_tab_id=3,
                      problems_keyboard_focused=True, editor_keyboard_focused=False)
        self.assertTrue(assert_problems_search_did_not_navigate(before, deepcopy(before)))
        for key, value in (("path", "/tmp/other.typ"), ("source_fingerprint", "edited"),
                           ("source_bytes", 401), ("cursor", [80, 80]), ("tabs", 3), ("active_tab_id", 4)):
            changed = {**before, key: value}
            with self.subTest(field=key), self.assertRaisesRegex(AssertionError, "navigated"):
                assert_problems_search_did_not_navigate(before, changed)
            changed = deepcopy(before)
            del changed[key]
            with self.subTest(missing=key), self.assertRaisesRegex(AssertionError, "navigated"):
                assert_problems_search_did_not_navigate(before, changed)
        self.assertFalse(assert_problems_search_did_not_navigate(before,
            {**before, "editor_keyboard_focused": True}))
        self.assertFalse(assert_problems_search_did_not_navigate(before,
            {**before, "problems_keyboard_focused": False}))

    def test_missing_result_inspection_never_counts_as_an_empty_pass(self):
        for document in ({}, {"problems": None}, {"problems": {}}, {"problems": {"visible_count": 0}}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                problems_state(document)

    def test_counts_and_original_row_identities_must_agree(self):
        for field, value in [("total_count", 2), ("visible_count", 2), ("displayed_count", 4),
                             ("displayed_count", True), ("visible_indices", [0, 1, 1]),
                             ("visible_indices", [0, 1, 3]), ("visible_indices", [0, 1, False])]:
            state = inspected_problems()
            state[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                problems_state({"problems": state, "raw_diagnostic_count": 3})

    def test_suppression_keeps_original_indices_instead_of_bounding_them_by_projected_count(self):
        state = inspected_problems()
        state.update(visible_count=2, displayed_count=2, total_count=2,
                     visible_indices=[0, 4])
        state["groups"][1]["count"] = 1
        self.assertIs(problems_state({"problems": state, "raw_diagnostic_count": 5}), state)
        for invalid in (5, -1, True):
            broken = deepcopy(state)
            broken["visible_indices"][1] = invalid
            with self.subTest(index=invalid), self.assertRaisesRegex(AssertionError, "row identities"):
                problems_state({"problems": broken, "raw_diagnostic_count": 5})

    def test_raw_count_is_required_and_cannot_be_smaller_than_projected_count(self):
        for value in (None, True, -1, 2, 3.0):
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, "raw diagnostic count"):
                problems_state({"problems": inspected_problems(), "raw_diagnostic_count": value})
        with self.assertRaisesRegex(AssertionError, "raw diagnostic count"):
            problems_state({"problems": inspected_problems()})

    def test_group_membership_and_expansion_must_agree_with_displayed_rows(self):
        for field, value in [("count", 1), ("open", False), ("open", 1), ("provider", ""),
                             ("type", "unsupported"), ("target", "problem.2")]:
            state = inspected_problems()
            state["groups"][1][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                problems_state({"problems": state, "raw_diagnostic_count": 3})

    def test_selected_provider_and_type_must_restrict_every_group(self):
        for field, value in [("source", "Harper"), ("type", "Help")]:
            state = inspected_problems()
            state[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "selected filters"):
                problems_state({"problems": state, "raw_diagnostic_count": 3})
        state = inspected_problems()
        state["groups"][1]["target"] = state["groups"][0]["target"]
        with self.assertRaisesRegex(AssertionError, "selected filters"):
            problems_state({"problems": state, "raw_diagnostic_count": 3})
        del state["source"]
        with self.assertRaisesRegex(AssertionError, "unavailable"):
            problems_state({"problems": state, "raw_diagnostic_count": 3})

    def test_select_all_requires_current_focused_problems_field_and_complete_selection(self):
        state = selected_query()
        self.assertTrue(problems_query_selected(state))
        state["targets"]["problems.search"]["selection"].reverse()
        self.assertTrue(problems_query_selected(state))
        for field, value in [("age_ms", 501), ("age_ms", -1), ("focused", False),
                             ("enabled", False), ("selection", [1, 9]),
                             ("selection", [False, 9]), ("selection", None)]:
            broken = deepcopy(state)
            broken["targets"]["problems.search"][field] = value
            with self.subTest(field=field, value=value):
                self.assertFalse(problems_query_selected(broken))
        self.assertFalse(problems_query_selected({"document": {}, "targets": {}}))

    def test_query_replacement_waits_for_native_selection_receipt(self):
        for replacement in ("no-match", ""):
            with self.subTest(replacement=replacement):
                journey = ProblemsJourneys()
                pending, ready = selected_query(selection=[1, 1]), selected_query()
                states = iter([pending, ready])
                actions = []
                document = deepcopy(ready["document"])
                journey.snapshot = lambda: next(states)
                journey.wait_target = lambda *args, **kwargs: actions.append(("focus", args, kwargs))

                def key(code, modifiers="cmd"):
                    actions.append(("key", code, modifiers))
                    if code == 51:
                        document["problems"]["query"] = ""

                def native(command, text):
                    self.assertEqual(list(states), [], "text was posted before Select All acknowledgement")
                    document["problems"]["query"] = text
                    actions.append((command, text))

                journey.key, journey.native = key, native
                journey.wait = lambda description, predicate: self.assertTrue(predicate(document), description)
                with patch("desktop_ui_problems_journeys.time.sleep") as sleep:
                    journey.replace_problems_query(replacement)
                sleep.assert_called_once_with(.05)
                self.assertEqual(actions[1], ("key", 0, "cmd"))
                self.assertEqual(actions[-1], ("text", replacement) if replacement else ("key", 51, "none"))

    def test_selection_timeout_fails_before_posting_replacement(self):
        journey = ProblemsJourneys()
        actions = []
        journey.wait_target = lambda *args, **kwargs: None
        journey.key = lambda *args: actions.append(args)
        journey.snapshot = lambda: selected_query(selection=[1, 1])
        journey.native = lambda *args: self.fail("cannot post text without a selection receipt")
        with patch("desktop_ui_problems_journeys.time.monotonic", side_effect=[0, 7]):
            with self.assertRaisesRegex(AssertionError, "acknowledge Select All"):
                journey.replace_problems_query("replacement")
        self.assertEqual(actions, [(0,)])


if __name__ == "__main__":
    unittest.main()
