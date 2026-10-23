"""Activity evidence must prove membership, source safety, and real keyboard input."""

from copy import deepcopy
import unittest
from unittest.mock import patch

from desktop_ui_activity_journeys import (
    ACTIVITY_JOURNEYS, ActivityJourneys, activity_query_selected, activity_scroll,
    activity_state, assert_activity_preserves_source,
)


def inspected_activity():
    return {"query": "", "group": None, "status": None, "grouped": True,
            "controls_visible": False, "sticky_filters": False,
            "total_count": 4, "visible_count": 4, "displayed_count": 4,
            "rows": [{"name": "Git status", "group": "Git", "state": "Failed", "detail": "git unavailable"},
                     {"name": "Git hunks", "group": "Git", "state": "Waiting", "detail": "Stale"},
                     {"name": "Harper", "group": "Checks", "state": "Idle", "detail": "Up to date / idle"},
                     {"name": "Unicode", "group": "Checks", "state": "Inactive", "detail": "Disabled"}],
            "visible_names": ["Git status", "Git hunks", "Harper", "Unicode"],
            "groups": [{"id": "Git", "label": "Git", "count": 2, "open": True,
                        "target": "activity.group.Git"},
                       {"id": "Checks", "label": "Checks", "count": 2, "open": True,
                        "target": "activity.group.Checks"}]}


def scrolled_activity(offset=0, sticky=False):
    activity = inspected_activity()
    activity.update(controls_visible=sticky, sticky_filters=sticky)
    return {"document": {"frame": 40, "activity_view": activity}, "scrolls": {"activity": {
        "x": 150, "y": 400, "frame": 40, "offset": offset, "sticky_search": sticky,
        "content_height": 500, "viewport_height": 100}}}


def selected_query(query="git status", selection=None):
    return {"document": {"activity_view": {"query": query}}, "targets": {"activity.search": {
        "enabled": True, "focused": True, "age_ms": 0,
        "selection": [0, len(query)] if selection is None else selection}}}


def document_owner():
    return dict(path="/tmp/fixture.txt", source_fingerprint="source", source_bytes=400,
                cursor=[7, 7], tabs=2, active_tab_id=3, find_visible=False,
                activity_keyboard_focused=True, editor_keyboard_focused=False,
                panel_rect=[0, 600, 1000, 800])


class ActivityContracts(unittest.TestCase):
    def test_actual_journey_is_registered_with_the_native_editor_mixin(self):
        from desktop_ui_journeys import EditorJourneys
        self.assertEqual(ACTIVITY_JOURNEYS, ("activity",))
        self.assertIs(EditorJourneys.activity, ActivityJourneys.activity)

    def test_complete_inventory_and_related_group_membership_are_required(self):
        state = inspected_activity()
        self.assertIs(activity_state({"activity_view": state}), state)
        state["groups"][0]["open"] = False
        state["displayed_count"] = 2
        self.assertEqual(activity_state({"activity_view": state})["visible_count"], 4)
        state["grouped"] = False
        state["displayed_count"] = 4
        self.assertEqual(activity_state({"activity_view": state})["displayed_count"], 4)

    def test_missing_inspection_never_passes_as_an_empty_activity_inventory(self):
        for document in ({}, {"activity_view": None}, {"activity_view": {}},
                         {"activity_view": {"visible_count": 0}}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                activity_state(document)

    def test_unknown_filters_are_rejected(self):
        for key, value in (("query", None), ("group", "unknown"), ("group", []),
                           ("status", "Busy"), ("status", {})):
            state = inspected_activity()
            state[key] = value
            with self.subTest(key=key), self.assertRaisesRegex(AssertionError, "filters"):
                activity_state({"activity_view": state})

    def test_layout_flags_must_be_boolean_and_active_filters_visible(self):
        for key in ("grouped", "controls_visible", "sticky_filters"):
            for value in (None, 1, "false"):
                state = inspected_activity()
                state[key] = value
                with self.subTest(key=key, value=value), self.assertRaisesRegex(AssertionError, "layout"):
                    activity_state({"activity_view": state})
        state = inspected_activity()
        state["sticky_filters"] = True
        with self.assertRaisesRegex(AssertionError, "remain visible"):
            activity_state({"activity_view": state})

    def test_duplicate_or_unidentified_sources_cannot_support_a_pass(self):
        for key, value in (("name", ""), ("name", "Git hunks"), ("group", "missing"),
                           ("state", "Active"), ("detail", None)):
            state = inspected_activity()
            state["rows"][0][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(AssertionError, "inventory"):
                activity_state({"activity_view": state})
        for value in (["Git status"] * 4, ["missing", "Git hunks", "Harper", "Unicode"], [], None):
            state = inspected_activity()
            state["visible_names"] = value
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, "identities"):
                activity_state({"activity_view": state})

    def test_related_groups_must_account_for_each_filtered_source_exactly_once(self):
        for key, value in (("id", "Preview"), ("count", 1), ("count", True),
                           ("open", 1), ("target", "activity.group.Checks"), ("label", "")):
            state = inspected_activity()
            state["groups"][0][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(AssertionError, "group"):
                activity_state({"activity_view": state})
        state = inspected_activity()
        state["groups"][1]["open"] = False
        with self.assertRaisesRegex(AssertionError, "collapsed"):
            activity_state({"activity_view": state})

    def test_group_and_state_filters_restrict_visible_source_identities(self):
        state = inspected_activity()
        state.update(group="Git", status="Active", visible_names=["Git status", "Git hunks"],
                     visible_count=2, displayed_count=2, groups=[state["groups"][0]])
        self.assertEqual(activity_state({"activity_view": state})["visible_count"], 2)
        state.update(status="Failed", visible_names=["Git status"], visible_count=1, displayed_count=1)
        state["groups"][0]["count"] = 1
        self.assertEqual(activity_state({"activity_view": state})["visible_names"], ["Git status"])
        state["visible_names"] = ["Git hunks"]
        with self.assertRaisesRegex(AssertionError, "selected filters"):
            activity_state({"activity_view": state})
        state = inspected_activity()
        state.update(status="Idle", visible_names=[], groups=[], visible_count=0, displayed_count=0)
        with self.assertRaisesRegex(AssertionError, "lost or admitted"):
            activity_state({"activity_view": state})

    def test_case_insensitive_search_is_checked_independently_against_actual_source_fields(self):
        for query, names in (("GIT STATUS", ["Git status"]), ("unavailable", ["Git status"]),
                             ("waiting", ["Git hunks"]), ("checks", ["Harper", "Unicode"])):
            state = inspected_activity()
            state.update(query=query, visible_names=names, visible_count=len(names), displayed_count=len(names))
            state["groups"] = [group for group in state["groups"]
                               if any(row["name"] in names and row["group"] == group["id"]
                                      for row in state["rows"])]
            for group in state["groups"]:
                group["count"] = sum(row["name"] in names and row["group"] == group["id"]
                                     for row in state["rows"])
            with self.subTest(query=query):
                self.assertEqual(activity_state({"activity_view": state})["visible_names"], names)
                state["query"] = "no-match"
                with self.assertRaisesRegex(AssertionError, "lost or admitted"):
                    activity_state({"activity_view": state})

    def test_filtering_empty_results_retains_the_complete_source_inventory(self):
        state = inspected_activity()
        state.update(query="no-match", visible_names=[], groups=[], visible_count=0, displayed_count=0,
                     controls_visible=True, sticky_filters=True)
        self.assertEqual(activity_state({"activity_view": state})["total_count"], 4)
        self.assertEqual(len(state["rows"]), 4)

    def test_current_scroll_geometry_is_required_before_posting_native_wheel_input(self):
        state = scrolled_activity()
        self.assertEqual(activity_scroll(state, sticky=False)["offset"], 0)
        for key, value in (("frame", 39), ("x", float("nan")), ("y", True),
                           ("offset", -1), ("viewport_height", 0), ("content_height", None)):
            changed = deepcopy(state)
            changed["scrolls"]["activity"][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(AssertionError, "geometry"):
                activity_scroll(changed)
        state["scrolls"]["activity"]["sticky_search"] = True
        with self.assertRaisesRegex(AssertionError, "sticky"):
            activity_scroll(state)
        journey = ActivityJourneys()
        actions = []
        journey.snapshot = lambda: scrolled_activity()
        journey.native = lambda *args: actions.append(args)
        journey.scroll_activity(-8)
        self.assertEqual(actions, [("move", 150, 400), ("wheel", 150, 400, -8)])

    def test_source_cursor_document_owner_and_height_cannot_change_while_filtering(self):
        before = document_owner()
        self.assertTrue(assert_activity_preserves_source(before, deepcopy(before)))
        for key, value in (("path", "/tmp/other.txt"), ("source_fingerprint", "edited"),
                           ("source_bytes", 401), ("cursor", [8, 8]), ("tabs", 3),
                           ("active_tab_id", 4), ("find_visible", True),
                           ("activity_keyboard_focused", False), ("editor_keyboard_focused", True),
                           ("panel_rect", [0, 590, 1000, 800])):
            changed = {**before, key: value}
            with self.subTest(key=key), self.assertRaises(AssertionError):
                assert_activity_preserves_source(before, changed)
        for key in ("path", "source_fingerprint", "source_bytes", "cursor", "tabs", "active_tab_id"):
            changed = deepcopy(before)
            del changed[key]
            with self.subTest(missing=key), self.assertRaisesRegex(AssertionError, "navigated"):
                assert_activity_preserves_source(before, changed)

    def test_select_all_requires_current_focused_activity_field_and_complete_selection(self):
        state = selected_query()
        self.assertTrue(activity_query_selected(state))
        state["targets"]["activity.search"]["selection"].reverse()
        self.assertTrue(activity_query_selected(state))
        for key, value in (("age_ms", 501), ("age_ms", -1), ("focused", False),
                           ("enabled", False), ("selection", [1, 10]),
                           ("selection", [False, 10]), ("selection", None)):
            changed = deepcopy(state)
            changed["targets"]["activity.search"][key] = value
            with self.subTest(key=key, value=value):
                self.assertFalse(activity_query_selected(changed))
        self.assertFalse(activity_query_selected({"document": {}, "targets": {}}))

    def test_query_replacement_waits_for_native_selection_receipt(self):
        for replacement in ("no-match", ""):
            with self.subTest(replacement=replacement):
                journey = ActivityJourneys()
                pending, ready = selected_query(selection=[1, 1]), selected_query()
                states = iter([pending, ready])
                actions = []
                document = deepcopy(ready["document"])
                journey.snapshot = lambda: next(states)
                journey.wait_target = lambda *args, **kwargs: actions.append(("focus", args, kwargs))

                def key(code, modifiers="cmd"):
                    actions.append(("key", code, modifiers))
                    if code == 51:
                        document["activity_view"]["query"] = ""

                def native(command, text):
                    self.assertEqual(list(states), [], "replacement posted before Select All acknowledgement")
                    document["activity_view"]["query"] = text
                    actions.append((command, text))

                journey.key, journey.native = key, native
                journey.wait = lambda description, predicate: self.assertTrue(predicate(document), description)
                with patch("desktop_ui_activity_journeys.time.sleep") as sleep:
                    journey.replace_activity_query(replacement)
                sleep.assert_called_once_with(.05)
                self.assertEqual(actions[1], ("key", 0, "cmd"))
                self.assertEqual(actions[-1], ("text", replacement) if replacement else ("key", 51, "none"))

    def test_selection_timeout_fails_before_posting_replacement(self):
        journey = ActivityJourneys()
        actions = []
        journey.wait_target = lambda *args, **kwargs: None
        journey.key = lambda *args: actions.append(args)
        journey.snapshot = lambda: selected_query(selection=[1, 1])
        journey.native = lambda *args: self.fail("cannot post text without a selection receipt")
        with patch("desktop_ui_activity_journeys.time.monotonic", side_effect=[0, 7]):
            with self.assertRaisesRegex(AssertionError, "acknowledge Select All"):
                journey.replace_activity_query("replacement")
        self.assertEqual(actions, [(0,)])


if __name__ == "__main__":
    unittest.main()
