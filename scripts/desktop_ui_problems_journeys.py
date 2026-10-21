"""Problems drill-down checks using real Harper and compiler diagnostics."""

import time


def problems_state(document):
    """Reject incomplete or inconsistent inspection rather than infer empty results."""
    state = document.get("problems")
    if not isinstance(state, dict) or not {"source", "type", "category"} <= state.keys():
        raise AssertionError("Problems inspection is unavailable")
    counts = ("visible_count", "displayed_count", "total_count")
    if any(type(state.get(key)) is not int or state[key] < 0 for key in counts) \
            or not state["displayed_count"] <= state["visible_count"] <= state["total_count"]:
        raise AssertionError("Problems result counts are inconsistent")
    indices = state.get("visible_indices")
    if not isinstance(indices, list) or len(indices) != state["visible_count"] \
            or any(type(index) is not int or not 0 <= index < state["total_count"] for index in indices) \
            or len(set(indices)) != len(indices):
        raise AssertionError("Problems row identities are missing or inconsistent")
    groups = state.get("groups")
    if not isinstance(groups, list) or any(
            not isinstance(group, dict) or not isinstance(group.get("provider"), str)
            or not group["provider"] or group.get("type") not in ("Error", "Warning", "Help", "Note", "Unknown")
            or type(group.get("count")) is not int or group["count"] <= 0
            or type(group.get("open")) is not bool or not isinstance(group.get("target"), str)
            or not group["target"].startswith("problems.group.") for group in groups) \
            or sum(group["count"] for group in groups) != state["visible_count"] \
            or sum(group["count"] for group in groups if group["open"]) != state["displayed_count"]:
        raise AssertionError("Problems group membership is inconsistent")
    if not isinstance(state.get("query"), str) or state.get("source") is not None \
            and not isinstance(state["source"], str) \
            or state.get("type") not in (None, "Error", "Warning", "Help", "Note", "Unknown"):
        raise AssertionError("Problems filter state is invalid")
    if len({group["target"] for group in groups}) != len(groups) \
            or len({(group["provider"], group["type"]) for group in groups}) != len(groups) \
            or any(state["source"] is not None and group["provider"] != state["source"]
                   or state["type"] is not None and group["type"] != state["type"] for group in groups):
        raise AssertionError("Problems groups do not match their selected filters")
    return state


def problems_query_selected(state):
    """Wait for native Select All before posting replacement text."""
    query = state.get("document", {}).get("problems", {}).get("query")
    target = state.get("targets", {}).get("problems.search")
    if not isinstance(query, str) or not isinstance(target, dict):
        return False
    selection, age = target.get("selection"), target.get("age_ms")
    return (target.get("enabled") is True and target.get("focused") is True
            and type(age) in (int, float) and 0 <= age <= 500
            and isinstance(selection, list) and len(selection) == 2
            and all(type(position) is int for position in selection)
            and sorted(selection) == [0, len(query)])


class ProblemsJourneys:
    def replace_problems_query(self, query):
        self.wait_target("problems.search", focused=True)
        self.key(0)
        deadline = time.monotonic() + 6
        while not problems_query_selected(self.snapshot()):
            if time.monotonic() >= deadline:
                raise AssertionError("Problems search did not acknowledge Select All")
            time.sleep(.05)
        if query:
            self.native("text", query)
        else:
            self.key(51, "none")
        self.wait("Problems search receives native typing", lambda d: d["problems"]["query"] == query)

    def problems_filters(self):
        from desktop_ui_journeys import fingerprint
        from desktop_ui_tex_journeys import HARPER_TYPOS
        # Select the backend while the existing document still compiles; the
        # fixture deliberately cannot produce a PDF and must expose an error.
        self.choose_backend("Pdfium", save_pdf=False)
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false')])
        tabs = self.snapshot()["document"]["tabs"]
        source = HARPER_TYPOS + "#problems_fixture_unknown_command\n"
        path = self.directory / "problems-filters.typ"
        path.write_text(source, encoding="utf-8")
        self.open_harper_fixture(path)
        self.key(35, "cmd+alt")
        self.wait("Problems fixture owns preview", lambda d: d["preview_tab_id"] == d["active_tab_id"])
        if self.snapshot()["document"]["panel"] is None:
            self.key(23)
        self.click("panel.problems")

        def unchanged(document):
            return document["path"] == str(path) and document["source_fingerprint"] == fingerprint(source) \
                and document["source_bytes"] == len(source.encode("utf-8"))

        ready = self.wait("Real Harper and compiler results populate Problems", lambda d:
                          unchanged(d) and d["panel"] == "Problems" and d["writing_current"]
                          and not d["writing_pending"] and d["writing_count"] == 2
                          and d["compiler_diagnostic_count"] > 0, timeout=30)["document"]
        baseline = problems_state(ready)
        total = baseline["total_count"]
        if baseline["visible_count"] != total or total < 3 \
                or not any(group["provider"] == "Harper" and group["type"] == "Help"
                           and group["count"] == 2 for group in baseline["groups"]) \
                or not any(group["type"] == "Error" for group in baseline["groups"]):
            raise AssertionError("Problems fixture lacks actual mixed providers and severities")

        def expect(description, *, query="", provider=None, kind=None, visible=None, displayed=None):
            def matches(document):
                if not unchanged(document) or document["find_visible"]:
                    raise AssertionError("Filtering Problems modified source or opened source Find")
                state = problems_state(document)
                return (document["problems_keyboard_focused"]
                        and state["query"] == query and state["source"] == provider and state["type"] == kind
                        and state["total_count"] == total
                        and (visible is None or state["visible_count"] == visible)
                        and (displayed is None or state["displayed_count"] == displayed))
            state = self.wait(description, matches)
            self.stable_state(matches, frames=4)
            return state["document"]["problems"]

        self.click("problems.source")
        self.click("problems.source.Harper")
        expect("Provider filter keeps only Harper", provider="Harper", visible=2)
        self.click("problems.type")
        self.click("problems.type.Help")
        expect("Provider and type filters combine", provider="Harper", kind="Help", visible=2)
        self.key(3)
        self.replace_problems_query("MISSPELLD")
        filtered = expect("Case-insensitive search combines with provider and type", query="MISSPELLD",
                          provider="Harper", kind="Help", visible=1, displayed=1)
        group = filtered["groups"][0]
        self.click(group["target"])
        expect("Collapsing a group keeps its filtered count", query="MISSPELLD", provider="Harper",
               kind="Help", visible=1, displayed=0)
        self.click(group["target"])
        expect("Expanding restores the filtered row", query="MISSPELLD", provider="Harper",
               kind="Help", visible=1, displayed=1)
        self.key(3)
        self.replace_problems_query("no-problem-can-match-this")
        expect("An unmatched search shows an empty result", query="no-problem-can-match-this",
               provider="Harper", kind="Help", visible=0, displayed=0)
        self.click("problems.reset")
        expect("Reset clears all filters together", visible=total, displayed=total)
        self.click("problems.type")
        self.click("problems.type.Error")
        errors = expect("Error filter excludes spelling help", kind="Error")
        if not errors["groups"] or any(group["type"] != "Error" or group["provider"] == "Harper"
                                      for group in errors["groups"]):
            raise AssertionError("Type filter admitted non-error results")
        self.click("problems.reset")
        expect("All results restored before navigation", visible=total, displayed=total)
        self.double_click("problem." + str(filtered["visible_indices"][0]))
        self.wait("Filtered row retains original source navigation", lambda d:
                  unchanged(d) and d["tabs"] == tabs + 1 and d["editor_keyboard_focused"])
        if path.read_text(encoding="utf-8") != source:
            raise AssertionError("Filtering or navigating Problems changed the source on disk")
        self.record("problems.filters", total=total, providers=[group["provider"] for group in baseline["groups"]],
                    filtered_indices=filtered["visible_indices"])
        if self.capture_review:
            self.capture_viewport("main")
        self.click("panel.close")
        self.click("editor.source")
        self.close_scratch(tabs)
        self.tex_settings([('"english_grammar": true', '"english_grammar": false'),
                           ('"auto_save": false', '"auto_save": true')])
