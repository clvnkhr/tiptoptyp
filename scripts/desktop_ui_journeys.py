"""Regression journeys driven only through native UI input.

The inspection socket supplies assertions and actual hit rectangles, never actions.
See docs/desktop-ui-audit.md for the lower-level tests these journeys promote.
"""


def fingerprint(text):
    value = 0xcbf29ce484222325
    for byte in text.encode("utf-8"):
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"


class EditorJourneys:
    def paste_source(self, text):
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
            self.source_is(text)
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

    def scratch(self, text=""):
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
        self.click("editor.source")
        self.key(0)
        self.key(51, "none")
        self.source_is("")
        self.wait("cleared scratch is clean", lambda d: not d["dirty"])
        self.key(13)
        self.wait("scratch tab closed once", lambda d: d["tabs"] == count)

    def editing(self):
        count = self.scratch("zzz")
        self.wait("typed caret follows text", lambda d: d["cursor"] == [3, 3])
        self.key(6)  # Undo
        self.source_is("")
        self.key(123, "cmd")  # Move caret before replaying the edit.
        self.key(6, "cmd+shift")
        self.source_is("zzz")
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
        self.paste_source("alpha\nbeta")
        self.source_is("alpha\nbeta")
        self.key(0)
        self.key(44)  # Toggle comment on a selection.
        self.source_is("// alpha\n// beta")
        self.key(44)
        self.source_is("alpha\nbeta")
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

    def layout(self):
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
        target = "fold." + str(headers[0])
        self.wait_target(target)
        self.click(target)
        self.wait("gutter click collapses", lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers)
        self.wait_target(target)
        self.click(target)
        self.wait("same gutter control expands", lambda d: not d["fold_collapsed"] and d["fold_headers"] == headers)
        self.source_is(text)
        self.close_scratch(count)

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

    def choose_backend(self, backend):
        self.settings()
        self.setting_section("backend", "Typst preview backend")
        self.wait_target("settings.backend." + backend)
        self.click("settings.backend." + backend)
        self.close_settings()
        self.key(15)
        self.wait("selected renderer ready", lambda d: d["preview_requested_backend"] == backend
                  and d["preview_backend"] == ("PDFium" if backend == "Pdfium" else "Tinymist")
                  and d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"], timeout=60)

    def controls(self):
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            self.wait("three-page preview loaded", lambda d: d["preview_pages"] == 3, timeout=30)
            if not self.snapshot()["document"]["preview_controls_open"]:
                if backend == "Pdfium":
                    target = self.snapshot()["targets"]["preview.open"]
                    self.native("move", target["x"], target["y"])
                    self.wait("toolbar tooltip appears before opening controls", lambda d: d["hover_tooltip_open"])
                self.click("preview.open")
                self.wait("opening controls dismisses the toolbar tooltip", lambda d: not d["hover_tooltip_open"])
            self.wait("controls opened", lambda d: d["preview_controls_open"])
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

    def settings_search(self):
        self.settings()
        for query, label in (("backend", "Typst preview backend"), ("appearance", "Appearance")):
            self.setting_section(query, label)
            self.wait("search destination highlighted", lambda d: d["settings_highlight"] == label)
            if self.capture_review and label == "Appearance":
                self.capture_viewport("settings")
        self.wait("highlight expires", lambda d: d["settings_highlight"] is None, timeout=5)
        self.close_settings()

    def background(self):
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
