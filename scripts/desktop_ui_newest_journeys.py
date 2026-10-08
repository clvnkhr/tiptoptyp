"""Real input regressions for focus, preview controls and table draft lifetimes."""

import math
import shlex
import time

NEWEST_JOURNEYS = ("problems_focus", "preview_find", "tables")


def assert_preview_drag_position(document, baseline, delta, tolerance=3):
    actual = document.get("preview_controls_position")
    vectors = (actual, baseline, delta)
    if any(not value or len(value) != 2 or not all(math.isfinite(x) for x in value)
           for value in vectors):
        raise AssertionError("invalid preview controls drag geometry")
    if any(abs(actual[axis] - baseline[axis] - delta[axis]) > tolerance for axis in (0, 1)):
        raise AssertionError("preview controls drifted from the held pointer")


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

    def wait_table_source_draft(self, source, valid):
        from desktop_ui_journeys import fingerprint
        self.wait("table source draft receives complete input", lambda d:
                  d["table_editor_open"] and (d.get("table_draft") or {}).get("fingerprint") == fingerprint(source)
                  and (d.get("table_draft") or {}).get("bytes") == len(source.encode("utf-8"))
                  and (d.get("table_draft") or {}).get("valid") is valid)

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
        return old

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
        old = self.open_table_workbench(line=2)
        self.table_target("table.source", previous_frame=old.get("table.source"))
        self.click("table.source")
        self.key(0)
        incomplete = "table(columns: widths, ..rows"
        self.native("text", incomplete)
        self.wait_table_source_draft(incomplete, False)
        self.table_target("table.apply", enabled=False)
        unchanged(dynamic)
        self.key(0)
        draft = "table(columns: widths, fill: blue, ..rows)"
        self.native("text", draft)
        self.wait_table_source_draft(draft, True)
        self.table_target("table.apply")
        changed = dynamic.replace("table(columns: widths, ..rows)", draft)
        close_draft("apply", changed)
        self.key(6)
        self.source_is(dynamic)
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
        rendered = r"\begin{tabular}[t]{lc}Changed & Beta\\[2pt]Gamma & Delta\\\end{tabular}"
        applied = tex.replace(environment, rendered)
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
        old_source = self.snapshot()["targets"].get("table.source")
        self.click("table.source-mode")
        self.table_target("table.source", previous_frame=old_source["frame"] if old_source else None)
        self.click("table.source")
        self.key(0)
        repaired = environment.replace("Alpha", "Repaired")
        self.native("text", repaired)
        self.wait_table_source_draft(repaired, True)
        self.table_target("table.apply")
        changed = tex.replace(environment, repaired)
        close_draft("apply", changed)
        self.key(6)
        self.source_is(tex)
        self.close_scratch(count)
        self.key(20)
        self.wait("table journey restores the split editor", lambda d: d["view_mode"] == "Split")
