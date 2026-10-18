"""Real input regressions for focus, preview controls and table draft lifetimes."""

import json
import math
from pathlib import Path
import shlex
import time

NEWEST_JOURNEYS = ("problems_focus", "preview_find", "tables", "arxiv_tables", "math_grids")


def arxiv_table_cases():
    """Use the attributed excerpts unchanged; keep native edit expectations literal."""
    directory = Path(__file__).resolve().parents[1] / "tests/fixtures/arxiv-tables"
    cells = {"scipy-roadmap.tex": (1, 1), "scipy-optimizers.tex": (1, 0),
             "platonic-metrics.tex": (3, 5)}
    dimensions = {"scipy-roadmap.tex": (16, 2), "scipy-optimizers.tex": (13, 14),
                  "platonic-metrics.tex": (10, 6)}
    cell_sources = {"platonic-metrics.tex": (
        r"{\vspace{-0.5em}Unbiased estimator of CKA that corrects for sample bias in "
        r"HSIC~\cite{song2012feature}.}")}
    editor_sources = {"platonic-metrics.tex": (
        r"Unbiased estimator of CKA that corrects for sample bias in "
        r"HSIC~\cite{song2012feature}.")}
    metadata = json.loads((directory / "provenance.json").read_text())
    if len(metadata) != len(cells) or {entry["file"] for entry in metadata} != set(cells):
        raise AssertionError("native arXiv table fixture inventory changed")
    cases = []
    for entry in metadata:
        source = (directory / entry["file"]).read_bytes().decode("utf-8")
        opening, closing = r"\begin{tabular}", r"\end{tabular}"
        if source.count(opening) != 1 or source.count(closing) != 1:
            raise AssertionError("native arXiv excerpt needs one complete table: " + entry["file"])
        start, end = source.index(opening), source.index(closing) + len(closing)
        environment = source[start:end]
        before, after = entry["edit_before"], entry["edit_after"]
        if not before or before == after or source.count(before) != 1 or environment.count(before) != 1:
            raise AssertionError("native arXiv edit must name one literal inside its table: " + entry["file"])
        cell_source = cell_sources.get(entry["file"], before)
        if environment.count(cell_source) != 1 or cell_source.count(before) != 1:
            raise AssertionError("native arXiv edit must preserve the complete cell: " + entry["file"])
        editor_text = editor_sources.get(entry["file"], cell_source)
        if cell_source.count(editor_text) != 1 or editor_text.count(before) != 1:
            raise AssertionError("native arXiv editor body must preserve its source wrappers: " + entry["file"])
        cases.append({"file": entry["file"], "source": source, "table": environment,
                      "line": source[:start].count("\n") + 1, "cell": cells[entry["file"]],
                      "dimensions": dimensions[entry["file"]],
                      "cell_source": cell_source, "editor_text": editor_text,
                      "edited_editor_text": editor_text.replace(before, after, 1),
                      "edit_before": before, "edit_after": after})
    return cases


def assert_preview_drag_position(document, baseline, delta, tolerance=3):
    actual = document.get("preview_controls_position")
    vectors = (actual, baseline, delta)
    if any(not value or len(value) != 2 or not all(math.isfinite(x) for x in value)
           for value in vectors):
        raise AssertionError("invalid preview controls drag geometry")
    if any(abs(actual[axis] - baseline[axis] - delta[axis]) > tolerance for axis in (0, 1)):
        raise AssertionError("preview controls drifted from the held pointer")


TABLE_WINDOW_TITLE = "tiptoptyp Table editor"


def table_native_window(windows, requested_size=None):
    matches = [window for window in windows if window.get("AXTitle") == TABLE_WINDOW_TITLE]
    if len(matches) != 1 or matches[0].get("AXFocused") is not True \
            or matches[0].get("AXMinimized") is True:
        return None
    window = matches[0]
    size = window.get("size")
    if not isinstance(size, list) or len(size) != 2 or any(
            type(value) not in (int, float) or not math.isfinite(value) or value <= 0
            for value in size):
        return None
    if requested_size is not None:
        width, height = requested_size
        # AX sizes include native window chrome; the inner minimum can add a
        # titlebar to the requested height when AppKit clamps the outer frame.
        if abs(size[0] - width) > 2 or not height - 2 <= size[1] <= height + 40:
            return None
    return window


def table_parent_sizes(windows):
    return {window["AXTitle"]: window.get("size") for window in windows
            if window.get("AXTitle") and window["AXTitle"] != TABLE_WINDOW_TITLE}


def table_grid_geometry(state, baseline_footer=None, *, require_overflow=True):
    """Require a current child grid/footer, optionally with the compact stress overflow."""
    document = state["document"]
    scroll = state.get("scrolls", {}).get("table.grid")
    targets = state.get("targets", {})
    apply, cancel = targets.get("table.apply"), targets.get("table.cancel")
    if not document.get("table_editor_open") or not scroll or not apply or not cancel:
        raise AssertionError("table grid/footer inspection is unavailable")
    for button in (apply, cancel):
        if not button.get("enabled") or button.get("age_ms", 501) > 500 \
                or button.get("frame") != scroll.get("frame") \
                or button.get("viewport") != apply.get("viewport"):
            raise AssertionError("table footer belongs to a stale or different child frame")
        if any(type(button.get(key)) not in (int, float) or not math.isfinite(button[key])
               for key in ("x", "y", "width", "height")) \
                or button["width"] <= 0 or button["height"] <= 0:
            raise AssertionError("table footer geometry is invalid")
    if any(type(scroll.get(key)) not in (int, float) or not math.isfinite(scroll[key])
           for key in ("x", "y", "offset", "content_height", "viewport_height")) \
            or scroll["offset"] < 0 or scroll["viewport_height"] <= 0 or scroll["content_height"] <= 0 \
            or (require_overflow and scroll["content_height"] <= scroll["viewport_height"] + 100):
        raise AssertionError("table fixture did not expose its scrollable grid")
    scale = document.get("interface_scale")
    if type(scale) not in (int, float) or not math.isfinite(scale) or scale <= 0:
        raise AssertionError("table interface scale is unavailable")
    grid_bottom = scroll["y"] + scroll["viewport_height"] * scale / 2
    if any(button["y"] - button["height"] / 2 < grid_bottom - 2 for button in (apply, cancel)):
        raise AssertionError("table footer overlaps the scrollable grid")
    footer = {name: tuple(button[key] for key in ("x", "y", "width", "height"))
              for name, button in (("apply", apply), ("cancel", cancel))}
    if baseline_footer is not None and any(
            abs(value - original) > 1
            for name, geometry in footer.items()
            for value, original in zip(geometry, baseline_footer[name])):
        raise AssertionError("table footer moved while the grid scrolled")
    return scroll, footer


def assert_table_grid_only(state, *, valid=True):
    """Require this child frame to expose its grid, never a source fallback."""
    document, targets = state["document"], state.get("targets", {})
    apply = targets.get("table.apply")
    if not document.get("table_editor_open") or not apply or apply.get("age_ms", 501) > 500 \
            or apply.get("enabled") is not valid:
        raise AssertionError("table grid has no current validation/footer state")
    draft = document.get("table_draft")
    if not draft or draft.get("valid") is not valid \
            or any(type(draft.get(axis)) is not int or draft[axis] <= 0
                   for axis in ("rows", "columns")) \
            or draft["rows"] * draft["columns"] > 4096:
        raise AssertionError("table grid has no bounded, validated cell metadata")
    for name in ("table.source", "table.source-mode"):
        target = targets.get(name)
        if target and target.get("frame") == apply.get("frame") \
                and target.get("viewport") == apply.get("viewport") \
                and target.get("age_ms", 501) <= 500:
            raise AssertionError("table workbench exposed a source editor: " + name)
    scroll = state.get("scrolls", {}).get("table.grid")
    if not scroll or scroll.get("frame") != apply.get("frame"):
        raise AssertionError("table workbench has no grid in its current child frame")


def visible_table_cell(state, name, previous_frame=None):
    """A partially clipped sliver or an old offscreen target is not clickable evidence."""
    targets = state.get("targets", {})
    target, footer = targets.get(name), targets.get("table.apply")
    grid = state.get("scrolls", {}).get("table.grid")
    if not state["document"].get("table_editor_open") or not target or not footer or not grid \
            or target.get("enabled") is not True or target.get("age_ms", 501) > 500 \
            or footer.get("enabled") is not True or footer.get("age_ms", 501) > 500 \
            or target.get("frame") != footer.get("frame") \
            or target.get("frame") != grid.get("frame") \
            or target.get("viewport") != footer.get("viewport") \
            or (previous_frame is not None and target.get("frame") == previous_frame):
        return None
    if any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
           for key in ("x", "y", "width", "height")) \
            or target["width"] < 40 or target["height"] < 8:
        return None
    return target


def table_offer_rejected(state, previous_menu_frame=None):
    """Only the newly opened source context menu can prove unsupported detection."""
    if state["document"].get("table_editor_open"):
        raise AssertionError("unsupported table unexpectedly opened a workbench")
    targets = state.get("targets", {})
    marker = targets.get("menu.Select All")
    if not marker or not marker.get("enabled") or marker.get("age_ms", 501) > 500 \
            or (previous_menu_frame is not None and marker.get("frame") == previous_menu_frame):
        return False
    table = targets.get("menu.Edit Table…")
    if table and table.get("frame") == marker.get("frame") \
            and table.get("viewport") == marker.get("viewport") \
            and table.get("age_ms", 501) <= 500:
        raise AssertionError("unsupported table was offered as Edit Table")
    return True


def assert_compact_table_rows(state):
    targets = state.get("targets", {})
    rows = [targets.get("table.cell.1.0"), targets.get("table.cell.2.0")]
    if any(not row or row.get("age_ms", 501) > 500 for row in rows) \
            or rows[0]["frame"] != rows[1]["frame"] \
            or rows[0]["viewport"] != rows[1]["viewport"]:
        raise AssertionError("compact table rows need fresh geometry from one child")
    scale = state["document"]["interface_scale"]
    pitch = (rows[1]["y"] - rows[0]["y"]) / scale
    if not 32 <= pitch <= 40:
        raise AssertionError("table row spacing is not compact: " + str(pitch))
    return pitch


class NewestJourneys:
    def newest_preview_document(self):
        self.focus_document_window()
        self.ensure_typesetting_tab()
        state = self.snapshot()["document"]
        if not (state["path"] or "").endswith(".typ"):
            raise AssertionError("newest preview journeys need their named Typst fixture")
        if state["preview_popout"]:
            self.native("close", "tiptoptyp Preview")
            self.wait("previous popout restored", lambda d: not d["preview_popout"])
        if state["find_visible"]:
            self.click("find.close")
            self.wait("previous source Find closes", lambda d: not d["find_visible"])
        self.key(20)  # Split, with an explicit pinned source.
        self.click("editor.source")
        self.key(35, "cmd+alt")
        self.wait("Typst source owns preview", lambda d: d["preview_tab_id"] == d["active_tab_id"])

    def problems_focus(self):
        self.newest_preview_document()
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            if self.snapshot()["document"]["preview_controls_open"]:
                self.click("preview.Minimize controls")
            # Cmd+5 reopens the remembered tab. Terminal handoff leaves that
            # tab as Terminal, so prepare Problems before testing the shortcut
            # from preview focus; a compensating click afterward would mask it.
            if self.snapshot()["document"]["panel"] is None:
                self.key(23)
                self.wait("bottom panel opens for Problems preparation", lambda d: d["panel"] is not None)
            if self.snapshot()["document"]["panel"] != "Problems":
                self.click("panel.problems")
                self.wait("Problems is the remembered panel tab", lambda d: d["panel"] == "Problems")
            self.click("panel.close")
            self.wait("prepared Problems closes before preview focus", lambda d: d["panel"] is None)
            self.click("preview.page")
            self.wait("preview owns keyboard before Problems", lambda d: d["preview_keyboard_focused"])
            original = self.snapshot()["document"]
            self.key(23)
            self.wait("Problems claims keyboard from preview", lambda d: d["panel"] == "Problems"
                      and d["problems_keyboard_focused"] and not d["preview_keyboard_focused"]
                      and not d["editor_keyboard_focused"] and d["explorer_focused"] is None)
            self.click("problems.input")
            self.wait("Problems blank area owns keyboard", lambda d: d["problems_keyboard_focused"])

            def unchanged(document):
                return document["source_fingerprint"] == original["source_fingerprint"] \
                    and document["source_bytes"] == original["source_bytes"] \
                    and document["page_dark"] == original["page_dark"]

            def source_selection_unchanged(document):
                return unchanged(document) and document["cursor"] == original["cursor"]

            self.native("text", "t")
            self.key(17, "none")  # A physical preview-invert key, with no modifiers.
            for code, modifiers in [(0, "cmd"), (44, "cmd"), (6, "cmd"), (6, "cmd+shift")]:
                self.key(code, modifiers)
                self.stable_state(source_selection_unchanged, frames=4)
            self.paste_source("must stay out of source", check=lambda: self.stable_state(source_selection_unchanged))
            self.key(3)
            self.wait("Problems Find does not open a source or preview Find", lambda d:
                      d["problems_keyboard_focused"] and not d["find_visible"]
                      and not d["preview_controls_open"] and unchanged(d))
            if getattr(self, "capture_review", False) and backend == "Pdfium":
                self.capture_viewport("main")
            self.click("panel.activity")
            self.wait("Activity owns the bottom panel keyboard", lambda d:
                      d["panel"] == "Activity" and d["bottom_panel_keyboard_focused"]
                      and not d["problems_keyboard_focused"] and not d["editor_keyboard_focused"]
                      and not d["preview_keyboard_focused"])
            self.native("text", "t")
            self.stable_state(source_selection_unchanged, frames=4)
            if getattr(self, "capture_review", False) and backend == "Pdfium":
                self.capture_viewport("main")
            self.click("panel.problems")
            self.wait("Problems header reclaims keyboard", lambda d: d["problems_keyboard_focused"])
            self.click("panel.close")
            self.wait("hidden Problems cannot retain keyboard", lambda d: d["panel"] is None
                      and not d["problems_keyboard_focused"] and unchanged(d))
            self.click("editor.source")
            self.wait("source can reclaim keyboard", lambda d: d["editor_keyboard_focused"])
            self.native("text", "t")
            self.wait("source typing resumes after Problems", lambda d:
                      d["source_fingerprint"] != original["source_fingerprint"])
            self.key(6)
            self.wait("source fixture restored once", unchanged)
            self.preview_terminal_handoff(backend, unchanged)

    def preview_terminal_handoff(self, backend, unchanged):
        # Off remains off when its panel reopens. Prepare a fresh isolated pane
        # before focusing the preview, including on the second backend run.
        if self.snapshot()["document"]["terminal_off"]:
            self.key(50, "ctrl")
            self.wait("off Terminal opens for preparation", lambda d: d["panel"] == "Terminal")
            self.click("terminal.new")
            self.wait("isolated terminal pane prepared", lambda d: not d["terminal_off"] and unchanged(d))
            self.click("panel.close")
            self.wait("prepared terminal panel closes", lambda d: d["panel"] is None)
        # The shortcut itself must claim native keyboard input: do not click
        # the terminal grid to compensate for a failed WebView handoff.
        self.click("preview.page")
        self.wait("preview owns keyboard before Terminal", lambda d: d["preview_keyboard_focused"])
        self.key(50, "ctrl")
        self.wait("Terminal shortcut claims keyboard from preview", lambda d:
                  d["panel"] == "Terminal" and d["terminal_keyboard_focused"]
                  and not d["preview_keyboard_focused"] and not d["editor_keyboard_focused"]
                  and d["explorer_focused"] is None and unchanged(d))
        self.wait_target("terminal.input")
        if getattr(self, "capture_review", False) and backend == "Pdfium":
            self.capture_viewport("main")
        self.key(17, "none")  # A physical 't' must reach the shell, not invert the preview.
        self.stable_state(unchanged, frames=4)
        self.key(32, "ctrl")  # Clear the shell input before writing an isolated receipt.
        receipt = self.directory / f"preview-terminal-{backend}.receipt"
        self.native("text", f"printf '%s' tiptoptyp > {shlex.quote(str(receipt))}")
        self.key(36, "none")
        self.wait("native typing reaches the terminal shell", lambda d:
                  receipt.exists() and receipt.read_text() == "tiptoptyp" and unchanged(d))
        self.click("terminal.off")
        self.wait("terminal shell stops after focus check", lambda d: d["terminal_off"] and unchanged(d))
        self.click("panel.close")
        self.wait("terminal panel releases keyboard on close", lambda d:
                  d["panel"] is None and not d["terminal_keyboard_focused"] and unchanged(d))
        self.record("preview.terminal_handoff", backend=backend)

    def stationary_preview_drag(self):
        self.wait_target("preview.drag_handle")
        state = self.snapshot()
        handle = state["targets"]["preview.drag_handle"]
        before = state["document"]["preview_controls_position"]
        local = state["document"]["preview_controls_local_position"]
        size = state["document"]["preview_controls_size"]
        owner_size = state["document"]["native_size"]
        scale = state["document"]["interface_scale"]
        if not before or not local or not size or not owner_size or not math.isfinite(scale) or scale <= 0:
            raise AssertionError("missing preview drag owner geometry")
        delta = [24 if local[0] < (owner_size[0] / scale - size[0]) / 2 else -24,
                 12 if local[1] < (owner_size[1] / scale - size[1]) / 2 else -12]
        end = [handle["x"] + delta[0], handle["y"] + delta[1]]
        try:
            self.native("drag-start", handle["x"], handle["y"])
            for step in range(1, 5):
                self.native("drag-move", handle["x"] + delta[0] * step / 4,
                            handle["y"] + delta[1] * step / 4)
            self.wait("header follows the complete drag distance", lambda d:
                      d["preview_controls_position"] is not None
                      and all(abs(d["preview_controls_position"][axis] - before[axis] - delta[axis]) <= 3
                              for axis in (0, 1)))
            if self.native("pointer-state").get("left") is not True:
                raise AssertionError("stationary-drag assertion ran after mouse release")
            deadline = time.monotonic() + 0.4
            frames = 0
            while time.monotonic() < deadline or frames < 8:
                assert_preview_drag_position(self.snapshot()["document"], before, delta)
                frames += 1
                time.sleep(0.02)
            if self.native("pointer-state").get("left") is not True:
                raise AssertionError("mouse button was released during stationary-drag frames")
            self.record("preview.drag.stationary", frames=frames, delta=delta, held=True)
        finally:
            # A missing receipt can still follow a posted mouse-down. Always
            # release the held button before propagating an assertion or error.
            self.native("drag-end", *end)
        if self.native("pointer-state").get("left"):
            raise AssertionError("drag cleanup left the mouse button pressed")
        for _ in range(8):
            assert_preview_drag_position(self.snapshot()["document"], before, delta)

    def wait_preview_query_focus(self):
        deadline = time.monotonic() + 6
        first = self.snapshot()["targets"].get("preview.query")
        previous_frame = first["frame"] if first else -1
        while time.monotonic() < deadline:
            state = self.snapshot()
            target = state["targets"].get("preview.query")
            if target and target["enabled"] and target["age_ms"] <= 500 \
                    and target["frame"] > previous_frame and target.get("focused") \
                    and not state["document"]["find_visible"]:
                return
            time.sleep(0.05)
        raise AssertionError("Cmd+F did not focus the preview query")

    def preview_find(self):
        from desktop_ui_journeys import fingerprint
        self.newest_preview_document()
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            if self.snapshot()["document"]["preview_controls_open"]:
                self.click("preview.Minimize controls")
            self.click("preview.page")
            self.wait("preview owns keyboard for Find", lambda d: d["preview_keyboard_focused"])
            original = self.snapshot()["document"]
            self.key(3)
            self.wait("Find opens preview controls", lambda d: d["preview_controls_open"]
                      and not d["find_visible"])
            self.wait_preview_query_focus()
            self.key(0)
            query = "preview typing t " + backend
            self.native("text", query)
            self.wait("Find typing goes directly to preview query", lambda d:
                      d["preview_query_fingerprint"] == fingerprint(query)
                      and d["source_fingerprint"] == original["source_fingerprint"]
                      and d["page_dark"] == original["page_dark"] and not d["find_visible"])
            self.key(3)
            self.key(0)
            replacement = "second preview query " + backend
            self.native("text", replacement)
            self.wait("Find stays with its focused preview field", lambda d:
                      d["preview_query_fingerprint"] == fingerprint(replacement)
                      and d["source_fingerprint"] == original["source_fingerprint"]
                      and not d["find_visible"])
            self.stationary_preview_drag()
            if getattr(self, "capture_review", False) and backend == "Pdfium":
                self.capture_viewport("preview-controls")
            self.click("preview.Pop out")
            self.wait("preview detached for Find", lambda d: d["preview_popout"])
            self.native_wait("native preview window exists", lambda ws:
                             any(w["AXTitle"] == "tiptoptyp Preview" for w in ws))
            if self.snapshot()["document"]["preview_controls_open"]:
                self.click("preview.Minimize controls")
                self.wait("detached controls hidden before Find", lambda d: not d["preview_controls_open"])
            self.native("raise", "tiptoptyp Preview")
            self.key(3)
            self.wait_preview_query_focus()
            self.key(0)
            detached_query = "detached preview query " + backend
            self.native("text", detached_query)
            self.wait("detached viewer Find edits preview query", lambda d:
                      d["preview_query_fingerprint"] == fingerprint(detached_query)
                      and d["source_fingerprint"] == original["source_fingerprint"]
                      and not d["find_visible"])
            self.native("close", "tiptoptyp Preview")
            self.wait("preview returns to original document", lambda d:
                      not d["preview_popout"] and d["view_mode"] == "Split")
            if self.snapshot()["document"]["preview_controls_open"]:
                self.click("preview.Minimize controls")

    def table_target(self, name, enabled=True, previous_frame=None):
        """A closed dialog's old widget is not evidence for a new draft."""
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            target = state["targets"].get(name)
            if state["document"]["table_editor_open"] and target \
                    and target["enabled"] is enabled and target["age_ms"] <= 500 \
                    and (previous_frame is None or target["frame"] != previous_frame):
                return target
            time.sleep(0.05)
        raise AssertionError("table widget did not expose the expected fresh state: " + name)

    def open_table_workbench(self, line=0):
        self.click("editor.source")
        self.key(126, "cmd")
        for _ in range(line):
            self.key(125, "none")
        self.key(124, "none")
        self.key(124, "none")
        self.wait_target("editor.caret")
        old = {name: target["frame"] for name, target in self.snapshot()["targets"].items()
               if name.startswith("table.")}
        self.right_click("editor.caret")
        self.wait_target("menu.Edit Table…")
        self.click("menu.Edit Table…")
        self.wait("table workbench opens without replacing the source", lambda d: d["table_editor_open"])
        self.table_target("table.apply", previous_frame=old.get("table.apply"))
        assert_table_grid_only(self.snapshot())
        return old

    def reveal_table_cell(self, name, previous_frame=None, *, require_offscreen=False):
        """Reveal a virtualized column through native input; never widen cells for the test."""
        state = self.snapshot()
        initially_offscreen = visible_table_cell(state, name) is None
        if require_offscreen and not initially_offscreen:
            raise AssertionError("table cell must begin offscreen for horizontal input coverage: " + name)
        if initially_offscreen or visible_table_cell(state, name, previous_frame) is None:
            scroll, _ = table_grid_geometry(state, require_overflow=False)
            self.native("move", scroll["x"], scroll["y"])
            self.native("horizontal-wheel", scroll["x"], scroll["y"], 100)
            last_scroll = time.monotonic()
        else:
            last_scroll = None
        deadline, steps, stable, previous = time.monotonic() + 6, 0, 0, None
        previous_marker = None
        while time.monotonic() < deadline:
            state = self.snapshot()
            scroll, _ = table_grid_geometry(state, require_overflow=False)
            target = visible_table_cell(state, name, previous_frame)
            if target is not None:
                geometry = tuple(target[key] for key in ("x", "y", "width", "height"))
                marker = (target["viewport"], target["frame"])
                if previous_marker is not None and marker[0] == previous_marker[0] \
                        and marker[1] <= previous_marker[1]:
                    # Inspection can publish the owner just before its child paints.
                    # One retained hit rectangle cannot count as several observations.
                    time.sleep(0.05)
                    continue
                if previous_marker is not None and marker[0] == previous_marker[0] \
                        and previous is not None and all(
                            abs(a - b) <= 1 for a, b in zip(geometry, previous)):
                    stable += 1
                else:
                    stable, previous = 1, geometry
                previous_marker = marker
                if stable >= 3:
                    if require_offscreen and steps == 0:
                        raise AssertionError("offscreen table cell appeared without right-wheel input: " + name)
                    self.record("table.cell_revealed", target=name, horizontal_steps=steps,
                                was_offscreen=initially_offscreen,
                                stable_samples=stable, geometry=geometry)
                    return target
            else:
                stable, previous, previous_marker = 0, None, None
                if steps < 8 and (last_scroll is None or time.monotonic() - last_scroll >= 0.25):
                    self.native("move", scroll["x"], scroll["y"])
                    self.native("horizontal-wheel", scroll["x"], scroll["y"], -8)
                    steps += 1
                    last_scroll = time.monotonic()
            time.sleep(0.05)
        raise AssertionError("table cell did not become stably visible after horizontal input: " + name)

    def open_table_fixture(self, path):
        # Go to the full file path so Open selects the TeX file, not its folder.
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("private TeX table opens", lambda d: d["path"] == str(path), timeout=15)

    def reject_table_workbench(self, name, source, line=0):
        from desktop_ui_journeys import fingerprint
        lines = source.split("\n")
        if not 0 <= line < len(lines) or len(lines[line]) < 2:
            raise AssertionError("unsupported table fixture has no intended caret location")
        expected_cursor = sum(len(row) + 1 for row in lines[:line]) + 2
        self.click("editor.source")
        self.key(126, "cmd")
        for _ in range(line):
            self.key(125, "none")
        self.key(124, "none")
        self.key(124, "none")
        self.wait_target("editor.caret")
        self.wait("caret reaches the unsupported table fixture", lambda document:
                  document.get("cursor") == [expected_cursor, expected_cursor]
                  and document.get("source_fingerprint") == fingerprint(source))
        previous = self.snapshot()["targets"].get("menu.Select All")
        previous_frame = previous.get("frame") if previous else None
        self.right_click("editor.caret")
        deadline, stable_samples = time.monotonic() + 6, 0
        last_marker = None
        while time.monotonic() < deadline:
            state = self.snapshot()
            if table_offer_rejected(state, previous_frame):
                target = state["targets"]["menu.Select All"]
                marker = (target["viewport"], target["frame"])
                if last_marker is not None and marker[0] == last_marker[0] \
                        and marker[1] <= last_marker[1]:
                    time.sleep(0.05)
                    continue
                stable_samples = (stable_samples + 1 if last_marker is not None
                                  and marker[0] == last_marker[0] else 1)
                last_marker = marker
                if stable_samples >= 3:
                    self.record("table.unsupported", fixture=name, stable_samples=stable_samples)
                    self.key(53, "none")
                    return
            else:
                stable_samples, last_marker = 0, None
            time.sleep(0.05)
        raise AssertionError("unsupported table has no current rejecting context menu: " + name)

    def arxiv_tables(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        view_mode = self.snapshot()["document"]["view_mode"]
        self.key(19)
        if self.snapshot()["document"]["find_visible"]:
            self.click("find.close")
        if self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.Minimize controls")
        for case in arxiv_table_cases():
            path = self.directory / ("arxiv-" + case["file"])
            original = case["source"]
            path.write_bytes(original.encode("utf-8"))
            count = self.snapshot()["document"]["tabs"]
            self.open_table_fixture(path)
            self.source_is(original)
            for disposition in ("cancel", "apply"):
                old = self.open_table_workbench(line=case["line"])
                # Grid scrolling belongs to the child and may survive reopening.
                self.table_target("table.apply", previous_frame=old.get("table.apply"))
                self.scroll_table_grid_to_top("arxiv:" + case["file"], disposition,
                                              require_overflow=False)
                state = self.snapshot()
                assert_table_grid_only(state)
                draft = state["document"]["table_draft"]
                assert (draft["rows"], draft["columns"]) == case["dimensions"]
                row, column = case["cell"]
                target = f"table.cell.{row}.{column}"
                self.reveal_table_cell(target, previous_frame=old.get(target))
                self.click(target)
                self.key(0)
                self.type_table_cell_text(case["edited_editor_text"])
                self.source_is(original)
                self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(original), frames=3)
                self.table_target("table.apply")
                if self.capture_review:
                    self.capture_viewport("table-editor")
                self.click("table." + disposition)
                self.wait("arXiv table draft closes once", lambda d: not d["table_editor_open"])
                expected = (original if disposition == "cancel" else
                            original.replace(case["edit_before"], case["edit_after"], 1))
                self.source_is(expected)
                if disposition == "apply":
                    self.click("editor.source")
                    self.key(6)
                    self.source_is(original)
                self.record("table.arxiv", fixture=case["file"], disposition=disposition,
                            mode="grid",
                            source_fingerprint=fingerprint(original), exact_undo=disposition == "apply")
            if case["file"] == "scipy-optimizers.tex":
                self.arxiv_optimizer_last_column(case)
            self.close_scratch(count)
        self.key({"Code": 19, "Split": 20, "Preview": 21}[view_mode])
        self.wait("arXiv table journey restores its original layout", lambda d: d["view_mode"] == view_mode)

    def arxiv_optimizer_last_column(self, case):
        from desktop_ui_journeys import fingerprint
        original = case["source"]
        before, after = "0.19 & 1.0", "0.19 & 1.1"
        if case["file"] != "scipy-optimizers.tex" or original.count(before) != 1:
            raise AssertionError("optimizer rightmost edit needs its unique literal cell")
        self.source_is(original)
        old = self.open_table_workbench(line=case["line"])
        self.scroll_table_grid_to_top("arxiv:scipy-optimizers.tex", "rightmost", require_overflow=False)
        state = self.snapshot()
        assert_table_grid_only(state)
        draft = state["document"]["table_draft"]
        assert (draft["rows"], draft["columns"]) == (13, 14)
        target = "table.cell.1.13"
        self.reveal_table_cell(target, previous_frame=old.get(target), require_offscreen=True)
        self.click(target)
        self.key(0)
        self.native("text", "1.1")
        self.source_is(original)
        self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(original), frames=3)
        self.table_target("table.apply")
        if self.capture_review:
            self.capture_viewport("table-editor")
        self.click("table.apply")
        self.wait("rightmost optimizer grid edit closes once", lambda d: not d["table_editor_open"])
        self.source_is(original.replace(before, after, 1))
        self.click("editor.source")
        self.key(6)
        self.source_is(original)
        self.record("table.arxiv_rightmost", fixture=case["file"], cell=[1, 13],
                    was_offscreen=True, exact_undo=True)

    def tables(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        self.key(19)  # Keep the source visible while a separate draft is open.
        if self.snapshot()["document"]["find_visible"]:
            self.click("find.close")
        if self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.Minimize controls")
        typst = '#table(columns: (40%, auto), inset: 3pt,\n  table.hline(y: 0, stroke: 1pt + blue),\n  [Alpha], [Beta], [Gamma], [Delta])'
        count = self.scratch(typst)

        def unchanged(text):
            self.source_is(text)
            self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(text))

        def close_draft(button, expected):
            self.click("table." + button)
            self.wait("table draft closes once", lambda d: not d["table_editor_open"])
            self.source_is(expected)
            self.click("editor.source")

        old = self.open_table_workbench()
        self.table_target("table.cell.0.0", previous_frame=old.get("table.cell.0.0"))
        self.click("table.cell.0.0")
        self.key(0)  # Cmd+A selects only this cell's text.
        self.native("text", "Cancelled")
        unchanged(typst)
        close_draft("cancel", typst)

        old = self.open_table_workbench()
        self.table_target("table.cell.0.0", previous_frame=old.get("table.cell.0.0"))
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Changed")
        unchanged(typst)
        applied = '#table(\n  columns: (40%, auto),\n  inset: 3pt,\n  table.hline(y: 0, stroke: 1pt + blue),\n  [Changed],\n  [Beta],\n  [Gamma],\n  [Delta],\n)'
        if getattr(self, "capture_review", False):
            self.capture_viewport("table-editor")
            self.click("table.format")
            self.capture_viewport("table-editor")
            self.click("table.format")
        close_draft("apply", applied)
        self.key(6)
        self.source_is(typst)
        self.key(6, "cmd+shift")
        self.source_is(applied)
        self.key(6)
        self.source_is(typst)
        self.close_scratch(count)

        dynamic = '#let widths = (40%, auto)\n#let rows = ([Alpha], [Beta])\n#table(columns: widths, ..rows)'
        count = self.scratch(dynamic)
        self.reject_table_workbench("typst-generated", dynamic, line=2)
        unchanged(dynamic)
        self.close_scratch(count)

        computed = '#let price = 60\n#table(columns: 2, [Revenue], [#price])'
        count = self.scratch(computed)
        self.open_table_workbench(line=1)
        self.wait("Typst variable resolves to a locked compiler sample", lambda d:
                  not d["table_samples"]["pending"] and not d["table_samples"]["failed"]
                  and d["table_samples"]["grid"] is not None)
        samples = self.snapshot()["document"]["table_samples"]
        assert samples["locked"] == [[0, 1]]
        assert samples["grid"]["values"][1]["fingerprint"] == fingerprint("60")
        self.table_target("table.locked.0.1")
        self.click("table.locked.0.1")
        self.native("text", "must stay locked")
        unchanged(computed)
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Income")
        if getattr(self, "capture_review", False):
            self.capture_viewport("table-editor")
        applied = '#let price = 60\n#table(\n  columns: 2,\n  [Income],\n  [#price],\n)'
        close_draft("apply", applied)
        self.key(6)
        self.source_is(computed)
        self.close_scratch(count)

        # TeX tables must use a named .tex tab, not Typst's default scratch kind.
        # Select the isolated fixture by its complete file path in native Open.
        environment = r"\begin{tabular}[t]{lc}Alpha & Beta \\[2pt]Gamma & Delta \\\end{tabular}"
        tex = "\\documentclass{article}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
        path = self.directory / "table-workbench.tex"
        path.write_text(tex)
        count = self.snapshot()["document"]["tabs"]
        self.open_table_fixture(path)
        self.source_is(tex)
        old = self.open_table_workbench(line=2)
        self.table_target("table.cell.0.0", previous_frame=old.get("table.cell.0.0"))
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Changed")
        unchanged(tex)
        applied = tex.replace("Alpha", "Changed", 1)
        close_draft("apply", applied)
        self.key(6)
        self.source_is(tex)
        self.key(6, "cmd+shift")
        self.source_is(applied)
        self.key(6)
        self.source_is(tex)

        old = self.open_table_workbench(line=2)
        self.table_target("table.cell.0.0", previous_frame=old.get("table.cell.0.0"))
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "X & Y")
        self.table_target("table.apply", enabled=False)
        unchanged(tex)
        assert_table_grid_only(self.snapshot(), valid=False)
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Repaired")
        self.table_target("table.apply")
        repaired = environment.replace("Alpha", "Repaired")
        changed = tex.replace(environment, repaired)
        close_draft("apply", changed)
        self.key(6)
        self.source_is(tex)
        self.close_scratch(count)

        # Dynamic TeX geometry must stay in the document editor instead of
        # opening an opaque source editor disguised as a table workbench.
        environment = r"\begin{tabular}{*{\colcount}{l}}Alpha & Beta\\\end{tabular}"
        tex = "\\documentclass{article}\n\\newcommand{\\colcount}{2}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
        path = self.directory / "table-dynamic-columns.tex"
        path.write_text(tex)
        count = self.snapshot()["document"]["tabs"]
        self.open_table_fixture(path)
        self.reject_table_workbench("tex-dynamic-columns", tex, line=3)
        unchanged(tex)
        self.close_scratch(count)

        # Missing column imports are added only on Apply, with one Undo/Redo.
        environment = r"\begin{tabular}{lS[table-format=2.0]}Alpha & 60\end{tabular}"
        tex = "\\documentclass{article}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
        path = self.directory / "table-auto-imports.tex"
        path.write_text(tex)
        count = self.snapshot()["document"]["tabs"]
        self.open_table_fixture(path)
        self.open_table_workbench(line=2)
        self.table_target("table.cell.0.0")
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Cancelled")
        unchanged(tex)
        close_draft("cancel", tex)
        self.open_table_workbench(line=2)
        self.table_target("table.cell.0.0")
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Changed")
        unchanged(tex)
        applied = tex.replace("\\begin{document}", "\\usepackage{siunitx}\n\\begin{document}").replace("Alpha & 60", "Changed & 60")
        close_draft("apply", applied)
        self.key(6)
        self.source_is(tex)
        self.key(6, "cmd+shift")
        self.source_is(applied)
        # Applying again preserves the single import and existing source.
        self.open_table_workbench(line=3)
        self.table_target("table.apply")
        close_draft("apply", applied)
        self.key(6)
        self.source_is(tex)
        self.close_scratch(count)

        environment = r"\begin{tabular}{ll}Revenue & \revenue\end{tabular}"
        tex = "\\documentclass{article}\n\\newcommand{\\revenue}{60}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
        path = self.directory / "table-sampled.tex"
        path.write_text(tex)
        count = self.snapshot()["document"]["tabs"]
        self.open_table_fixture(path)
        self.source_is(tex)
        self.open_table_workbench(line=3)
        self.wait("TeX macro resolves to a locked compiler sample", lambda d:
                  not d["table_samples"]["pending"] and not d["table_samples"]["failed"]
                  and d["table_samples"]["grid"] is not None)
        samples = self.snapshot()["document"]["table_samples"]
        assert samples["locked"] == [[0, 1]]
        assert samples["grid"]["values"][0]["fingerprint"] == fingerprint("60")
        self.table_target("table.locked.0.1")
        self.click("table.locked.0.1")
        self.native("text", "must stay locked")
        unchanged(tex)
        self.click("table.cell.0.0")
        self.key(0)
        self.native("text", "Income")
        if getattr(self, "capture_review", False):
            self.capture_viewport("table-editor")
        close_draft("apply", tex.replace("Revenue &", "Income &"))
        self.key(6)
        self.source_is(tex)
        self.close_scratch(count)
        # Exercise both newly supported column dialects with real clicks and
        # compiler-backed locks. Header options must survive Apply and undo.
        for name, package, environment in [
            ("tabularray", "tabularray", r"\begin{tblr}[baseline=T]{colspec={Q[l]X[2,r]},hlines,vlines}Revenue & \revenue\\\end{tblr}"),
            ("siunitx", "siunitx", r"\begin{tabular}{lS[table-format=2.0]}Revenue & \revenue\\\end{tabular}"),
        ]:
            tex = "\\documentclass{article}\n\\usepackage{" + package + "}\n\\newcommand{\\revenue}{60}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
            path = self.directory / ("table-package-" + name + ".tex")
            path.write_text(tex)
            count = self.snapshot()["document"]["tabs"]
            self.open_table_fixture(path)
            self.source_is(tex)
            self.open_table_workbench(line=4)
            self.wait(name + " macro resolves to a locked sample", lambda d:
                      not d["table_samples"]["pending"] and not d["table_samples"]["failed"]
                      and d["table_samples"]["grid"] is not None, timeout=20)
            samples = self.snapshot()["document"]["table_samples"]
            assert samples["locked"] == [[0, 1]]
            assert samples["grid"]["values"][0]["fingerprint"] == fingerprint("60")
            self.table_target("table.cell.0.0")
            self.table_target("table.locked.0.1")
            self.click("table.locked.0.1")
            self.native("text", "must stay locked")
            unchanged(tex)
            self.click("table.cell.0.0")
            self.key(0)
            self.native("text", "Income")
            if getattr(self, "capture_review", False):
                self.capture_viewport("table-editor")
            close_draft("apply", tex.replace("Revenue &", "Income &"))
            self.key(6)
            self.source_is(tex)
            self.close_scratch(count)
        self.compact_table_grid("typst")
        self.compact_table_grid("tex")
        self.key(20)
        self.wait("table journey restores the split editor", lambda d: d["view_mode"] == "Split")

    def scroll_table_grid_to_top(self, language, disposition, *, require_overflow=True):
        scroll, footer = table_grid_geometry(self.snapshot(), require_overflow=require_overflow)
        before = scroll["offset"]
        if before > 1:
            self.native("move", scroll["x"], scroll["y"])
            self.native("wheel", scroll["x"], scroll["y"], 20)
        deadline, stable_samples = time.monotonic() + 4, 0
        while time.monotonic() < deadline:
            scroll, _ = table_grid_geometry(self.snapshot(), footer, require_overflow=require_overflow)
            stable_samples = stable_samples + 1 if scroll["offset"] <= 1 else 0
            if stable_samples >= 3:
                self.record("table.scroll_top", language=language, disposition=disposition,
                            before=before, after=scroll["offset"], stable_samples=stable_samples)
                return
            time.sleep(0.05)
        raise AssertionError(language + " table grid did not settle at its first row after wheel input")

    def type_table_cell_text(self, text):
        # The native driver deliberately paces 16-unit key batches. Bound each
        # call so a long-cell fixture stays inside its fixed process watchdog.
        for start in range(0, len(text), 256):
            self.native("text", text[start:start + 256])

    def compact_table_grid(self, language):
        from desktop_ui_journeys import fingerprint
        long_text = ("A long table cell stays inside its own row while the footer remains visible. " * 8).rstrip()
        edited_long = ("Edited long cell text should remain keyboard editable without expanding the entire grid. " * 10).rstrip()
        rows = [[long_text, "Neighbour", "Status"]] + [
            [f"Row {row}", f"Owner {row}", "Planned"] for row in range(1, 36)]
        if language == "typst":
            source = "#table(\n  columns: 3,\n" + "".join(
                "  [" + cell + "],\n" for row in rows for cell in row) + ")"
            count = self.scratch(source)
            line = 0
        else:
            environment = "\\begin{tabular}{lll}\n" + "".join(
                " & ".join(row) + "\\\\\n" for row in rows) + "\\end{tabular}"
            source = "\\documentclass{article}\n\\begin{document}\n" + environment + "\n\\end{document}\n"
            path = self.directory / "table-compact-grid.tex"
            path.write_text(source)
            count = self.snapshot()["document"]["tabs"]
            self.open_table_fixture(path)
            line = 2
        applied = source.replace(long_text, edited_long).replace("Neighbour", "Edited neighbour")
        for action in ("cancel", "apply"):
            self.open_table_workbench(line=line)
            windows = self.native_wait("the native Table editor owns focus before resizing", lambda ws:
                                       table_native_window(ws) is not None)
            parent_sizes = table_parent_sizes(windows)
            if not parent_sizes:
                raise AssertionError("table resize has no observed parent window")
            requested_size = [620, 420]
            self.native("resize-focused", *requested_size)
            previous_size, stable_samples = None, 0
            def minimum_size_settled(windows):
                nonlocal previous_size, stable_samples
                table = table_native_window(windows, requested_size)
                if table is None or table_parent_sizes(windows) != parent_sizes:
                    previous_size, stable_samples = None, 0
                    return False
                size = table["size"]
                stable_samples = stable_samples + 1 if size == previous_size else 1
                previous_size = list(size)
                return stable_samples >= 3
            windows = self.native_wait("minimum-size Table editor settles without resizing its parent",
                                       minimum_size_settled)
            self.record("table.native_size", language=language, disposition=action,
                        requested=requested_size, actual=table_native_window(windows)["size"],
                        parent_sizes=parent_sizes, stable_samples=stable_samples)
            # Reopening intentionally preserves the stable grid's scroll state.
            # Use real wheel input to reveal virtualized first-row editors.
            self.scroll_table_grid_to_top(language, action)
            self.table_target("table.cell.0.0")
            self.click("table.cell.0.0")
            self.key(0)
            self.type_table_cell_text(edited_long)
            if language == "typst":
                self.key(48, "none")
                self.wait_target("table.cell.0.1", focused=True)
            else:
                # TeX's code cells keep Tab available for source indentation.
                self.click("table.cell.0.1")
            self.key(0)
            self.native("text", "Edited neighbour")
            self.source_is(source)
            self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(source))
            state = self.snapshot()
            pitch = assert_compact_table_rows(state)
            scroll, footer = table_grid_geometry(state)
            before = scroll["offset"]
            self.native("move", scroll["x"], scroll["y"])
            self.native("wheel", scroll["x"], scroll["y"], -8)
            deadline = time.monotonic() + 4
            while True:
                scroll, _ = table_grid_geometry(self.snapshot(), footer)
                if scroll["offset"] > before + 20:
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(language + " table grid did not scroll after wheel input")
                time.sleep(0.05)
            for _ in range(4):
                current, _ = table_grid_geometry(self.snapshot(), footer)
                if current["offset"] <= before + 20:
                    raise AssertionError(language + " table grid scroll fought back")
            if getattr(self, "capture_review", False):
                self.capture_viewport("table-editor")
            self.record("table.compact_grid", language=language, disposition=action,
                        row_pitch=pitch, offset_before=before, offset_after=current["offset"],
                        footer=footer, long_cell_bytes=len(edited_long.encode("utf-8")))
            self.click("table." + action)
            self.wait("compact table draft closes once", lambda d: not d["table_editor_open"])
            self.source_is(source if action == "cancel" else applied)
            self.click("editor.source")
            if action == "apply":
                self.key(6)
                self.source_is(source)
        self.close_scratch(count)
