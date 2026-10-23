"""Activity drill-down through real controls and read-only source observations."""

import math
import time

ACTIVITY_JOURNEYS = ("activity",)
GROUP_LABELS = {"Preview": "Build & preview", "Code": "Code", "Checks": "Checks",
                "Files": "Files", "Git": "Git", "Tools": "Tools"}
GROUPS = set(GROUP_LABELS)
STATES = {"Working", "Waiting", "Failed", "Idle", "Inactive"}


def matches_state(actual, selected):
    return selected is None or actual == selected \
        or selected == "Active" and actual in {"Working", "Waiting", "Failed"}


def activity_state(document):
    """Require complete membership evidence; absent inspection cannot pass as empty."""
    state = document.get("activity_view")
    if not isinstance(state, dict) or not {"query", "group", "status"} <= state.keys():
        raise AssertionError("Activity inspection is unavailable")
    if not isinstance(state["query"], str) \
            or not (state["group"] is None or isinstance(state["group"], str) and state["group"] in GROUPS) \
            or not (state["status"] is None or isinstance(state["status"], str) \
                    and state["status"] in STATES | {"Active"}):
        raise AssertionError("Activity filters are invalid")
    for flag in ("grouped", "controls_visible", "sticky_filters"):
        if type(state.get(flag)) is not bool:
            raise AssertionError("Activity compact layout inspection is unavailable")
    if state["sticky_filters"] and not state["controls_visible"]:
        raise AssertionError("Active Activity filters must remain visible")
    if any(type(state.get(key)) is not int or state[key] < 0
           for key in ("total_count", "visible_count", "displayed_count")) \
            or not state["displayed_count"] <= state["visible_count"] <= state["total_count"]:
        raise AssertionError("Activity result counts are inconsistent")
    rows = state.get("rows")
    if not isinstance(rows, list) or len(rows) != state["total_count"] or any(
            not isinstance(row, dict) or not isinstance(row.get("name"), str) or not row["name"]
            or not isinstance(row.get("group"), str) or row["group"] not in GROUPS \
            or not isinstance(row.get("state"), str) or row["state"] not in STATES
            or not isinstance(row.get("detail"), str) for row in rows) \
            or len({row["name"] for row in rows}) != len(rows):
        raise AssertionError("Activity source inventory is incomplete or duplicated")
    by_name = {row["name"]: row for row in rows}
    visible = state.get("visible_names")
    if not isinstance(visible, list) or len(visible) != state["visible_count"] \
            or any(not isinstance(name, str) or name not in by_name for name in visible) \
            or len(set(visible)) != len(visible):
        raise AssertionError("Activity visible source identities are inconsistent")
    if any(state["group"] is not None and by_name[name]["group"] != state["group"]
           or not matches_state(by_name[name]["state"], state["status"]) for name in visible):
        raise AssertionError("Activity rows do not match the selected filters")
    needle = state["query"].strip().lower()
    def query_matches(row):
        # The inspector's failure detail prefixes its reason with the state.
        # Search them as separate fields, as the actual model does, rather
        # than accidentally matching across a synthetic field boundary.
        detail = row["detail"]
        if row["state"] == "Failed":
            detail = detail.removeprefix("Failed ")
        fields = (row["name"], GROUP_LABELS[row["group"]], row["state"], detail)
        return not needle or any(needle in value.lower() for value in fields)
    expected = [row["name"] for row in rows
                if (state["group"] is None or row["group"] == state["group"])
                and matches_state(row["state"], state["status"]) and query_matches(row)]
    if set(visible) != set(expected):
        raise AssertionError("Activity filters lost or admitted source rows")
    groups = state.get("groups")
    if not isinstance(groups, list) or any(
            not isinstance(group, dict) or not isinstance(group.get("id"), str) or group["id"] not in GROUPS
            or not isinstance(group.get("label"), str) or not group["label"]
            or type(group.get("count")) is not int or group["count"] <= 0
            or type(group.get("open")) is not bool
            or group.get("target") != "activity.group." + group["id"] for group in groups) \
            or len({group["id"] for group in groups}) != len(groups) \
            or sum(group["count"] for group in groups) != state["visible_count"]:
        raise AssertionError("Activity group membership is inconsistent")
    expected_groups = {by_name[name]["group"] for name in visible}
    if {group["id"] for group in groups} != expected_groups or any(
            group["count"] != sum(by_name[name]["group"] == group["id"] for name in visible)
            for group in groups):
        raise AssertionError("Activity group counts do not match their source identities")
    displayed = sum(group["count"] for group in groups if group["open"]) \
        if state["grouped"] else state["visible_count"]
    if state["displayed_count"] != displayed:
        raise AssertionError("Activity collapsed groups disagree with displayed sources")
    return state


def activity_scroll(state, *, sticky=None):
    document = state.get("document", {})
    layout = activity_state(document)
    scroll = state.get("scrolls", {}).get("activity")
    if not isinstance(scroll, dict) or scroll.get("frame") != document.get("frame") \
            or any(type(scroll.get(key)) not in (int, float) or not math.isfinite(scroll[key])
                   for key in ("x", "y", "offset", "content_height", "viewport_height")) \
            or scroll["offset"] < 0 or scroll["content_height"] <= 0 or scroll["viewport_height"] <= 0:
        raise AssertionError("Activity scroll geometry is unavailable or stale")
    if type(scroll.get("sticky_search")) is not bool \
            or scroll["sticky_search"] != layout["sticky_filters"] \
            or sticky is not None and scroll["sticky_search"] is not sticky:
        raise AssertionError("Activity sticky filter state disagrees with scroll geometry")
    return scroll


def activity_query_selected(state):
    query = state.get("document", {}).get("activity_view", {}).get("query")
    target = state.get("targets", {}).get("activity.search")
    if not isinstance(query, str) or not isinstance(target, dict):
        return False
    selection, age = target.get("selection"), target.get("age_ms")
    return (target.get("enabled") is True and target.get("focused") is True
            and type(age) in (int, float) and 0 <= age <= 500
            and isinstance(selection, list) and len(selection) == 2
            and all(type(position) is int for position in selection)
            and sorted(selection) == [0, len(query)])


def assert_activity_preserves_source(before, document, *, focused=True):
    fields = ("path", "source_fingerprint", "source_bytes", "cursor", "tabs", "active_tab_id")
    if any(field not in before or field not in document or before[field] != document[field]
           for field in fields):
        raise AssertionError("Filtering Activity navigated or changed the source document")
    if document.get("find_visible") is not False:
        raise AssertionError("Activity Find opened source Find")
    if focused and (document.get("activity_keyboard_focused") is not True
                    or document.get("editor_keyboard_focused") is not False):
        raise AssertionError("Activity did not retain keyboard ownership")
    for owner in (before, document):
        rect = owner.get("panel_rect")
        if not isinstance(rect, list) or len(rect) != 4 \
                or any(type(value) not in (int, float) or not math.isfinite(value) for value in rect):
            raise AssertionError("Activity panel geometry is unavailable")
    if abs((before["panel_rect"][3] - before["panel_rect"][1])
           - (document["panel_rect"][3] - document["panel_rect"][1])) > .5:
        raise AssertionError("Activity filters changed the user's panel height")
    return True


class ActivityJourneys:
    def scroll_activity(self, amount):
        scroll = activity_scroll(self.snapshot())
        self.native("move", scroll["x"], scroll["y"])
        self.native("wheel", scroll["x"], scroll["y"], amount)
        return scroll

    def replace_activity_query(self, query):
        self.wait_target("activity.search", focused=True)
        self.key(0)
        deadline = time.monotonic() + 6
        while not activity_query_selected(self.snapshot()):
            if time.monotonic() >= deadline:
                raise AssertionError("Activity search did not acknowledge Select All")
            time.sleep(.05)
        if query:
            self.native("text", query)
        else:
            self.key(51, "none")
        self.wait("Activity search receives native typing", lambda d: d["activity_view"]["query"] == query)

    def activity(self):
        if self.snapshot()["document"]["panel"] is None:
            self.key(23)
        self.click("panel.activity")
        ready = self.wait("Live Activity inventory is grouped and compact", lambda d:
                          d["panel"] == "Activity" and d["activity_keyboard_focused"]
                          and d["activity_view"]["grouped"]
                          and not d["activity_view"]["controls_visible"]
                          and not d["activity_view"]["sticky_filters"])["document"]
        initial = activity_state(ready)
        if initial["total_count"] < 20 or len(initial["groups"]) < 4:
            raise AssertionError("Activity fixture lacks the actual app source inventory")
        total = initial["total_count"]
        git_rows = [row for row in initial["rows"] if row["group"] == "Git"]
        if len(git_rows) < 2 or not any(row["name"] == "Git status" for row in git_rows):
            raise AssertionError("Activity cannot demonstrate grouping of related Git work")

        def expect(description, *, query="", group=None, status=None, names=None,
                   grouped=True, displayed=None):
            def matches(document):
                assert_activity_preserves_source(ready, document)
                state = activity_state(document)
                return (document["panel"] == "Activity" and state["query"] == query
                        and state["group"] == group and state["status"] == status
                        and state["grouped"] is grouped and state["total_count"] == total
                        and (names is None or set(state["visible_names"]) == set(names))
                        and (displayed is None or state["displayed_count"] == displayed))
            result = self.wait(description, matches)
            self.stable_state(matches, frames=4)
            return result["document"]["activity_view"]

        if self.capture_review:
            self.capture_viewport("main")
        self.scroll_activity(100)
        self.wait("Scrolling above Activity reveals its inactive compact filters", lambda d:
                  d["activity_view"]["controls_visible"] and not d["activity_view"]["sticky_filters"])
        self.wait_target("activity.search")
        self.scroll_activity(-100)
        self.wait("Empty Activity filters scroll out of the way", lambda d:
                  not d["activity_view"]["controls_visible"])
        offset = activity_scroll(self.snapshot(), sticky=False)["offset"]
        for _ in range(6):
            if abs(activity_scroll(self.snapshot(), sticky=False)["offset"] - offset) > .5:
                raise AssertionError("Activity scrolling fought back after the wheel stopped")

        self.key(3)
        self.wait_target("activity.search", focused=True)
        expect("Cmd+F reveals Activity search without opening source Find")
        self.click("activity.group-filter")
        self.click("activity.group-option.Git")
        filtered = expect("Group filter keeps related Git work", group="Git",
                          names=[row["name"] for row in git_rows])
        if not filtered["sticky_filters"] or not filtered["controls_visible"]:
            raise AssertionError("An active Activity filter did not stay visible")
        self.key(3)
        self.replace_activity_query("GIT STATUS")
        filtered = expect("Case-insensitive Activity search combines with the group", query="GIT STATUS",
                          group="Git", names=["Git status"], displayed=1)
        settled = self.wait("Git status settles before filtering its concrete state", lambda d:
                            any(row["name"] == "Git status" and row["state"] in {"Idle", "Inactive", "Failed"}
                                for row in activity_state(d)["rows"]))["document"]
        selected = next(row["state"] for row in activity_state(settled)["rows"] if row["name"] == "Git status")
        self.click("activity.state-filter")
        self.click("activity.state-option." + selected)
        expect("Group, state, and query combine without losing source identities", query="GIT STATUS",
               group="Git", status=selected, names=["Git status"], displayed=1)
        self.click("activity.group.Git")
        expect("A collapsed related group retains its filtered count", query="GIT STATUS",
               group="Git", status=selected, names=["Git status"], displayed=0)
        self.click("activity.group.Git")
        expect("Expanding the group restores its source", query="GIT STATUS",
               group="Git", status=selected, names=["Git status"], displayed=1)
        self.click("activity.grouping")
        expect("Compact flat mode preserves the selected filters", query="GIT STATUS",
               group="Git", status=selected, grouped=False, names=["Git status"], displayed=1)
        self.key(3)
        self.replace_activity_query("no-activity-can-match-this")
        expect("An unmatched Activity query has an honest empty result", query="no-activity-can-match-this",
               group="Git", status=selected, grouped=False, names=[], displayed=0)
        self.click("panel.problems")
        self.wait("Problems receives focus from Activity", lambda d:
                  d["panel"] == "Problems" and d["problems_keyboard_focused"]
                  and not d["activity_keyboard_focused"])
        self.click("panel.activity")
        expect("Returning to Activity preserves its filters and compact mode", query="no-activity-can-match-this",
               group="Git", status=selected, grouped=False, names=[], displayed=0)
        self.click("activity.reset")
        expect("Reset clears the Activity filters together", grouped=False,
               names=[row["name"] for row in initial["rows"]], displayed=total)
        self.click("activity.grouping")
        expect("Related grouping restores without changing the document", displayed=total)
        if self.capture_review:
            self.capture_viewport("main")
        self.record("activity.filters", total=total, groups=[group["id"] for group in initial["groups"]],
                    source_preserved=True)
        self.click("panel.close")
        self.wait("Closing Activity releases its keyboard", lambda d:
                  d["panel"] is None and not d["activity_keyboard_focused"])
