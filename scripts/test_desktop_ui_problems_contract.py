"""Problems observations must prove actual filtered membership and keyboard focus."""

from copy import deepcopy
import unittest
from unittest.mock import patch

from desktop_ui_problems_journeys import ProblemsJourneys, problems_query_selected, problems_state


def inspected_problems():
    return {"query": "", "source": None, "type": None, "category": None,
            "visible_count": 3, "displayed_count": 3, "total_count": 3,
            "visible_indices": [0, 1, 2], "groups": [
                {"provider": "Typst", "type": "Error", "count": 1, "open": True,
                 "target": "problems.group.Typst.Error"},
                {"provider": "Harper", "type": "Help", "count": 2, "open": True,
                 "target": "problems.group.Harper.Help"}]}


def selected_query(query="misspelld", selection=None):
    return {"document": {"problems": {"query": query}}, "targets": {"problems.search": {
        "enabled": True, "focused": True, "age_ms": 0,
        "selection": [0, len(query)] if selection is None else selection}}}


class ProblemsContracts(unittest.TestCase):
    def test_group_collapse_preserves_filtered_count_and_original_row_identity(self):
        state = inspected_problems()
        self.assertIs(problems_state({"problems": state}), state)
        state.update(query="misspelld", source="Harper", type="Help", visible_count=1,
                     displayed_count=0, visible_indices=[2], groups=[state["groups"][1]])
        state["groups"][0].update(count=1, open=False)
        self.assertIs(problems_state({"problems": state}), state)
        state.update(visible_count=0, displayed_count=0, visible_indices=[], groups=[])
        self.assertEqual(problems_state({"problems": state})["total_count"], 3)

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
                problems_state({"problems": state})

    def test_group_membership_and_expansion_must_agree_with_displayed_rows(self):
        for field, value in [("count", 1), ("open", False), ("open", 1), ("provider", ""),
                             ("type", "unsupported"), ("target", "problem.2")]:
            state = inspected_problems()
            state["groups"][1][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                problems_state({"problems": state})

    def test_selected_provider_and_type_must_restrict_every_group(self):
        for field, value in [("source", "Harper"), ("type", "Help")]:
            state = inspected_problems()
            state[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "selected filters"):
                problems_state({"problems": state})
        state = inspected_problems()
        state["groups"][1]["target"] = state["groups"][0]["target"]
        with self.assertRaisesRegex(AssertionError, "selected filters"):
            problems_state({"problems": state})
        del state["source"]
        with self.assertRaisesRegex(AssertionError, "unavailable"):
            problems_state({"problems": state})

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
