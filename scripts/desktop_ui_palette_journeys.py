"""Command palette checks using native keys/clicks and read-only observations."""

import math
import shlex
import time
from pathlib import Path


class PaletteJourneys:
    def palette_open(self):
        self.key(35, "cmd+shift")  # Cmd+Shift+P
        self.wait("command palette opens", lambda d: d["palette_visible"])
        self.wait_target("palette.search", focused=True)

    def palette_query(self, query):
        from desktop_ui_journeys import fingerprint
        self.wait_target("palette.search", focused=True)
        self.key(0)
        self.native("text", query)
        return self.wait("palette receives the complete query", lambda d:
                         d["palette_visible"] and d["palette_query_fingerprint"] == fingerprint(query))["document"]

    @staticmethod
    def palette_source_unchanged(document, before):
        return all(document[field] == before[field]
                   for field in ("source_fingerprint", "source_bytes", "tabs", "path"))

    def palette_cancel_outside(self):
        state = self.snapshot()
        source = state["targets"].get("editor.source")
        if not source or source["age_ms"] > 500:
            raise AssertionError("missing current source geometry for outside-palette click")
        if (not all(math.isfinite(source.get(key, float("nan")))
                    for key in ("x", "y", "width", "height"))
                or source["width"] <= 0 or source["height"] <= 0):
            raise AssertionError("invalid source geometry for outside-palette click")
        # Choose a point inside the bottom-left of the actual source response,
        # away from the centered palette. No hard-coded screen coordinates.
        x = source["x"] - source["width"] / 2 + min(12, source["width"] / 4)
        y = source["y"] + source["height"] / 2 - min(12, source["height"] / 4)
        for name, target in state["targets"].items():
            if name.startswith("palette.") and target["age_ms"] <= 500:
                if (abs(x - target["x"]) <= target["width"] / 2 + 4
                        and abs(y - target["y"]) <= target["height"] / 2 + 4):
                    raise AssertionError("derived outside click overlaps a palette control")
        self.native("move", x, y)
        self.record("target", name="palette.outside", target={"x": x, "y": y})
        self.native("click", x, y)

    def palette_find_origin(self, before):
        """Cancellation restores the actual Replace field after Find's blur grace."""
        from desktop_ui_journeys import fingerprint
        unchanged = lambda d: self.palette_source_unchanged(d, before)
        self.click("editor.source")
        self.key(3)
        self.wait_target("find.query", focused=True)
        self.key(0)
        self.native("text", "Palette")
        self.wait("Find query is ready before palette handoff", lambda d:
                  d["find_query_fingerprint"] == fingerprint("Palette") and unchanged(d))
        self.click("find.replace")
        self.wait("Replace is expanded", lambda d: d["replace_visible"] and unchanged(d))
        self.click("find.replacement")
        self.wait_target("find.replacement", focused=True)
        self.key(0)
        self.native("text", "palette-before")
        self.palette_open()
        self.palette_query("Explorer")
        # Cross the real 150 ms Find blur grace while its temporary palette is
        # focused; a single settled frame cannot establish this regression.
        time.sleep(0.3)
        self.stable_state(lambda d: d["palette_visible"] and d["find_visible"]
                          and d["replace_visible"] and unchanged(d))
        self.key(53, "none")
        self.wait("palette cancellation returns native focus to Find", lambda d:
                  not d["palette_visible"] and d["find_visible"] and d["find_focused"]
                  and d["replace_visible"] and unchanged(d))
        self.wait_target("find.replacement", focused=True)
        self.native("text", "-after")
        self.wait("typing after palette cancellation preserves source and Find query", lambda d:
                  d["find_query_fingerprint"] == fingerprint("Palette") and unchanged(d))
        self.click("find.replace_one")
        replaced = "palette-before-after input must never change this source."
        self.source_is(replaced)
        # In this short fixture the source response sits behind the Find
        # popup. Close it before clicking source, so Undo reaches the document.
        self.click("find.close")
        self.wait("Find closes after palette origin checks", lambda d:
                  not d["find_visible"] and d["source_fingerprint"] == fingerprint(replaced))
        self.click("editor.source")
        self.wait("source owns Undo after Find closes", lambda d: d["editor_keyboard_focused"])
        self.key(6)
        self.wait("Undo restores the unchanged palette source", unchanged)

    def command_palette(self):
        self.focus_document_window()
        if self.snapshot()["document"]["find_visible"]:
            self.click("find.close")
            self.wait("source Find closes before palette checks", lambda d: not d["find_visible"])
        initial = self.snapshot()["document"]
        count = self.scratch("Palette input must never change this source.")
        before = self.snapshot()["document"]
        unchanged = lambda d: self.palette_source_unchanged(d, before)
        wrap = before["line_wrap"]
        explorer = before["explorer_visible"]

        self.palette_open()
        if getattr(self, "capture_review", False):
            self.capture_viewport("popup")
        empty = self.palette_query("unmatched-palette-zzzz")
        if empty["palette_results"] or empty["palette_selected"] is not None:
            raise AssertionError("unmatched palette query retained a selectable command")
        self.key(36, "none")
        self.stable_state(lambda d: d["palette_visible"] and not d["palette_results"]
                          and d["palette_selected"] is None and unchanged(d))
        mixed = self.palette_query("LiNe")
        results = mixed["palette_results"]
        if len(results) < 2 or "editor.line_wrap" not in results or "editor.line_numbers" not in results:
            raise AssertionError("line query does not expose both editor commands")
        enabled = [action for action in results if action in mixed["palette_enabled"]]
        if len(enabled) < 2 or mixed["palette_selected"] != enabled[0]:
            raise AssertionError("palette did not select its first enabled result")
        lower = self.palette_query("line")
        if lower["palette_results"] != results:
            raise AssertionError("palette filtering is not case-insensitive")
        self.key(125, "none")
        self.wait("Down selects exactly the next enabled command", lambda d:
                  d["palette_selected"] == enabled[1] and unchanged(d))
        self.key(126, "none")
        self.wait("Up returns to the first enabled command", lambda d:
                  d["palette_selected"] == enabled[0] and unchanged(d))
        selected = self.palette_query("line wrapping")
        if selected["palette_selected"] != "editor.line_wrap" or "editor.line_wrap" not in selected["palette_enabled"]:
            raise AssertionError("line wrapping query did not select its enabled command")
        if getattr(self, "capture_review", False):
            self.capture_viewport("popup")
        self.key(36, "none")
        self.wait("Enter executes line wrapping once and closes palette", lambda d:
                  not d["palette_visible"] and d["line_wrap"] != wrap and unchanged(d))
        self.stable_state(lambda d: not d["palette_visible"] and d["line_wrap"] != wrap and unchanged(d), frames=16)
        self.palette_open()
        self.palette_query("line wrapping")
        self.click("palette.command.editor.line_wrap")
        self.wait("mouse command restores wrapping once", lambda d:
                  not d["palette_visible"] and d["line_wrap"] == wrap and unchanged(d))
        self.stable_state(lambda d: not d["palette_visible"] and d["line_wrap"] == wrap and unchanged(d))

        for cancel in (lambda: self.key(53, "none"), self.palette_cancel_outside):
            self.palette_open()
            pending = self.palette_query("Explorer")
            if "view.explorer" not in pending["palette_enabled"]:
                raise AssertionError("Explorer command is not available before cancellation")
            cancel()
            self.wait("cancellation dismisses palette without executing a command", lambda d:
                      not d["palette_visible"] and d["explorer_visible"] == explorer
                      and d["line_wrap"] == wrap and unchanged(d))
            self.stable_state(lambda d: not d["palette_visible"] and d["explorer_visible"] == explorer and unchanged(d))

        self.palette_find_origin(before)
        self.palette_terminal(before)
        self.settings()
        self.key(35, "cmd+shift")
        self.stable_state(lambda d: d["settings_visible"] and not d["palette_visible"]
                          and d["line_wrap"] == wrap and unchanged(d), frames=16)
        self.native_wait("Settings retains native focus after palette shortcut", lambda windows:
                         any("Settings" in window["AXTitle"] and window.get("AXFocused") for window in windows))
        self.close_settings()
        self.click("editor.source")
        self.close_scratch(count)
        self.wait("palette checks preserve the original document", lambda d:
                  self.palette_source_unchanged(d, initial))
        if getattr(self, "command_palette_check_empty", False):
            self.palette_empty_document()

    def palette_terminal(self, before):
        unchanged = lambda d: self.palette_source_unchanged(d, before) and not d["find_visible"]
        state = self.snapshot()["document"]
        panel = state["panel"]
        terminal_count = state["terminal_count"]
        terminal_active = state["terminal_active"]
        if panel is None:
            self.key(23)
            self.wait("bottom panel opens for palette focus check", lambda d: d["panel"] is not None)
        if self.snapshot()["document"]["panel"] != "Terminal":
            self.click("panel.terminal")
        self.wait_target("terminal.new")
        self.click("terminal.new")
        self.wait("palette check owns a private terminal pane", lambda d:
                  d["terminal_count"] == terminal_count + 1 and d["terminal_active"] == terminal_count, timeout=15)
        self.wait_target("terminal.input")
        self.click("terminal.input")
        self.wait("terminal owns keyboard before palette", lambda d: d["terminal_keyboard_focused"] and unchanged(d))
        self.palette_open()
        self.palette_query("Explorer")
        self.key(53, "none")
        self.wait("palette Escape restores terminal focus", lambda d:
                  not d["palette_visible"] and d["terminal_keyboard_focused"] and unchanged(d))
        wrap = self.snapshot()["document"]["line_wrap"]
        for expected in (not wrap, wrap):
            self.palette_open()
            self.palette_query("line wrapping")
            self.key(36, "none")
            self.wait("executing a palette command retains terminal ownership", lambda d:
                      not d["palette_visible"] and d["line_wrap"] == expected
                      and d["terminal_keyboard_focused"] and unchanged(d))
            self.stable_state(lambda d: d["line_wrap"] == expected and unchanged(d))
        comfy = self.snapshot()["document"]["comfy"]
        for expected in (not comfy, comfy):
            self.palette_open()
            self.palette_query("comfy preview")
            self.key(36, "none")
            self.wait("palette Comfy Preview runs while the terminal keeps focus", lambda d:
                      not d["palette_visible"] and d["comfy"] == expected
                      and d["terminal_keyboard_focused"] and unchanged(d))
            self.stable_state(lambda d: d["comfy"] == expected and unchanged(d))
        receipt = self.directory / "palette-terminal.receipt"
        self.native("text", "printf '%s' palette > " + shlex.quote(str(receipt)))
        self.key(36, "none")
        self.wait("native typing still reaches the terminal shell", lambda d:
                  receipt.exists() and receipt.read_text() == "palette" and unchanged(d), timeout=15)
        self.palette_open()
        document = self.palette_query("Find")
        if "edit.find" not in document["palette_enabled"]:
            raise AssertionError("Find is disabled for the focused terminal")
        self.click("palette.command.edit.find")
        self.wait("palette Find targets terminal instead of source", lambda d:
                  not d["palette_visible"] and d["terminal_find"]["open"]
                  and d["terminal_find"]["focused"] and unchanged(d))
        self.native("text", "palette-terminal-search")
        self.wait("terminal Find query receives palette handoff typing", lambda d:
                  d["terminal_find"]["query"] == "palette-terminal-search"
                  and d["terminal_find"]["focused"] and unchanged(d))
        self.key(53, "none")
        self.wait("terminal Find closes after palette handoff", lambda d: not d["terminal_find"]["open"] and unchanged(d))
        self.click("terminal.off")
        self.wait("palette terminal pane is reaped", lambda d: d["terminal_count"] == terminal_count and unchanged(d))
        if terminal_count and terminal_active is not None:
            self.click("terminal." + str(terminal_active))
        if panel is None:
            self.click("panel.close")
        elif panel != "Terminal":
            self.click("panel." + panel.lower())
        self.wait("palette restores the previous bottom panel", lambda d: d["panel"] == panel and unchanged(d))

    def palette_empty_document(self):
        # Standalone-only: the aggregate suite may own unsaved tabs from other
        # journeys, so it must not discard them merely to create an empty window.
        before = self.snapshot()["document"]
        if before["tabs"] != 1 or before["dirty"] or before["path"] is None:
            raise AssertionError("empty palette check requires its clean launch fixture")
        path = Path(before["path"])
        disk = path.read_bytes()
        self.key(13)
        self.wait("closing the final clean tab keeps the window alive", lambda d: d["tabs"] == 0)
        self.palette_open()
        document = self.palette_query("New document")
        if "file.new" not in document["palette_enabled"]:
            raise AssertionError("New is unavailable in the no-tabs palette")
        self.click("palette.command.file.new")
        self.wait("palette New creates exactly one empty document", lambda d:
                  not d["palette_visible"] and d["tabs"] == 1 and d["path"] is None and d["source_bytes"] == 0)
        self.stable_state(lambda d: not d["palette_visible"] and d["tabs"] == 1
                          and d["path"] is None and d["source_bytes"] == 0, frames=16)
        self.key(13)
        self.wait("clean palette document closes", lambda d: d["tabs"] == 0)
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("original palette fixture reopens unchanged", lambda d:
                  d["path"] == str(path) and self.palette_source_unchanged(d, before), timeout=15)
        if path.read_bytes() != disk:
            raise AssertionError("palette command changed the original fixture on disk")
