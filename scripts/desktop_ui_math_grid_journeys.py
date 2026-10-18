"""Real-input math-grid editing: formula fields, exact source patches and Undo."""

import json
from pathlib import Path


def math_grid_fixtures():
    directory = Path(__file__).resolve().parent / "fixtures"
    definitions = {
        "tex": [
            ("array", [[r"\frac{\alpha+1}{n}", "=", "b_n"],
                       [r"\sum_{k=1}^{n} k", "=", r"\frac{n(n+1)}{2}"]],
             r"  \frac{\alpha+1}{n} & = & b_n", r"\frac{\alpha+2}{n}"),
            ("pmatrix", [["x_{11}", "x_{12}"], ["x_{21}", "x_{22}"]],
             "  x_{11} & x_{12}", "x_{11}+1"),
            ("cases", [["u^2", r"\text{if } u \geq 0"],
                       ["-u", r"\text{otherwise}"]],
             r"  u^2 & \text{if } u \geq 0", "u^3"),
        ],
        "typ": [
            ("mat", [["x", "alpha"], ["0", "frac(1, 2)"]],
             "  x, alpha;", "x + 1"),
            ("vec", [["y"], ["beta"], ["0"]], "  y,", "y^2"),
            ("cases", [["u", "u >= 0"], ["-u", "u < 0"]],
             "  u & u >= 0,", "u^2"),
        ],
    }
    fixtures = []
    for extension, definitions_for_language in definitions.items():
        filename = "math-grid-editor." + extension
        source = (directory / filename).read_bytes().decode("utf-8")
        cases = []
        for kind, cells, patch, edited_cell in definitions_for_language:
            if source.count(patch) != 1 or cells[0][0] not in patch \
                    or not cells or not cells[0] or any(len(row) != len(cells[0]) for row in cells):
                raise AssertionError("math-grid fixture needs one literal, rectangular edit: " + kind)
            changed = patch.replace(cells[0][0], edited_cell, 1)
            if changed == patch or source.replace(patch, changed, 1) == source:
                raise AssertionError("math-grid fixture edit must change its formula: " + kind)
            cases.append({"kind": kind, "cells": cells, "edited_cell": edited_cell,
                          "line": source[:source.index(patch)].count("\n"),
                          "applied": source.replace(patch, changed, 1)})
        fixtures.append({"filename": filename, "source": source, "cases": cases})
    return fixtures


def math_grid_fingerprint(cells):
    from desktop_ui_journeys import fingerprint
    return fingerprint(json.dumps(cells, ensure_ascii=False, separators=(",", ":")))


def assert_math_grid(state, case):
    from desktop_ui_newest_journeys import assert_table_grid_only, table_grid_geometry
    assert_table_grid_only(state)
    table_grid_geometry(state, require_overflow=False)
    draft = state["document"]["table_draft"]
    cells = case["cells"]
    if (draft.get("math") is not True or draft.get("kind") != case["kind"]
            or (draft["rows"], draft["columns"]) != (len(cells), len(cells[0]))
            or draft["fingerprint"] != math_grid_fingerprint(cells)):
        raise AssertionError("math-grid metadata/formulas do not match the fixture: " + case["kind"])
    samples = state["document"].get("table_samples")
    if not samples or samples.get("locked") != [] or samples.get("pending") is not False \
            or samples.get("failed") is not False:
        raise AssertionError("ordinary math formulas were incorrectly sampled or locked")


class MathGridJourneys:
    def math_grids(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        view_mode = self.snapshot()["document"]["view_mode"]
        self.key(19)
        if self.snapshot()["document"]["find_visible"]:
            self.click("find.close")
        if self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.Minimize controls")
        for fixture in math_grid_fixtures():
            original = fixture["source"]
            path = self.directory / fixture["filename"]
            path.write_bytes(original.encode("utf-8"))
            count = self.snapshot()["document"]["tabs"]
            self.open_table_fixture(path)
            self.source_is(original)
            for case in fixture["cases"]:
                for disposition in ("cancel", "apply"):
                    old = self.open_table_workbench(line=case["line"])
                    assert_math_grid(self.snapshot(), case)
                    target = "table.cell.0.0"
                    self.reveal_table_cell(target, previous_frame=old.get(target))
                    self.click(target)
                    self.key(0)
                    self.type_table_cell_text(case["edited_cell"])
                    edited = [row.copy() for row in case["cells"]]
                    edited[0][0] = case["edited_cell"]
                    expected_draft = math_grid_fingerprint(edited)
                    self.wait("math field stores the formula without wrappers", lambda d:
                              d.get("table_draft") is not None
                              and d["table_draft"]["fingerprint"] == expected_draft)
                    self.source_is(original)
                    self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(original), frames=3)
                    self.table_target("table.apply")
                    if self.capture_review and disposition == "apply":
                        self.capture_viewport("table-editor")
                    self.click("table." + disposition)
                    self.wait("math-grid draft closes once", lambda d: not d["table_editor_open"])
                    self.source_is(original if disposition == "cancel" else case["applied"])
                    if disposition == "apply":
                        self.click("editor.source")
                        self.key(6)
                        self.source_is(original)
                    self.record("table.math_grid", fixture=fixture["filename"], kind=case["kind"],
                                dimensions=[len(case["cells"]), len(case["cells"][0])],
                                disposition=disposition, exact_undo=disposition == "apply",
                                formulas_editable=True, source_fallback=False)
                if case["kind"] == "mat":
                    self.resize_math_matrix(case, original)
            self.close_scratch(count)
        self.key({"Code": 19, "Split": 20, "Preview": 21}[view_mode])
        self.wait("math-grid journey restores the original layout", lambda d: d["view_mode"] == view_mode)

    def resize_math_matrix(self, case, original):
        from desktop_ui_journeys import fingerprint
        self.open_table_workbench(line=case["line"])
        assert_math_grid(self.snapshot(), case)
        self.table_target("table.rows")
        self.double_click("table.rows")
        self.key(0)
        self.native("text", "3")
        self.key(36, "none")
        enlarged = {**case, "cells": [*case["cells"], ["0", "0"]]}
        expected = math_grid_fingerprint(enlarged["cells"])
        self.wait("matrix row control creates a third zero-filled row", lambda d:
                  d.get("table_draft") is not None and d["table_draft"]["rows"] == 3
                  and d["table_draft"]["columns"] == 2
                  and d["table_draft"]["fingerprint"] == expected)
        assert_math_grid(self.snapshot(), enlarged)
        self.source_is(original)
        self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(original), frames=3)
        before = "  0, frac(1, 2),\n"
        after = "  0, frac(1, 2);\n  0, 0;\n"
        if original.count(before) != 1:
            raise AssertionError("matrix resizing needs one unambiguous original final row")
        self.table_target("table.apply")
        if self.capture_review:
            self.capture_viewport("table-editor")
        self.click("table.apply")
        self.wait("resized math-grid draft closes once", lambda d: not d["table_editor_open"])
        self.source_is(original.replace(before, after, 1))
        self.click("editor.source")
        self.key(6)
        self.source_is(original)
        self.record("table.math_grid_resize", kind="mat", before=[2, 2], after=[3, 2],
                    exact_undo=True, formula_wrappers=False)
