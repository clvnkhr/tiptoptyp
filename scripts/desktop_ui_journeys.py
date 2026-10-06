"""Regression journeys driven only through native UI input.

The inspection socket supplies assertions and actual hit rectangles, never actions.
See docs/desktop-ui-audit.md for the lower-level tests these journeys promote.
"""

from desktop_ui_tex_journeys import TexJourneys

PREVIEW_SOURCE = '#set page(width: 200pt, height: 200pt, margin: 20pt)\n= First\nBlack text on white paper.\n#pagebreak()\n= Second\nMiddle page.\n#pagebreak()\n= Third\n#v(95pt)\nFinal needle.\n'


def fingerprint(text):
    value = 0xcbf29ce484222325
    for byte in text.encode("utf-8"):
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"


class EditorJourneys(TexJourneys):
    def focus_document_window(self):
        if self.snapshot()["document"]["focused"]:
            return
        windows = self.native("windows").get("windows", [])
        title = next((window.get("AXTitle") for window in windows
                      if window.get("AXMain") and not window.get("AXMinimized")), None)
        self.native("activate")
        if title:
            self.native("raise", title)
        self.wait("document window focused", lambda d: d["focused"])

    def paste_source(self, text, check=None):
        import json
        import subprocess
        process = subprocess.Popen([str(self.driver), "paste", str(self.process.pid), text],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            import select
            if not select.select([process.stdout], [], [], 6)[0]:
                raise RuntimeError("paste adapter did not acknowledge input")
            receipt = process.stdout.readline()
            if not receipt or json.loads(receipt).get("posted") != "paste":
                raise RuntimeError("paste adapter failed before posting input")
            self.record("paste", bytes=len(text.encode("utf-8")), fingerprint=fingerprint(text))
            if check is None:
                self.source_is(text)
            else:
                check()
        finally:
            _, error = process.communicate("restore\n", timeout=6)
            if process.returncode:
                raise RuntimeError(error)

    def capture_viewport(self, target):
        import shutil
        import time
        directory = self.directory / ".tiptoptyp/screenshots"
        before = set(directory.rglob("*.png"))
        self.key(111, "cmd+shift")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            fresh = set(directory.rglob("*.png")) - before
            matches = [path for path in fresh if target in path.name and path.read_bytes().endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82")]
            if matches:
                for path in matches:
                    shutil.copy2(path, self.evidence / path.name)
                self.record("framebuffer.captured", target=target, files=[p.name for p in matches])
                return
            time.sleep(0.05)
        raise AssertionError("no fresh framebuffer for " + target)

    def source_is(self, text):
        return self.wait("exact fixture content", lambda d: d["source_fingerprint"] == fingerprint(text) and d["source_bytes"] == len(text.encode("utf-8")))

    def ensure_typesetting_tab(self):
        """Keep document-scoped journeys independent of dialog tab residue."""
        if (self.snapshot()["document"]["path"] or "").endswith((".typ", ".tex")):
            return
        for _ in range(4):
            self.key(48, "ctrl+shift")
            if (self.snapshot()["document"]["path"] or "").endswith((".typ", ".tex")):
                break
        self.wait("journey selects a typesetting document", lambda d: (d["path"] or "").endswith((".typ", ".tex")))

    def scratch(self, text=""):
        self.focus_document_window()
        count = self.snapshot()["document"]["tabs"]
        self.key(45)
        self.wait("new empty scratch tab", lambda d: d["tabs"] == count + 1 and d["path"] is None and d["source_bytes"] == 0)
        self.wait_target("editor.source")
        self.click("editor.source")
        if text:
            if "\n" in text:
                self.paste_source(text)
            else:
                self.native("text", text)
                self.source_is(text)
        return count

    def close_scratch(self, count):
        self.key(13)
        self.wait("scratch close either completes or asks for confirmation",
                  lambda d: d["tabs"] == count or d["modal_open"])
        if self.snapshot()["document"]["modal_open"]:
            self.wait_target("modal.discard")
            self.click("modal.discard")
        self.wait("scratch tab closed once", lambda d: d["tabs"] == count and not d["modal_open"])

    def editing(self):
        count = self.scratch()
        self.click("editor.source")
        self.native("text", "zzz")
        # Observe the edit and caret in one frame. Waiting for the source
        # first can allow an asynchronous selection update to move the caret
        # before the regression assertion samples it.
        self.wait("typed caret follows text", lambda d: d["source_fingerprint"] == fingerprint("zzz")
                  and d["cursor"] == [3, 3] and d["dirty"])
        self.stable_state(lambda d: d["source_fingerprint"] == fingerprint("zzz") and d["cursor"] == [3, 3])
        self.click("editor.source")
        self.key(6)  # Undo
        self.source_is("")
        self.wait("undo to saved contents clears unsaved state", lambda d: not d["dirty"])
        self.click("editor.source")
        self.key(123, "cmd")  # Move caret before replaying the edit.
        self.click("editor.source")
        self.key(6, "cmd+shift")
        self.source_is("zzz")
        self.wait("redo restores unsaved state", lambda d: d["dirty"])
        self.wait("redo restores post-edit caret", lambda d: d["cursor"] == [3, 3])
        self.native("text", "!")
        self.source_is("zzz!")
        self.key(0)
        self.native("text", "é🙂z")
        self.source_is("é🙂z")
        self.wait("Unicode scalar caret", lambda d: d["cursor"] == [3, 3])
        self.key(6)
        self.source_is("zzz!")
        self.key(6, "cmd+shift")
        self.source_is("é🙂z")
        self.wait("Unicode redo caret", lambda d: d["cursor"] == [3, 3])
        self.key(0)
        self.paste_source("  alpha\n\tbeta")
        self.source_is("  alpha\n\tbeta")
        self.key(0)
        self.key(44)  # Toggle comment without moving the indentation.
        self.source_is("  // alpha\n\t// beta")
        self.key(44)
        self.source_is("  alpha\n\tbeta")
        self.key(0)
        self.paste_source("one\ntwo\nthree")
        self.key(126, "cmd")  # Start of document.
        self.key(125, "shift")  # Caret at column zero of line two.
        self.wait("downward selection ends on next line", lambda d: d["cursor"] == [4, 0])
        self.key(44)
        self.source_is("// one\n// two\nthree")
        self.wait("comment preserves downward selection", lambda d: d["cursor"] == [10, 3])
        self.key(44)
        self.source_is("one\ntwo\nthree")
        self.wait("uncomment preserves downward selection", lambda d: d["cursor"] == [4, 0])
        self.key(124, "none")  # Collapse at line two's start.
        self.key(126, "shift")
        self.wait("upward selection includes preceding newline", lambda d: d["cursor"] == [0, 4])
        self.key(44)
        self.source_is("// one\ntwo\nthree")
        self.wait("comment preserves upward selection", lambda d: d["cursor"] == [3, 7])
        self.key(44)
        self.source_is("one\ntwo\nthree")
        self.wait("uncomment preserves upward selection", lambda d: d["cursor"] == [0, 4])
        self.wait_target("editor.line_start.1")
        self.drag("editor.line_start.1", "editor.line_start.0")
        self.wait("upward mouse selection includes preceding newline", lambda d: d["cursor"] == [0, 4])
        self.key(51, "none")
        self.source_is("two\nthree")  # Deleting the selection removes its newline too.
        self.key(6)
        self.source_is("one\ntwo\nthree")
        self.key(0)
        self.paste_source("one\n")
        self.key(126, "cmd")
        self.key(125, "shift")
        self.wait("blank endpoint selected", lambda d: d["cursor"] == [4, 0])
        self.key(44)
        self.source_is("// one\n// ")
        self.key(44)
        self.source_is("one\n")
        self.close_scratch(count)

    def search(self):
        text = "alpha beta alpha ALPHA"
        count = self.scratch(text)
        self.key(3)
        self.wait("Find focused", lambda d: d["find_focused"])
        self.native("text", "alpha")
        self.wait("query received", lambda d: d["find_query_fingerprint"] == fingerprint("alpha") and d["find_selected"] is not None)
        self.source_is(text)
        self.key(0)  # Select All must affect the query, never the source.
        self.native("text", "beta")
        self.wait("query replacement isolated", lambda d: d["find_query_fingerprint"] == fingerprint("beta"))
        self.source_is(text)
        self.key(0)
        self.native("text", "alpha")
        self.wait("search reset", lambda d: d["find_selected"] is not None)
        first = self.snapshot()["document"]["find_selected"]
        self.key(36, "none")
        self.wait("Enter advances match", lambda d: d["find_selected"] is not None and d["find_selected"] != first)
        self.key(36, "shift")
        self.wait("Shift+Enter returns match", lambda d: d["find_selected"] == first)
        self.key(8, "cmd+alt")
        self.wait("case toggle", lambda d: not d["find_case"])
        self.key(8, "cmd+alt")
        self.wait("case restores", lambda d: d["find_case"])
        self.key(7, "cmd+alt")
        self.wait("regex toggle", lambda d: d["find_regex"])
        self.key(7, "cmd+alt")
        self.wait("regex restores", lambda d: not d["find_regex"])
        self.click("find.replace")
        self.wait("replace fields open", lambda d: d["replace_visible"])
        self.wait_target("find.replacement")
        self.click("find.replacement")
        self.native("text", "omega")
        self.click("find.replace_one")
        self.wait("replace one edits only one occurrence", lambda d: d["source_fingerprint"] in
                  (fingerprint("omega beta alpha ALPHA"), fingerprint("alpha beta omega ALPHA")))
        self.click("find.replace_all")
        self.source_is("omega beta omega ALPHA")
        self.key(53, "none")
        self.wait("Escape closes Find", lambda d: not d["find_visible"])
        self.key(6)
        self.wait("replace all undoes once", lambda d: d["source_fingerprint"] in
                  (fingerprint("omega beta alpha ALPHA"), fingerprint("alpha beta omega ALPHA")))
        self.key(6)
        self.source_is(text)
        self.close_scratch(count)

    def tabs(self):
        original = self.snapshot()["document"]
        count = self.scratch("first scratch")
        self.key(123, "none")
        self.wait("caret moved", lambda d: d["cursor"] == [12, 12])
        self.scratch("second scratch")
        self.key(48, "ctrl+shift")
        self.source_is("first scratch")
        self.wait("previous tab restores caret", lambda d: d["cursor"] == [12, 12])
        self.key(48, "ctrl")
        self.source_is("second scratch")
        self.wait("next tab restores caret", lambda d: d["cursor"] == [14, 14])
        self.close_scratch(count + 1)
        self.source_is("first scratch")
        self.close_scratch(count)
        self.wait("original tab and pinned preview retained", lambda d: d["path"] == original["path"]
                  and d["source_fingerprint"] == original["source_fingerprint"] and d["preview_path"] == original["preview_path"])

    def tab_drag(self):
        count = self.scratch("first scratch")
        self.scratch("second scratch")
        before = self.snapshot()["document"]["tab_order"]
        if len(before) != count + 2:
            raise AssertionError(f"unexpected tab order before drag: {before}")
        source = f"tab.{before[-1]}"
        destination = f"tab.{before[0]}"
        self.drag(source, destination)
        self.wait("tab drag reorders the native strip", lambda d: d["tab_order"] == [before[-1], *before[:-1]])
        self.click(f"tab.{before[1]}")
        self.wait("first scratch remains selectable after drag", lambda d: d["active_tab_id"] == before[1])
        self.close_scratch(count + 1)
        self.click(f"tab.{before[-1]}")
        self.close_scratch(count)
        self.wait("tab drag preserves the original tab", lambda d: d["tabs"] == count and d["path"] is not None)

    def dialogs(self):
        opened = self.directory / "opened.tex"
        opened.write_text("\\documentclass{article}\n\\begin{document}\nOpened.\n\\end{document}\n")
        self.key(31, "cmd")  # Cmd+O
        import time
        time.sleep(0.8)  # rfd creates the native panel asynchronously.
        self.native("ax-press-description", "search")
        self.native("text", opened.name)
        time.sleep(0.8)
        self.native("ax-press-title", "Open")
        self.wait("native Open panel loads a TeX file", lambda d: d["path"] == str(opened), timeout=15)

        self.key(45)  # New empty document.
        self.wait("dialog journey gets an empty tab", lambda d: d["path"] is None and d["source_bytes"] == 0)
        destination = self.directory / "saved.arbitrary"
        self.save_to_fixture(destination)
        self.wait("Save As accepts an arbitrary extension", lambda d: d["path"] == str(destination), timeout=15)

    def startup_preview(self):
        source = self.directory / "journey.typ"
        self.wait("plain launch starts with an unbound startup buffer", lambda d: d["tabs"] == 1 and d["path"] is None and d["source_bytes"] > 0)
        self.key(31, "cmd")  # Cmd+O
        import time
        time.sleep(0.8)
        self.native("ax-press-description", "search")
        self.native("text", source.name)
        time.sleep(0.8)
        self.native("ax-press-title", "Open")
        self.wait("first opened source becomes active", lambda d: d["path"] == str(source), timeout=15)
        self.wait("first opened source becomes the preview target", lambda d: d["preview_path"] == str(source) and d["preview_tab_id"] == d["active_tab_id"], timeout=30)

    def diagnostics(self):
        current_path = self.snapshot()["document"]["path"] or ""
        if not current_path.endswith((".typ", ".tex")):
            # The dialog journey deliberately leaves an arbitrary-extension
            # tab active. Move back to the original Typst owner before
            # asking the compiler for diagnostics.
            for _ in range(2):
                self.key(48, "ctrl+shift")
            self.wait("diagnostic journey selects a typesetting tab", lambda d: (d["path"] or "").endswith(".typ"))
        before = self.snapshot()["document"]
        self.click("editor.source")
        self.key(0)
        self.paste_source("#let broken = {\n")
        self.key(23)  # Problems
        self.wait("Problems exposes compiler diagnostics", lambda d: d["panel"] == "Problems" and d["diagnostic_count"] > 0, timeout=30)
        self.wait_target("problem.0")
        self.double_click("problem.0")
        self.wait("diagnostic navigation returns focus to the editor", lambda d: d["focused"] and d["panel"] == "Problems")
        self.key(6, "cmd")
        self.wait("diagnostic fixture undo restores the prior source", lambda d: d["source_fingerprint"] == before["source_fingerprint"] and d["source_bytes"] == before["source_bytes"])
        self.key(23)
        self.wait("diagnostic journey closes its panel", lambda d: d["panel"] is None)

    def rectangle(self):
        # Optional local integration: preserve the user's bindings and settings.
        import pathlib
        import plistlib
        import subprocess
        subprocess.run(["pgrep", "-x", "Rectangle"], check=True, capture_output=True)
        preferences = plistlib.loads((pathlib.Path.home() / "Library/Preferences/com.knollsoft.Rectangle.plist").read_bytes())
        self.key(3)
        self.wait("Find focused before Rectangle shortcuts", lambda d: d["find_visible"] and d["find_focused"])
        for name in ("almostMaximize", "bottomLeftSixth"):
            binding = preferences[name]
            flags = "+".join(name for bit, name in ((1 << 20, "cmd"), (1 << 19, "alt"), (1 << 18, "ctrl"), (1 << 17, "shift")) if binding["modifierFlags"] & bit) or "none"
            before = self.snapshot()["document"]["native_size"]
            self.record("window-manager.shortcut", window_action=name, keycode=binding["keyCode"], modifiers=flags)
            self.key(binding["keyCode"], flags)
            self.wait("Rectangle " + name + " resizes Find's owner", lambda d: d["native_size"] != before and d["find_visible"] and d["find_focused"])
            self.stable_state(lambda d: d["find_visible"] and d["find_focused"])
        self.click("find.close")
        self.wait("Find still closes normally after Rectangle commands", lambda d: not d["find_visible"])

    def drawing(self):
        count = self.scratch()
        if not self.snapshot()["document"]["explorer_visible"]:
            self.key(18)
        self.click("explorer.maximize.Draw symbol")
        self.wait("drawing panel maximized", lambda d: d["explorer_maximized"] == "Draw symbol")
        self.wait_target("drawing.canvas")
        target = self.snapshot()["targets"]["drawing.canvas"]
        # Three genuine pen strokes form an A; no fixture state injection.
        x,y = target["x"],target["y"]
        for a,b in [((-55,70),(0,-70)), ((0,-70),(55,70)), ((-35,25),(35,25))]:
            self.native("drag", x+a[0], y+a[1], x+b[0], y+b[1])
        state = self.wait("drawing recognized", lambda d: len(d["drawing"]["strokes"]) == 3 and not d["drawing"]["busy"] and any(p["detexify"] for p in d["drawing"]["predictions"]), timeout=30)
        strokes = state["document"]["drawing"]["strokes"]
        self.click("drawing.undo")
        self.wait("Undo removes one stroke", lambda d: len(d["drawing"]["strokes"]) == 2)
        self.click("drawing.redo")
        self.wait("Redo restores exact ink", lambda d: d["drawing"]["strokes"] == strokes)
        self.native("resize-focused", 1100, 700)
        old_size = state["document"]["native_size"]
        self.wait("drawing survives resize", lambda d: d["native_size"] != old_size and d["drawing"]["strokes"] == strokes)
        self.wait_target("drawing.canvas")
        prediction = next(p for p in self.snapshot()["document"]["drawing"]["predictions"] if p["typst"] is not None)
        self.click("drawing.prediction." + str(prediction["index"]))
        self.source_is(prediction["typst"])
        self.key(6)
        self.source_is("")
        self.settings()
        self.setting_section("handwriting", "Handwriting recognition")
        self.click("settings.handwriting.Detypify")
        self.click("settings.handwriting.names")
        self.close_settings()
        state = self.wait("only Detypify re-recognizes retained ink", lambda d: d["drawing"]["engine"] == "detypify" and not d["drawing"]["busy"] and bool(d["drawing"]["predictions"]) and all(not p["detexify"] for p in d["drawing"]["predictions"]), timeout=30)
        assert state["document"]["drawing"]["strokes"] == strokes
        prediction = state["document"]["drawing"]["predictions"][0]
        assert not prediction["typst"].startswith("#sym."), "Unicode preference should apply"
        self.click("drawing.prediction." + str(prediction["index"]))
        self.source_is(prediction["typst"])
        self.settings()
        self.setting_section("handwriting", "Handwriting recognition")
        self.click("settings.handwriting.Detexify")
        self.click("settings.handwriting.names")
        self.close_settings()
        self.click("drawing.clear")
        self.wait("Clear discards ink and old predictions", lambda d: not d["drawing"]["strokes"] and not d["drawing"]["predictions"] and not d["drawing"]["busy"])
        self.click("explorer.maximize.Draw symbol")
        self.close_scratch(count)

    def snippets(self):
        import json
        self.settings()
        self.click("settings.json.mode")
        self.wait("JSON settings ready", lambda d: d["settings_json"]["visible"])
        self.key(3)
        self.wait_target("settings.json.find")
        self.native("text", '"snippets": []')
        self.key(36, "none")
        before = self.snapshot()["document"]["settings_json"]["fingerprint"]
        settings = {"snippets": [{"prefix": "env", "description": "Linked environment", "language": "both", "body": "\\begin{${1:name}}\n\\end{$1}$0"}]}
        encoded = json.dumps(settings)[1:-1]
        self.paste_source(encoded, lambda: self.wait("snippet replacement validates", lambda d: d["settings_json"]["fingerprint"] != before and d["settings_json"]["valid"]))
        self.click("settings.json.save")
        self.wait("snippet settings saved", lambda d: d["settings_json"]["saved"])
        self.close_settings()
        count = self.scratch("env")
        self.key(49, "ctrl")
        self.wait("custom snippet offered", lambda d: d["completion_count"] > 0)
        self.key(48, "none")
        self.source_is("\\begin{name}\n\\end{name}")
        self.native("text", "enumerate")
        self.source_is("\\begin{enumerate}\n\\end{enumerate}")
        self.key(6)
        self.source_is("\\begin{name}\n\\end{name}")
        self.key(6, "cmd+shift")
        self.source_is("\\begin{enumerate}\n\\end{enumerate}")
        self.close_scratch(count)

    def completion(self):
        count = self.scratch("#mi(`\\alp`)")
        self.key(123, "none")
        self.key(123, "none")
        self.key(49, "ctrl")  # Explicit completion request.
        self.wait("completion popup receives a real response", lambda d: d["completion_visible"] and d["completion_count"] > 0, timeout=30)
        self.wait_target("completion.item.0")
        self.key(53, "none")  # Escape dismisses without editing the source.
        self.wait("completion dismisses without changing source", lambda d: not d["completion_visible"])
        self.source_is("#mi(`\\alp`)")
        self.close_scratch(count)

    def layout(self):
        # The dialog journey intentionally leaves an arbitrary-extension tab
        # active. View-mode and panel commands are document-scoped, so make
        # this journey self-contained instead of relying on aggregate order.
        self.ensure_typesetting_tab()
        initial = self.snapshot()["document"]
        for key, mode in ((19, "Code"), (21, "Preview"), (20, "Split")):
            self.key(key)
            self.wait("view mode " + mode, lambda d: d["view_mode"] == mode)
        self.key(18)
        self.wait("explorer hides", lambda d: not d["explorer_visible"])
        self.key(18)
        self.wait("explorer reopens", lambda d: d["explorer_visible"])
        for section in ("Tags", "Files"):
            self.wait_target("explorer.maximize." + section)
            self.click("explorer.maximize." + section)
            self.wait("explorer section maximized", lambda d: d["explorer_maximized"] == section)
            for _ in range(3):
                state = self.snapshot()
                visible = {name for name, target in state["targets"].items()
                           if name.startswith("explorer.maximize.") and target["frame"] == state["document"]["frame"]}
                if visible != {"explorer.maximize." + section}:
                    raise AssertionError(f"maximized section left sibling controls visible: {visible}")
            self.click("explorer.maximize." + section)
            self.wait("explorer sections restored", lambda d: d["explorer_maximized"] is None)
        for code, field in ((6, "line_wrap"), (45, "line_numbers"), (17, "sticky_context")):
            self.key(code, "cmd+alt")
            self.wait("editor setting toggled", lambda d: d[field] != initial[field])
            self.key(code, "cmd+alt")
            self.wait("editor setting restored", lambda d: d[field] == initial[field])
        self.key(23)
        document = self.wait("panel opens", lambda d: d["panel"] is not None)["document"]
        baseline = document["panel_rect"][3] - document["panel_rect"][1]
        self.click("panel.maximize")
        self.wait("maximize button", lambda d: d["panel_maximized"] and d["panel_rect"][3] - d["panel_rect"][1] > baseline)
        self.key(23, "cmd+alt")
        self.wait("maximize shortcut restores", lambda d: not d["panel_maximized"])
        self.stable(baseline)
        self.click("panel.activity")
        self.wait("Activity selected", lambda d: d["panel"] == "Activity")
        self.stable(baseline)
        self.click("panel.close")
        self.wait("close panel button", lambda d: d["panel"] is None)
        self.wait("layout leaves source unchanged", lambda d: d["source_fingerprint"] == initial["source_fingerprint"])

    def explorer_scroll(self):
        self.ensure_typesetting_tab()
        import time
        def scroll_stays(section, sticky=False):
            deadline = time.monotonic() + 10
            while True:
                state = self.snapshot()
                scroll = state["scrolls"].get(section)
                if (scroll and scroll["frame"] == state["document"]["frame"]
                        and scroll["content_height"] > scroll["viewport_height"] + 100
                        and scroll["sticky_search"] == sticky):
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(f"{section} did not expose a scrollable list")
                time.sleep(0.05)
            before = scroll["offset"]
            self.native("move", scroll["x"], scroll["y"])
            self.native("wheel", scroll["x"], scroll["y"], -8)
            deadline = time.monotonic() + 3
            while True:
                after = self.snapshot()["scrolls"][section]["offset"]
                if after > before + 20:
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(f"{section} did not scroll after wheel input: {before} -> {after}")
                time.sleep(0.05)
            for _ in range(8):
                after = self.snapshot()["scrolls"][section]["offset"]
                if after <= before + 20:
                    raise AssertionError(f"{section} scroll reset after wheel input: {before} -> {after}")
            self.record("explorer.scroll.stable", section=section, sticky=sticky, before=before, after=after)

        scroll_stays("Files")
        if self.explorer_scroll_checks_contents:
            scroll_stays("Contents")
            files = self.snapshot()["scrolls"]["Files"]
            self.native("click", files["x"], files["y"])
            self.wait("Files receives focus", lambda d: d["explorer_focused"] == "Files")
            self.key(3)  # Cmd+F exposes the sticky Files search.
            scroll_stays("Files", sticky=True)
            if getattr(self, "capture_review", False):
                self.capture_viewport("main")

    def folding(self):
        text = "#let block = {\n  let a = 1\n  a + 2\n}\n\nEnd."
        count = self.scratch(text)
        self.key(126, "cmd")
        self.wait("fold header available", lambda d: len(d["fold_headers"]) > 0 and d["cursor"] == [0, 0])
        headers = self.snapshot()["document"]["fold_headers"]
        for _ in range(3):
            self.key(37, "cmd+alt")
            self.wait("fold collapses", lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers)
            self.stable_state(lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers)
            self.key(37, "cmd+alt")
            self.wait("fold expands", lambda d: not d["fold_collapsed"] and d["fold_headers"] == headers)
        self.key(125, "cmd")
        self.wait("caret at document end", lambda d: d["cursor"] == [len(text), len(text)])
        target = "fold." + str(headers[0])
        self.wait_target(target)
        self.click(target)
        self.wait("gutter preserves visible caret", lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers and d["cursor"] == [len(text), len(text)])
        self.wait_target(target)
        self.click(target)
        self.wait("same gutter control expands", lambda d: not d["fold_collapsed"] and d["fold_headers"] == headers)
        self.source_is(text)
        self.close_scratch(count)
        self.folding_large()

    def folding_large(self):
        prefix = "\n".join(f"Prose line {i}." for i in range(8)) + "\n"
        block = "#let large = {\n" + "\n".join(f"  let value{i} = {i}" for i in range(180)) + "\n}\n"
        suffix = "\n".join(f"Following prose {i}." for i in range(100))
        text = prefix + block + suffix
        count = self.scratch(text)
        self.key(125, "cmd")
        self.wait("large document caret at end", lambda d: d["cursor"] == [len(text), len(text)])
        self.stable_state(lambda d: d["cursor"] == [len(text), len(text)])
        target = "fold.8"
        def scroll_to_header():
            for _ in range(30):
                state = self.snapshot()
                scroll = state["scrolls"]["editor"]
                if scroll["offset"] < 1:
                    break
                self.native("move", scroll["x"], scroll["y"])
                self.native("wheel", scroll["x"], scroll["y"], 100)
            self.wait_target(target)
        scroll_to_header()
        self.click(target)
        self.wait("large fold preserves distant caret", lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        self.stable_state(lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        assert self.snapshot()["scrolls"]["editor"]["offset"] < 1, "fold must not scroll to the distant caret"
        self.click(target)
        self.wait("large block expands without moving caret", lambda d: not d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        # Put the caret inside the block through ordinary keyboard navigation.
        self.click("editor.source")
        self.key(126, "cmd")
        for _ in range(18): self.key(125, "none")
        inside = self.snapshot()["document"]["cursor"][0]
        assert len(prefix) < inside < len(prefix + block)
        scroll_to_header()
        self.click(target)
        header_end = len(prefix) + len("#let large = {")
        self.wait("hidden caret moves to visible fold header", lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [header_end, header_end])
        self.stable_state(lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [header_end, header_end])
        self.source_is(text)
        self.close_scratch(count)

    def selection_wrap(self):
        text = "α🦀 words"
        count = self.scratch(text)
        for opening, closing in (("(", ")"), ("[", "]"), ("{", "}"), ("$", "$"), ('"', '"'), ("*", "*"), ("_", "_"), ("`", "`")):
            self.key(0)
            self.native("text", opening)
            self.source_is(opening + text + closing)
            self.key(6)
            self.source_is(text)
            self.key(6, "cmd+shift")
            self.source_is(opening + text + closing)
            self.key(6)
            self.source_is(text)
        self.close_scratch(count)

    def git_hunks(self):
        source = self.directory / "hunks.typ"
        if self.snapshot()["document"]["path"] != str(source):
            import time
            self.key(31)
            time.sleep(0.8)
            self.native("ax-press-description", "search")
            self.native("text", source.name)
            time.sleep(0.8)
            self.native("ax-press-title", "Open")
            self.wait("hunk fixture opened", lambda d: d["path"] == str(source), timeout=15)
        lines = source.read_text().splitlines(keepends=True)
        original = "".join(lines)
        self.choose_backend("Pdfium")
        self.key(20)  # Split: the docked editor must preserve this resizable pane.
        self.wait("split editor shown", lambda d: d["view_mode"] == "Split" and d["editor_panel_width"] is not None)
        self.wait("three separated Git hunks", lambda d: d["git_hunk_count"] == 3)
        self.settings()
        self.setting_section("independent main dividers", "Independent main dividers")
        self.click("settings.dividers")
        self.wait("independent dividers enabled", lambda d: d["independent_main_dividers"])
        self.close_settings()
        self.wait_target("divider.explorer")
        state = self.snapshot()
        border = state["targets"]["divider.explorer"]
        preview_edge = state["targets"]["divider.editor"]["x"]
        from concurrent.futures import ThreadPoolExecutor
        moving_frames = 0
        with ThreadPoolExecutor(max_workers=1) as worker:
            drag = worker.submit(self.native, "drag", border["x"], border["y"], border["x"] + 35, border["y"])
            while not drag.done():
                frame = self.snapshot()
                if abs(frame["targets"]["divider.explorer"]["x"] - border["x"]) > 0.5:
                    moving_frames += 1
                if abs(frame["targets"]["divider.editor"]["x"] - preview_edge) > 1:
                    raise AssertionError("preview border moved during independent divider drag")
            drag.result()
        self.wait("Explorer divider moved", lambda d: abs(d["explorer_panel_width"] - state["document"]["explorer_panel_width"] - 35) < 2)
        self.record("independent_resize.frames", intermediate=moving_frames)
        moved = self.snapshot()
        if abs(moved["targets"]["divider.editor"]["x"] - preview_edge) > 1:
            raise AssertionError("independent Explorer resize moved preview border")
        if abs(moved["targets"]["divider.explorer"]["x"] - border["x"] - 35) > 2:
            raise AssertionError("Explorer divider did not follow the drag")
        self.settings()
        self.setting_section("independent main dividers", "Independent main dividers")
        self.click("settings.dividers")
        self.wait("fixed code width mode enabled", lambda d: not d["independent_main_dividers"])
        self.close_settings()
        self.wait_target("divider.explorer")
        state = self.snapshot()
        border = state["targets"]["divider.explorer"]
        width = state["document"]["editor_panel_width"]
        self.native("drag", border["x"], border["y"], border["x"] - 35, border["y"])
        self.stable_state(lambda d: abs(d["editor_panel_width"] - width) < 1)

        self.click("editor.source")
        self.key(126, "cmd")
        for line in (5, 85, 165, 5):
            self.key(125, "cmd+alt")
            self.wait("keyboard hunk jump keeps normal editor focused", lambda d: d["editor_keyboard_focused"]
                      and not d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])))
        self.wait_target("git.hunk.0")
        target = self.snapshot()["targets"]["git.hunk.0"]
        self.native("move", target["x"], target["y"])
        self.wait("read-only hunk hover preserves editor focus", lambda d: d["hover_tooltip_open"] and not d["git_diff_open"] and d["editor_keyboard_focused"])
        self.click("git.hunk.0")
        self.wait("full document diff opens at clicked change", lambda d: d["git_diff_open"] and d["git_diff_lines"] == [180, 180]
                  and d["cursor"][0] == sum(map(len, lines[:5])))
        for button, line in (("Next",85), ("Next",165), ("Next",5), ("Previous",165)):
            self.click("git.hunk." + button)
            self.wait("navigation moves live editor cursor", lambda d: d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])) and d["editor_keyboard_focused"])
            self.stable_state(lambda d: d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])))
        self.native("resize-focused", 1400, 850)
        self.wait("document diff resizes with owner", lambda d: d["native_size"] == [1400, 850] and d["git_diff_open"])
        self.click("git.hunk.Next")  # Wrap to first changed line.
        self.wait("first change selected", lambda d: d["cursor"][0] == sum(map(len, lines[:5])))
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        pair = self.snapshot()["document"]["git_diff_changed_pairs"][0]
        self.click("git.hunk.line." + str(pair))
        expected = original.replace("005 changed", "005 original")
        self.source_is(expected)
        self.wait("line revert immediately refreshes live diff", lambda d: d["git_diff_open"] and len(d["git_diff_hunks"]) == 2)
        self.key(6)
        self.source_is(original)
        self.wait("undo restores live highlights", lambda d: len(d["git_diff_hunks"]) == 3)
        builds = self.snapshot()["document"]["git_diff_builds"]
        self.key(126, "cmd")
        self.native("text", "Prefix ")
        self.source_is("Prefix " + original)
        self.wait("editing outside original hunk refreshes full diff", lambda d: d["git_diff_open"] and d["git_diff_changed_pairs"] == [0, 5, 85, 165] and d["git_diff_hunks"] == [0, 85, 165] and d["git_diff_builds"] > builds)
        self.key(6)
        self.source_is(original)
        self.key(6, "cmd+shift")
        self.source_is("Prefix " + original)
        self.key(6)
        self.source_is(original)
        self.key(125, "cmd")
        self.native("text", "Extra line")
        self.source_is(original + "Extra line")
        self.wait("last line is editable and not hidden", lambda d: d["git_diff_lines"] == [180, 181])
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        self.key(6)
        self.source_is(original)
        width = self.snapshot()["document"]["editor_panel_width"]
        self.key(53, "none")
        self.wait("Escape returns to code without resizing pane", lambda d: not d["git_diff_open"] and d["editor_keyboard_focused"] and abs(d["editor_panel_width"] - width) < 1)
        self.stable_state(lambda d: abs(d["editor_panel_width"] - width) < 1)
        self.wait("original gutter markers refreshed", lambda d: d["git_hunk_count"] == 3)
        self.key(126, "cmd")
        self.wait("source cursor returned to start", lambda d: d["cursor"] == [0, 0])
        self.wait_target("git.hunk.0")
        self.click("git.hunk.0")
        self.wait("reopening uses fresh full-document baseline", lambda d: d["git_diff_open"] and d["git_diff_lines"] == [180, 180] and len(d["git_diff_hunks"]) == 3)
        self.key(3)
        self.wait("Find owns keyboard while document diff stays open", lambda d: d["find_focused"] and d["git_diff_open"])
        self.native("text", "Line 085")
        self.wait("Find query updates without stealing focus", lambda d: d["find_focused"] and d["find_query_fingerprint"] == fingerprint("Line 085") and d["find_selected"] is not None)
        self.source_is(original)
        self.key(3)
        self.wait("closing Find preserves document diff", lambda d: not d["find_visible"] and d["git_diff_open"] and d["editor_keyboard_focused"])
        self.choose_backend("Interactive")
        self.wait_target("git.hunk.Next")
        target = self.snapshot()["targets"]["git.hunk.Next"]
        self.native("move", target["x"], target["y"])
        self.wait("docked diff tooltip gets its own native window above preview", lambda d: d["git_diff_open"] and d["hover_tooltip_open"] and d["native_hover_window"])
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        # Action shortcuts must target the live caret, not the initially opened hunk.
        self.click("git.hunk.editor")
        self.key(126, "cmd")
        self.key(125, "cmd+alt")
        self.key(125, "cmd+alt")
        at = sum(len(line) for line in original.splitlines(keepends=True)[:85])
        self.wait("diff cursor reaches second hunk", lambda d: d["cursor"] == [at, at])
        self.key(1, "cmd+alt")  # Stage the second hunk.
        import subprocess
        expected_index = original.replace("005 changed", "005 original").replace("165 changed", "165 original")
        self.wait("keyboard stages only the hunk under the diff caret", lambda d:
            subprocess.run(["git", "-C", str(self.directory), "show", ":hunks.typ"],
                           capture_output=True, text=True, check=True).stdout == expected_index)
        self.key(32, "cmd+alt")  # Unstage the same hunk (U).
        committed = expected_index.replace("085 changed", "085 original")
        self.wait("keyboard unstages the caret hunk", lambda d:
            subprocess.run(["git", "-C", str(self.directory), "show", ":hunks.typ"],
                           capture_output=True, text=True, check=True).stdout == committed)


    def preview_keyboard(self):
        self.ensure_typesetting_tab()
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            self.click("editor.source")
            self.key(125, "cmd")
            before = self.snapshot()["document"]
            self.key(15)  # Compile/reload must preserve the source keyboard owner.
            self.wait("preview ready after compile", lambda d: d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"])
            target = self.snapshot()["targets"]["preview.page"]
            self.native("move", target["x"], target["y"])
            self.native("wheel", target["x"], target["y"], -3)
            self.stable_state(lambda d: d["editor_keyboard_focused"] and not d["preview_keyboard_focused"])
            self.native("text", "t")
            self.wait("typing stays in source after preview build", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
            self.key(6)
            self.wait("undo restores source", lambda d: d["source_fingerprint"] == before["source_fingerprint"])
            for iteration in range(3):
                self.click("preview.page")
                self.wait("preview owns keyboard after click", lambda d: d["preview_keyboard_focused"] and not d["editor_keyboard_focused"])
                if backend == "Interactive":
                    self.native("text", "t")
                    self.wait("focused preview accepts its shortcut", lambda d: d["page_dark"] != before["page_dark"] and d["source_fingerprint"] == before["source_fingerprint"])
                    self.native("text", "t")
                    self.wait("second inversion restores page", lambda d: d["page_dark"] == before["page_dark"])
                if getattr(self, "capture_review", False) and backend == "Pdfium" and iteration == 0:
                    self.capture_viewport("main")
                self.key(53, "none")
                self.wait("Escape returns to source", lambda d: d["editor_keyboard_focused"] and not d["preview_keyboard_focused"])
                self.native("text", "t")
                self.wait("t edits source instead of inverting preview", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
                self.key(6)
                self.wait("source restored after focus roundtrip", lambda d: d["source_fingerprint"] == before["source_fingerprint"])

            self.click("preview.page")
            self.wait("preview focused before hiding", lambda d: d["preview_keyboard_focused"])
            self.key(19)  # Cmd+2: hiding a focused preview must hand keyboard ownership to source.
            self.wait("code-only view selected", lambda d: d["view_mode"] == "Code")
            self.native("text", "t")
            self.wait("hidden preview cannot retain keyboard input", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
            self.key(6)
            self.key(20)  # Cmd+3 restores split view for the next backend.
            self.wait("split view restored", lambda d: d["view_mode"] == "Split")

    def closing(self):
        text = "Unsaved fixture must survive cancellation."
        count = self.scratch(text)
        self.key(13)
        self.wait("dirty close asks first", lambda d: d["modal_open"] and d["tabs"] == count + 1)
        self.wait_target("modal.cancel")
        self.click("modal.cancel")
        self.wait("Cancel preserves dirty tab", lambda d: not d["modal_open"] and d["dirty"] and d["tabs"] == count + 1)
        self.source_is(text)
        self.key(13)
        self.wait("close asks again", lambda d: d["modal_open"])
        self.key(53, "none")
        self.wait("Escape cancels dirty close", lambda d: not d["modal_open"] and d["tabs"] == count + 1)
        self.source_is(text)
        self.key(13)
        self.wait("explicit discard required", lambda d: d["modal_open"])
        self.wait_target("modal.discard")
        self.click("modal.discard")
        self.wait("discard closes only fixture tab", lambda d: not d["modal_open"] and d["tabs"] == count)

    def choose_backend(self, backend, save_pdf=True):
        self.settings()
        if self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
        self.setting_section("backend", "Typst preview backend")
        self.wait_target("settings.backend." + backend)
        self.click("settings.backend." + backend)
        self.close_settings()
        if save_pdf:
            self.key(15)
        else:
            self.wait_target("build.preview")
            generation = self.snapshot()["document"]["preview_generation"]
            self.click("build.preview")
        self.wait("selected renderer ready", lambda d: d["preview_requested_backend"] == backend
                  and d["preview_backend"] == ("PDFium" if backend == "Pdfium" else "Tinymist")
                  and d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"]
                  and (save_pdf or (not d["compile_indicator_spinning"] and
                       (backend != "Interactive" or (d["preview_generation"] != "None" and d["preview_generation"] != generation)))), timeout=60)

    def pdf_output(self):
        """Preview activity must never imply an adjacent PDF was saved."""
        import time
        self.focus_document_window()
        before = self.snapshot()["document"]
        if before["find_visible"]:
            self.click("find.close")
            self.wait("PDF journey closes earlier Find", lambda d: not d["find_visible"])
        fixture_directory = self.directory / "pdf-output"
        fixture_directory.mkdir()
        fixture = fixture_directory / "journey.typ"
        fixture.write_text(PREVIEW_SOURCE)
        destination = fixture.with_suffix(".pdf")
        # The aggregate run can already have a journey.pdf from other tests.
        # A new document path isolates output ownership without deleting files.
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(fixture))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("isolated three-page Typst document opened", lambda d: d["path"] == str(fixture), timeout=15)
        self.key(35, "cmd+alt")
        self.wait("PDF source is designated for preview", lambda d: d["preview_tab_id"] == d["active_tab_id"] and d["preview_path"] == str(fixture), timeout=30)
        self.key(20)  # Split view so readiness refers to a visible renderer.
        self.tex_settings([('"auto_save": true', '"auto_save": false')])
        self.choose_backend("Interactive", save_pdf=False)
        self.wait("Tinymist preview settles without a disk export", lambda d: d["preview_backend"] == "Tinymist" and d["preview_native_ready"] and d["preview_pages"] == 3 and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Unverified" and not d["pdf_output_pending"] and not destination.exists())

        self.wait_target("build.pdf")
        self.click("build.pdf")
        saved = self.wait("PDF button confirms a committed file", lambda d: d["pdf_output_state"] == "Saved" and str(d["pdf_output_revision"]) == d["revision"] and not d["pdf_output_pending"], timeout=60)
        first_pdf = destination.read_bytes()
        if not first_pdf.startswith(b"%PDF-"):
            raise AssertionError("PDF action did not create a PDF document")
        first_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == first_pdf and destination.stat().st_mtime_ns == first_mtime, frames=30)
        self.record("pdf.saved", revision=saved["document"]["revision"], bytes=len(first_pdf))
        if getattr(self, "capture_review", False):
            self.capture_viewport("main")

        edited = PREVIEW_SOURCE.replace("Black text on white paper.", "A changed paragraph awaiting PDF export.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(edited)
        self.wait("typing marks the saved PDF stale", lambda d: d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"])
        generation = self.snapshot()["document"]["preview_generation"]
        self.click("build.preview")
        self.wait("updated Tinymist preview settles", lambda d: d["preview_backend"] == "Tinymist" and d["preview_native_ready"] and d["preview_pages"] == 3 and not d["compile_indicator_spinning"] and d["preview_generation"] != "None" and d["preview_generation"] != generation, timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"] and destination.read_bytes() == first_pdf and destination.stat().st_mtime_ns == first_mtime)

        self.key(15)  # Cmd+R explicitly saves PDF; engine refresh above did not.
        self.wait("Cmd+R updates the PDF to the new source revision", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        second_pdf = destination.read_bytes()
        if not second_pdf.startswith(b"%PDF-") or second_pdf == first_pdf:
            raise AssertionError("Cmd+R did not update the saved PDF contents")
        second_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == second_pdf and destination.stat().st_mtime_ns == second_mtime, frames=30)
        self.click("editor.source")
        self.key(0)
        self.paste_source("#let broken = {\n")
        self.click("build.pdf")
        self.wait("failed PDF creation is visible and not queued", lambda d: d["pdf_output_state"] == "Failed" and not d["pdf_output_pending"] and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Failed" and not d["pdf_output_pending"] and destination.read_bytes() == second_pdf and destination.stat().st_mtime_ns == second_mtime, frames=30)

        recovered = PREVIEW_SOURCE.replace("Black text on white paper.", "Recovered after a deliberate syntax error.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(recovered)
        self.wait_target("build.pdf")
        self.click("build.pdf")
        self.wait("PDF action recovers after fixing the source", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        recovered_pdf = destination.read_bytes()
        if not recovered_pdf.startswith(b"%PDF-") or recovered_pdf == second_pdf:
            raise AssertionError("PDF retry did not write corrected source")
        recovered_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == recovered_pdf and destination.stat().st_mtime_ns == recovered_mtime, frames=30)
        self.choose_backend("Pdfium", save_pdf=False)
        self.wait("PDFium cache build settles without exporting", lambda d: d["preview_backend"] == "PDFium" and d["preview_pdfium_ready"] and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: not d["pdf_output_pending"] and destination.read_bytes() == recovered_pdf and destination.stat().st_mtime_ns == recovered_mtime)
        self.record("pdf.preview-only", bytes=len(recovered_pdf), unchanged_on_backend_switch=True)

        # Restore aggregate-run ownership and settings without overwriting its files.
        self.tex_settings([('"auto_save": false', '"auto_save": true')])
        self.choose_backend(before["preview_requested_backend"], save_pdf=False)
        self.close_scratch(before["tabs"])
        if before["preview_tab_id"] is not None:
            self.click("tab." + str(before["preview_tab_id"]))
            self.key(35, "cmd+alt")
        if before["active_tab_id"] is not None:
            self.click("tab." + str(before["active_tab_id"]))
        if before["view_mode"] != "Split":
            self.key(19 if before["view_mode"] == "Code" else 21)

    def controls(self):
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            self.wait("three-page preview loaded", lambda d: d["preview_pages"] == 3, timeout=30)
            if not self.snapshot()["document"]["preview_controls_open"]:
                if backend == "Pdfium":
                    target = self.snapshot()["targets"]["preview.open"]
                    self.native("move", target["x"], target["y"])
                    self.wait("toolbar tooltip appears before opening controls", lambda d: d["hover_tooltip_open"] and d["hover_tooltip_rect"] is not None)
                    card = self.snapshot()["document"]["hover_tooltip_rect"]
                    if card[2] >= 250:
                        raise AssertionError("short toolbar tooltip retained a documentation-sized minimum width")
                    if self.capture_review:
                        state = self.snapshot()["document"]
                        pointer = state["pointer"]
                        self.native("click", card[0] + card[2] / 2 + target["x"] - pointer[0],
                                    card[1] + card[3] / 2 + target["y"] - pointer[1])
                        self.capture_viewport("diagnostic")
                self.click("preview.open")
                self.wait("opening controls dismisses the toolbar tooltip", lambda d: not d["hover_tooltip_open"])
            self.wait("controls opened", lambda d: d["preview_controls_open"])
            before_position = self.wait("controls position measured", lambda d: d["preview_controls_position"] is not None)["document"]["preview_controls_position"]
            self.wait_target("preview.drag_handle")
            handle = self.snapshot()["targets"]["preview.drag_handle"]
            self.native("drag", handle["x"], handle["y"], handle["x"] + 48, handle["y"] + 24)
            self.wait("preview controls drag persists", lambda d: d["preview_controls_position"] is not None and
                      abs(d["preview_controls_position"][0] - before_position[0] - 48) <= 3 and
                      abs(d["preview_controls_position"][1] - before_position[1] - 24) <= 3)
            self.wait_target("preview.Outline")
            if not self.snapshot()["document"]["preview_outline"]:
                self.click("preview.Outline")
            self.wait("outline is open", lambda d: d["preview_outline"])
            size = self.wait("native controls measured", lambda d: d["preview_controls_size"] is not None and d["preview_controls_size"] == d["preview_controls_measured_size"])["document"]["preview_controls_size"]
            self.stable_state(lambda d: d["preview_controls_size"] == size, frames=30)
            if self.capture_review and backend == "Pdfium":
                self.capture_viewport("preview-controls")
            self.native("finder")
            self.wait("controls hide outside the app", lambda d: d["preview_controls_visible"] is False and d["preview_controls_open"])
            self.native("activate")
            self.wait("controls return with the app", lambda d: d["preview_controls_visible"] is True)
            self.wait_target("preview.outline.0")
            self.click("preview.outline.0")
            self.wait("start navigation at first page", lambda d: d["preview_page"] == 0)
            self.wait_target("preview.outline.1")
            self.click("preview.outline.1")
            self.wait("outline navigates to second page", lambda d: d["preview_page"] == 1 and d["preview_back"])
            self.click("preview.Back")
            self.wait("back restores first page", lambda d: d["preview_page"] == 0 and d["preview_forward"])
            self.click("preview.Forward")
            self.wait("forward restores second page", lambda d: d["preview_page"] == 1)
            self.click("preview.Next page")
            self.wait("next page", lambda d: d["preview_page"] == 2)
            self.stable_state(lambda d: d["preview_page"] == 2)
            self.click("preview.Previous page")
            self.wait("previous page", lambda d: d["preview_page"] == 1)
            before = self.snapshot()["document"]["preview_zoom"]
            self.click("preview.Zoom in")
            self.wait("zoom in", lambda d: d["preview_zoom"] > before)
            self.click("preview.Fit page width")
            self.wait("fit restores zoom", lambda d: abs(d["preview_zoom"] - before) < 0.01)
            self.click("preview.query")
            self.key(0)
            self.native("text", "Final needle")
            self.key(36, "none")
            self.wait("preview text search", lambda d: d["preview_page"] == 2)
            self.click("preview.Minimize controls")
            self.wait("controls minimized", lambda d: not d["preview_controls_open"])
            self.click("preview.open")
            self.wait("controls reopened", lambda d: d["preview_controls_open"])
            self.click("preview.Pop out")
            self.wait("preview detached", lambda d: d["preview_popout"])
            self.native_wait("native preview window exists", lambda ws: any(w["AXTitle"] == "tiptoptyp Preview" for w in ws))
            self.native("close", "tiptoptyp Preview")
            self.wait("preview returns to document", lambda d: not d["preview_popout"] and d["view_mode"] == "Split")

    def editor_preferences(self):
        count = self.scratch("#let sample = {\n  // TODO: inspect\n  (1, 2, 3)\n}\n")
        original = self.snapshot()["document"]["source_fingerprint"]
        self.settings()
        self.click("settings.help")
        self.wait("Help opens within Settings", lambda d: d["settings_help_visible"] and d["settings_visible"])
        if self.capture_review:
            self.capture_viewport("settings")
        self.key(3)
        self.wait_target("settings.search")
        self.wait("Find returns to Settings search", lambda d: not d["settings_help_visible"] and d["settings_text_input_focused"])
        for query, title, target, field in [
            ("guides", "Indentation guides", "settings.editor.guides", "indentation_guides"),
            ("definitions", "Highlight definitions and redefinitions", "settings.editor.definitions", "highlight_definitions"),
            ("wrap", "Wrap lines", "settings.editor.wrap", "line_wrap"),
        ]:
            self.setting_section(query, title)
            before = self.snapshot()["document"][field]
            self.wait_target(target)
            self.click(target)
            self.wait("preference applies: " + field, lambda d: d[field] != before)
            self.click(target)
            self.wait("preference restores: " + field, lambda d: d[field] == before)
        self.setting_section("highlights", "Custom text highlights")
        self.wait_target("settings.highlight.1")
        self.click("settings.highlight.1")
        self.key(0)
        self.native("text", "FIXME:")
        self.wait("rule typing belongs to Settings", lambda d: d["settings_text_input_focused"] and d["source_fingerprint"] == original)
        if self.capture_review:
            self.capture_viewport("settings")
        self.close_settings()
        self.wait("preferences do not edit source", lambda d: d["source_fingerprint"] == original)
        if self.capture_review:
            self.capture_viewport("main")
        self.close_scratch(count)

    def settings_search(self):
        self.settings()
        tabs = self.snapshot()["document"]["tabs"]
        panel = self.snapshot()["document"]["panel"]
        self.key(45)  # Cmd+N belongs to Settings; it must not create a document.
        self.stable_state(lambda d: d["settings_visible"] and d["tabs"] == tabs)
        self.key(23)  # Cmd+5 must not toggle the document's bottom panel.
        self.stable_state(lambda d: d["settings_visible"] and d["panel"] == panel)
        self.key(3)  # Cmd+F focuses the Settings search, not document Find.
        self.wait("Settings search receives its shortcut", lambda d: d["settings_text_input_focused"] and not d["find_visible"])
        self.native("text", "backend")
        self.wait("Settings search receives typed text", lambda d: d["settings_query_fingerprint"] == fingerprint("backend"))
        self.key(3)
        self.wait("repeated Cmd+F keeps Settings search focused", lambda d: d["settings_text_input_focused"] and not d["find_visible"])
        for query, label in (("backend", "Typst preview backend"), ("appearance", "Appearance")):
            self.setting_section(query, label)
            self.wait("search destination highlighted", lambda d: d["settings_highlight"] == label)
            if self.capture_review and label == "Appearance":
                self.capture_viewport("settings")
        self.wait("highlight expires", lambda d: d["settings_highlight"] is None, timeout=5)
        self.click("settings.json.mode")
        self.wait("JSON settings opens with valid current settings", lambda d: d["settings_json"]["visible"] and d["settings_json"]["valid"])
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.json")
        self.key(0)  # Select all in the focused JSON editor.
        self.native("text", "{broken")
        self.wait("invalid JSON cannot be saved", lambda d: not d["settings_json"]["valid"])
        state = self.snapshot()
        if state["targets"]["settings.json.save"]["enabled"]:
            raise AssertionError("invalid settings left Save enabled")
        self.click("settings.json.reload")
        self.wait("Reload restores valid settings", lambda d: d["settings_json"]["valid"])
        self.click("settings.json")
        self.key(0)
        self.key(124, "none")  # Collapse selection at EOF; append valid whitespace.
        self.native("text", " ")
        self.wait("valid JSON is accepted", lambda d: d["settings_json"]["valid"])
        self.click("settings.json.save")
        self.wait("JSON saves through normal settings updates", lambda d: d["settings_json"]["saved"])
        settings_window = next(w for w in self.native("windows")["windows"] if "Settings" in w["AXTitle"])
        size = settings_window["size"]
        self.native("zoom-focused")
        self.native_wait("Settings maximizes independently", lambda ws: any(w.get("AXTitle") == settings_window["AXTitle"] and w.get("size") != size for w in ws))
        self.native("zoom-focused")
        self.native_wait("Settings restores its previous size", lambda ws: any(w.get("AXTitle") == settings_window["AXTitle"] and w.get("size") == size for w in ws))
        self.key(3)
        self.wait("Cmd+F stays in JSON", lambda d: d["settings_json"]["visible"] and not d["find_visible"])
        self.wait_target("settings.json.find")
        self.native("text", "writing_language")
        self.wait("JSON find receives typing", lambda d: d["settings_json"]["query"] == "writing_language")
        self.key(13)  # Cmd+W closes Settings, not the document tab.
        self.wait("Cmd+W closes Settings but preserves the document", lambda d: not d["settings_visible"] and d["tabs"] == 1)
        self.native_wait("document regains focus after Settings shortcut", lambda ws: not any("Settings" in w["AXTitle"] for w in ws) and any(w.get("AXFocused") for w in ws))

    def background(self):
        self.ensure_typesetting_tab()
        self.choose_backend("Pdfium")
        self.key(19)
        self.wait("preview pane hidden", lambda d: d["view_mode"] == "Code")
        self.choose_backend("Interactive")
        self.wait("hidden frontend created", lambda d: d["preview_surface_exists"] and not d["preview_surface_visible"])
        import time
        deadline = time.monotonic() + 30
        while True:
            state = self.snapshot()
            renderer = state.get("renderer")
            if renderer and renderer["age_ms"] < 500 and renderer["value"] and renderer["value"].get("loaded"):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("hidden Tinymist frontend did not finish loading")
            time.sleep(0.05)
        generation = state["renderer_generation"]
        self.key(20)
        self.wait("prepared preview revealed", lambda d: d["view_mode"] == "Split" and d["preview_surface_visible"])
        # WebKit may defer animation-frame painting for a hidden NSView; loading
        # the frontend must finish before reveal, without requiring hidden pixels.
        deadline = time.monotonic() + 10
        while True:
            state = self.snapshot()
            renderer = state.get("renderer")
            if renderer and renderer["age_ms"] < 500 and renderer["value"] and renderer["value"].get("ready"):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("prepared Tinymist viewer did not paint on reveal")
            time.sleep(0.05)
        for _ in range(8):
            if self.snapshot()["renderer_generation"] != generation:
                raise AssertionError("revealing the prepared preview reloaded it")

    def context_menu(self):
        import time
        original = "$ x $\n\nBody text here.\n"
        self.focus_document_window()
        self.wait("menu fixture opened", lambda d: d["path"] == str(self.directory / "menu.typ"), timeout=15)
        self.wait_target("editor.source")
        self.click("editor.source")
        self.wait_target("editor.caret")

        def caret_point():
            target = self.snapshot()["targets"].get("editor.caret")
            if not target or not target["enabled"] or target["age_ms"] > 500:
                raise AssertionError("missing, disabled or stale hit target: editor.caret")
            return target["x"], target["y"]

        # Home the caret, then step into the math on line 1. Keyboard
        # navigation from a known state keeps the click point deterministic.
        self.key(126, "cmd")
        self.key(124, "none")
        self.key(124, "none")
        self.wait_target("editor.caret")
        math_point = caret_point()
        self.right_click_at(*math_point)
        self.wait_target("menu.Convert to…")
        convert = self.snapshot()["targets"]["menu.Convert to…"]
        self.native("move", convert["x"], convert["y"])
        self.wait_target("menu.Display math")
        self.click("menu.Display math")
        converted = "$\nx\n$\n\nBody text here.\n"
        self.wait("display-math conversion rewrites the source",
                  lambda d: d["source_fingerprint"] == fingerprint(converted), timeout=15)
        self.key(6)
        self.wait("undo restores the inline math",
                  lambda d: d["source_fingerprint"] == fingerprint(original), timeout=15)

        # A plain-text line offers the standard rows but no Convert row. The
        # absent target can only be asserted once the new menu has painted
        # fresh rows while the old entry keeps aging.
        self.key(126, "cmd")
        self.key(125, "none")
        self.key(125, "none")
        self.wait_target("editor.caret")
        self.right_click_at(*caret_point())
        # Paste is always enabled, unlike Undo right after an undo.
        self.wait_target("menu.Paste")
        deadline = time.monotonic() + 6
        while True:
            state = self.snapshot()
            paste = state["targets"].get("menu.Paste")
            convert = state["targets"].get("menu.Convert to…")
            if paste and paste["enabled"] and paste["age_ms"] <= 500 and (
                    convert is None or convert["age_ms"] > 1500):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("plain-text menu kept a fresh Convert row")
            time.sleep(0.05)
        self.key(53, "none")
        self.wait("dismissed menu leaves the source alone",
                  lambda d: d["source_fingerprint"] == fingerprint(original), timeout=15)
