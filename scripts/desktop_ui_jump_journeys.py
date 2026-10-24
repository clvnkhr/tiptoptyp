"""Real source/preview context-menu navigation; observations never perform jumps."""

import math
import time


JUMP_TYPST_SOURCE = """#set page(
  width: 200pt,
  height: 240pt,
  margin: 20pt,
)
= First target
First page body.
#pagebreak()
= Middle target
Middle page body.
#pagebreak()
= Last target
Last page body.
"""
JUMP_TEX_SOURCE = r"""\documentclass{article}
\usepackage{geometry}
\geometry{
paperwidth=200pt,
paperheight=240pt,
margin=20pt}
\pagestyle{empty}
\begin{document}
\section*{First target}
First page body.
\newpage
\section*{Middle target}
Middle page body.
\newpage
\section*{Last target}
Last page body.
\end{document}
"""


JUMP_TEX_AUTO_SOURCE = r"""\documentclass{article}
\usepackage[paperwidth=400pt,paperheight=1500pt,margin=24pt]{geometry}
\pagestyle{empty}
\begin{document}
\vspace*{400pt}
First automatic target.
\newpage
\vspace*{540pt}
Middle automatic target.
\newpage
\vspace*{660pt}
Last automatic target.
\end{document}
"""


def compiled_pdf_geometry(document, previous_revision):
    """Require freshly painted geometry from a new fully admitted PDF."""
    from desktop_ui_journeys import pdfium_render_state
    render = pdfium_render_state(document)
    revision, active = render.get("revision"), render.get("active_revision")
    if (type(previous_revision) is not int or previous_revision < 0
            or type(revision) is not int or revision <= previous_revision
            or type(active) is not int or active != revision
            or document.get("preview_pdfium_ready") is not True
            or document.get("compile_indicator_spinning") is not False):
        raise AssertionError("edit has not admitted its newly compiled PDF")
    geometry = render.get("page_geometry")
    if (not isinstance(geometry, dict) or type(document.get("frame")) is not int
            or type(geometry.get("frame")) is not int or geometry["frame"] != document["frame"]
            or type(geometry.get("page")) is not int
            or type(document.get("preview_page")) is not int
            or geometry["page"] != document["preview_page"]):
        raise AssertionError("edit lacks fresh painted PDF geometry")
    rect, visible, zoom = geometry.get("rect"), geometry.get("interact"), geometry.get("zoom")
    if (not isinstance(rect, list) or len(rect) != 4
            or not isinstance(visible, list) or len(visible) != 4
            or any(type(n) not in (int, float) or not math.isfinite(n)
                   for n in (*rect, *visible, zoom))
            or min(rect[2], rect[3], visible[2], visible[3], zoom) <= 0):
        raise AssertionError("invalid compiled page geometry")
    if (not isinstance(document.get("preview_reveal"), dict)
            or not reveal_retired(document["preview_reveal"])):
        raise AssertionError("automatic compile unexpectedly armed a reveal animation")
    return render, geometry


def assert_pdfium_scroll_preserved(document, previous):
    """A default TeX recompile must retain the user's visible page and offset."""
    render, geometry = compiled_pdf_geometry(document, previous["revision"])
    baseline = previous["page_geometry"]
    if (geometry["page"] != baseline["page"]
            or abs(geometry["zoom"] - baseline["zoom"]) > 0.0001
            or any(abs(geometry["rect"][axis] - baseline["rect"][axis]) > 1
                   for axis in (0, 1))):
        raise AssertionError("default TeX recompilation moved the preview scroll position")
    position = render.get("source_position")
    if isinstance(position, dict) and position.get("revision") == render["revision"]:
        raise AssertionError("disabled TeX edit-follow still applied a source destination")
    return {"revision": render["revision"], "page": geometry["page"],
            "page_origin": geometry["rect"][:2], "zoom": geometry["zoom"]}


def assert_automatic_pdf_follow(document, previous_revision, page):
    """Check the new PDF's actual destination baseline independently of its cue."""
    render, geometry = compiled_pdf_geometry(document, previous_revision)
    position = render.get("source_position")
    if (not isinstance(position, dict)
            or type(position.get("revision")) is not int or position["revision"] != render["revision"]
            or type(position.get("page")) is not int or position["page"] != page
            or geometry["page"] != page):
        raise AssertionError("automatic follow belongs to another PDF or source destination")
    point, rect = position.get("point"), geometry["rect"]
    visible, zoom = geometry["interact"], geometry["zoom"]
    if (not isinstance(point, list) or len(point) != 2
            or any(type(n) not in (int, float) or not math.isfinite(n) for n in point)):
        raise AssertionError("invalid automatic-follow destination")
    center = [rect[axis] + point[axis] * zoom for axis in (0, 1)]
    inset = min(24, visible[3] * 0.1)
    if (not visible[0] <= center[0] <= visible[0] + visible[2]
            or not visible[1] + inset <= center[1] <= visible[1] + visible[3] - inset):
        raise AssertionError("automatic-follow baseline is clipped at the preview edge")
    return {"revision": render["revision"], "page": page, "center": center,
            "visible": visible, "top_inset": center[1] - visible[1]}


def source_region(source, heading):
    """TeX can attribute a heading box to the following paragraph's line."""
    lines = source.splitlines(keepends=True)
    line = next(index for index, text in enumerate(lines) if heading in text)
    start = sum(map(len, lines[:line]))
    end = start + sum(map(len, lines[line:line + 2]))
    return line, start, end


def cursor_in_region(document, bounds):
    cursor = document.get("cursor")
    return (isinstance(cursor, list) and len(cursor) == 2 and cursor[0] == cursor[1]
            and bounds[1] <= cursor[0] < bounds[2])


def renderer_ink_point(state, page):
    """Translate observed SVG ink into the observed native WebView rectangle."""
    renderer = state.get("renderer") or {}
    value = renderer.get("value") or {}
    target = state["targets"].get("preview.page") or {}
    size = value.get("window_size")
    ink = next((entry.get("bounds") for entry in value.get("source_ink", [])
                if entry.get("page") == page), None)
    if (renderer.get("age_ms", 1000) > 500 or value.get("ready") is not True
            or target.get("age_ms", 1000) > 500 or not target.get("enabled")
            or not isinstance(size, list) or len(size) != 2
            or not isinstance(ink, list) or len(ink) != 4):
        raise AssertionError("no fresh actual Tinymist glyph for source navigation")
    numbers = [*size, *ink, *(target.get(field) for field in ("x", "y", "width", "height"))]
    if any(isinstance(number, bool) or not isinstance(number, (int, float))
           or not math.isfinite(number) for number in numbers):
        raise AssertionError("invalid Tinymist source glyph geometry")
    if min(*size, ink[2], ink[3], target["width"], target["height"]) <= 0:
        raise AssertionError("empty Tinymist source glyph geometry")
    client = [ink[0] + ink[2] / 2, ink[1] + ink[3] / 2]
    if not (0 <= client[0] < size[0] and 0 <= client[1] < size[1]):
        raise AssertionError("Tinymist source glyph lies outside its native viewport")
    return [target["x"] - target["width"] / 2 + client[0] * target["width"] / size[0],
            target["y"] - target["height"] / 2 + client[1] * target["height"] / size[1]]


def assert_reveal(value, page=None, region=None, duration_ms=900):
    """Require actual visible, finite feedback at the observed destination."""
    if (isinstance(duration_ms, bool) or not isinstance(duration_ms, (int, float))
            or not math.isfinite(duration_ms) or not 250 <= duration_ms <= 2000):
        raise AssertionError("invalid expected reveal duration")
    if not isinstance(value, dict) or value.get("active") is not True or value.get("pending") is True:
        raise AssertionError("destination reveal is not visibly animating")
    center, viewport = value.get("center"), value.get("viewport")
    elapsed = value.get("elapsed_ms")
    if (not isinstance(center, list) or len(center) != 2
            or not isinstance(viewport, list) or len(viewport) != 4
            or any(isinstance(number, bool) or not isinstance(number, (int, float))
                   or not math.isfinite(number) for number in [*center, *viewport, elapsed])
            or not 0 <= elapsed < duration_ms
            or viewport[0] >= viewport[2] or viewport[1] >= viewport[3]
            or not viewport[0] <= center[0] <= viewport[2]
            or not viewport[1] <= center[1] <= viewport[3]):
        raise AssertionError("invalid or offscreen destination reveal geometry")
    if page is not None and (isinstance(value.get("page"), bool) or value.get("page") != page):
        raise AssertionError("destination reveal belongs to another paper")
    if region is not None:
        at = value.get("char_index")
        if not isinstance(at, int) or isinstance(at, bool) or not region[1] <= at < region[2]:
            raise AssertionError("source reveal belongs to another source location")
    target = value.get("target")
    if target is not None:
        if (not isinstance(target, list) or len(target) != 4
                or any(isinstance(edge, bool) or not isinstance(edge, (int, float))
                       or not math.isfinite(edge) for edge in target)
                or target[0] > target[2] or target[1] >= target[3]
                or abs(center[0] - (target[0] + target[2]) / 2) > 0.5
                or abs(center[1] - (target[1] + target[3]) / 2) > 0.5):
            raise AssertionError("destination reveal is not centered on its target")
    return value


def reveal_retired(value):
    return value is None or (isinstance(value, dict) and value.get("active") is False
                             and value.get("pending") is not True)


class PreviewJumpJourneys:
    def jump_reveal_feedback(self, backend, destination, arrival=None, page=None, region=None, options=None):
        options = options or {"enabled": True, "style": "Rings", "duration_ms": 900, "size_percent": 100}
        duration_ms = options["duration_ms"]
        def observation(state):
            if destination == "source":
                return state["document"].get("source_reveal"), True
            if backend == "Pdfium":
                return state["document"].get("preview_reveal"), True
            renderer = state.get("renderer") or {}
            return (renderer.get("value") or {}).get("reveal"), renderer.get("age_ms", 1000) <= 500

        state = arrival or self.snapshot()
        if not options["enabled"]:
            deadline = time.monotonic() + 4
            samples = 0
            while samples < 5:
                value, fresh = observation(state)
                if fresh:
                    if not reveal_retired(value):
                        raise AssertionError("disabled reveal feedback still armed or painted a cue")
                    if isinstance(value, dict) and value.get("options") != options:
                        raise AssertionError("disabled native preview did not receive its configured options")
                    samples += 1
                if time.monotonic() >= deadline:
                    raise AssertionError("disabled reveal feedback could not be observed freshly")
                time.sleep(0.06)
                state = self.snapshot()
            self.record("navigation.reveal", backend=backend, destination=destination,
                        options=options, disabled=True, fresh_observations=samples)
            return
        deadline = time.monotonic() + 4
        while True:
            value, fresh = observation(state)
            if fresh and isinstance(value, dict) and value.get("active") is True and value.get("pending") is not True:
                first = assert_reveal(value, page=page, region=region, duration_ms=duration_ms)
                if value.get("options") is not None and value["options"] != options:
                    raise AssertionError("native preview did not receive its configured reveal options")
                break
            if time.monotonic() >= deadline:
                raise AssertionError("navigation reached its destination without visible reveal feedback")
            time.sleep(0.025)
            state = self.snapshot()
        if (getattr(self, "capture_review", False) and destination == "source"
                and duration_ms != 900 and not getattr(self, "jump_effect_captured", False)):
            self.capture_viewport("main")
            self.jump_effect_captured = True
        deadline = time.monotonic() + duration_ms / 1000 + 1
        while True:
            state = self.snapshot()
            value, fresh = observation(state)
            if fresh and reveal_retired(value):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("destination reveal did not retire after its finite animation")
            if fresh and isinstance(value, dict) and value.get("active") is True and value.get("pending") is not True:
                assert_reveal(value, page=page, region=region, duration_ms=duration_ms)
            time.sleep(0.025)
        for _ in range(4):
            value, fresh = observation(self.snapshot())
            if not fresh or not reveal_retired(value):
                raise AssertionError("destination reveal reappeared after it retired")
        self.record("navigation.reveal", backend=backend, destination=destination,
                    first_visible=first, options=options, retired=True)

    def jump_open_fixture(self, path, source):
        path.write_text(source)
        self.focus_document_window()
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        document = self.wait("source-jump fixture opened", lambda d: d["path"] == str(path), timeout=15)["document"]
        tab = document["active_tab_id"]
        self.key(35, "cmd+alt")
        self.wait("source-jump fixture owns preview", lambda d:
                  d["preview_tab_id"] == tab and d["preview_path"] == str(path))
        self.key(20)
        self.wait("source-jump fixture has both panels", lambda d: d["view_mode"] == "Split")
        return tab

    def jump_renderer_ready(self, backend):
        expected = {"Pdfium": "PDFium", "Interactive": "Tinymist",
                    "InteractivePartial": "Tinymist partial"}[backend]
        self.wait("actual source-jump renderer is ready", lambda d:
                  d["preview_backend"] == expected and d["preview_pages"] == 3
                  and d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"]
                  and not d["compile_indicator_spinning"]
                  and (backend != "Pdfium" or d.get("synctex_available") is True), timeout=60)
        if backend == "Pdfium":
            return
        deadline = time.monotonic() + 20
        while True:
            state = self.snapshot()
            renderer = state.get("renderer") or {}
            value = renderer.get("value") or {}
            if (renderer.get("age_ms", 1000) <= 500 and value.get("ready") is True
                    and value.get("partial") is (backend == "InteractivePartial")
                    and value.get("pages") == 3):
                return
            if time.monotonic() >= deadline:
                raise AssertionError("source-jump journey did not observe the requested live Tinymist renderer")
            time.sleep(0.05)

    def jump_preview_page(self, page):
        if not self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.open")
        self.wait("navigation controls open", lambda d: d["preview_controls_open"])
        if self.snapshot()["document"]["preview_outline"]:
            self.click("preview.Outline")
            self.wait("navigation controls leave outline", lambda d: not d["preview_outline"])
        for _ in range(3):
            current = self.snapshot()["document"]["preview_page"]
            if current == page:
                break
            self.click("preview.Next page" if current < page else "preview.Previous page")
            self.wait("preview page changes through its real control", lambda d: d["preview_page"] != current, timeout=15)
        self.wait("preview reaches the requested paper", lambda d: d["preview_page"] == page, timeout=15)
        self.click("preview.Minimize controls")
        self.wait("navigation controls minimize", lambda d: not d["preview_controls_open"])

    def jump_caret(self, source, line, column=0):
        self.click("editor.source")
        self.key(126)
        if line:
            self.native("key-burst", 125, "none", line, 12)
        self.key(123)
        if column:
            self.native("key-burst", 124, "none", column, 12)
        position = sum(map(len, source.splitlines(keepends=True)[:line])) + column
        self.wait("native source caret is exact", lambda d:
                  d["cursor"] == [position, position] and d["editor_keyboard_focused"])
        self.wait_target("editor.caret")
        target = self.snapshot()["targets"]["editor.caret"]
        return target["x"], target["y"]

    def jump_inverse_point(self, backend, page):
        if backend == "Pdfium":
            self.wait_target("preview.source_ink")
            state = self.snapshot()
            if state["document"]["preview_page"] != page or not state["document"].get("synctex_available"):
                raise AssertionError("SyncTeX ink belongs to another preview")
            target = state["targets"]["preview.source_ink"]
            return target["x"], target["y"]
        deadline = time.monotonic() + 15
        while True:
            state = self.snapshot()
            try:
                return renderer_ink_point(state, page)
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)

    def jump_context_roundtrip(self, backend, source, path, options=None):
        from desktop_ui_journeys import fingerprint
        unchanged = lambda d: (d["path"] == str(path) and not d["dirty"]
                               and d["source_fingerprint"] == fingerprint(source)
                               and d["source_bytes"] == len(source.encode("utf-8")))
        first, last = source_region(source, "First target"), source_region(source, "Last target")
        column = 2 if path.suffix == ".typ" else 10
        last_point = self.jump_caret(source, last[0], column)
        self.jump_caret(source, first[0], column)
        self.jump_preview_page(0)
        # The menu must bind to this clicked source point rather than the old
        # caret or the pointer's later position over its menu row.
        self.right_click_at(*last_point)
        self.wait_target("menu.Reveal in Preview")
        self.stable_state(lambda d: unchanged(d) and d["preview_page"] == 0)
        if getattr(self, "capture_review", False):
            self.capture_viewport("popup")
        self.click("menu.Reveal in Preview")
        arrived = self.wait("source context action reveals the last paper", lambda d:
                            unchanged(d) and d["preview_page"] == 2, timeout=20)
        self.jump_reveal_feedback(backend, "preview", arrived, page=2,
                                  options=options["preview"] if options else None)
        self.stable_state(lambda d: unchanged(d) and d["preview_page"] == 2)
        # Source menus may preserve a pre-existing caret/selection. Establish a
        # distinct origin before testing that inverse navigation moves it.
        self.jump_caret(source, last[0], column)
        for page, region in ((0, first), (2, last)):
            self.jump_preview_page(page)
            self.jump_renderer_ready(backend)
            before = self.snapshot()["document"]
            if cursor_in_region(before, region):
                raise AssertionError("inverse jump does not start from a different source location")
            point = self.jump_inverse_point(backend, page)
            self.right_click_at(*point)
            row = "preview.Reveal in Source" if backend == "Pdfium" else "menu.Reveal in Source"
            self.wait_target(row)
            self.stable_state(lambda d: unchanged(d) and d["cursor"] == before["cursor"])
            if getattr(self, "capture_review", False):
                self.capture_viewport("main" if backend == "Pdfium" else "popup")
            # click() deliberately moves to the menu row before pressing it.
            self.click(row)
            arrived = self.wait("preview context action reveals source and focuses its editor", lambda d:
                                unchanged(d) and cursor_in_region(d, region)
                                and d["editor_keyboard_focused"] and not d["preview_keyboard_focused"], timeout=20)
            after = arrived["document"]
            self.jump_reveal_feedback(backend, "source", arrived, region=region,
                                      options=options["source"] if options else None)
            self.stable_state(lambda d: unchanged(d) and cursor_in_region(d, region)
                              and d["editor_keyboard_focused"])
            self.record("preview.jump", backend=backend, page=page,
                        source_region=[region[1], region[2]], cursor=after["cursor"],
                        source_fingerprint=after["source_fingerprint"])
        if path.read_text() != source:
            raise AssertionError("source navigation rewrote its fixture on disk")

    def jump_tex_automatic_follow(self):
        """Real edits preserve scrolling by default and optionally follow safely."""
        from desktop_ui_journeys import fingerprint
        original = self.snapshot()["document"]
        follow = original.get("tex_preview_follow_edits")
        if not isinstance(follow, bool):
            raise AssertionError("missing effective TeX edit-follow preference")
        if follow:
            self.tex_settings([('"tex_preview_follow_edits": true', '"tex_preview_follow_edits": false')])
        if original["compilation_paused"]:
            self.key(15, "cmd+shift")
            self.wait("automatic TeX compilation resumes for edit-follow checks",
                      lambda d: not d["compilation_paused"])
        try:
            source = JUMP_TEX_AUTO_SOURCE
            path = self.directory / "preview-jump-automatic.tex"
            tab = self.jump_open_fixture(path, source)
            self.key(15)
            self.jump_renderer_ready("Pdfium")
            if not self.snapshot()["document"]["preview_controls_open"]:
                self.click("preview.open")
            self.click("preview.Fit page width")
            self.click("preview.Minimize controls")
            self.jump_renderer_ready("Pdfium")
            for enabled in (False, True):
                if enabled:
                    self.tex_settings([('"tex_preview_follow_edits": false',
                                        '"tex_preview_follow_edits": true')])
                    self.wait("TeX edit-follow opts in independently of Typst",
                              lambda d: d["tex_preview_follow_edits"] is True
                              and d["preview_follow_edits"] is original["preview_follow_edits"])
                for iteration, (page, label, addition) in enumerate((
                        (1, "Middle automatic target.", " One."),
                        (2, "Last automatic target.", " Two."),
                        (1, "Middle automatic target.", " Three."))):
                    # The user's visible page deliberately differs from the
                    # active source location before every automatic compile.
                    self.jump_preview_page(0)
                    if not enabled:
                        self.wait_target("preview.page")
                        initial = self.snapshot()
                        target = initial["targets"]["preview.page"]
                        top = initial["document"]["pdfium_render"]["page_geometry"]["rect"][1]
                        self.native("wheel", target["x"], target["y"], -8)
                        deadline = time.monotonic() + 4
                        last, stable = None, 0
                        while True:
                            document = self.snapshot()["document"]
                            geometry = document["pdfium_render"].get("page_geometry") or {}
                            current = (geometry.get("rect") or [None, None])[1]
                            fresh = (geometry.get("frame") == document["frame"]
                                     and geometry.get("page") == 0
                                     and type(current) in (int, float)
                                     and math.isfinite(current) and current <= top - 12)
                            if fresh:
                                stable = stable + 1 if last is not None and abs(current - last) <= 0.25 else 0
                                last = current
                                if stable >= 4:
                                    break
                            else:
                                stable, last = 0, None
                            if time.monotonic() >= deadline:
                                raise AssertionError("TeX preview did not settle at a nonzero native-wheel offset")
                            time.sleep(0.06)
                        self.stable_state(lambda d:
                                          d["pdfium_render"]["page_geometry"]["page"] == 0
                                          and abs(d["pdfium_render"]["page_geometry"]["rect"][1] - last) <= 1)
                        self.record("preview.jump.scrolled", iteration=iteration,
                                    page=0, previous_top=top, scrolled_top=last)
                    line = source_region(source, label)[0]
                    column = len(source.splitlines()[line])
                    self.jump_caret(source, line, column)
                    before = self.snapshot()["document"]
                    previous = before["pdfium_render"]
                    if type(previous.get("revision")) is not int:
                        raise AssertionError("missing PDFium document revision")
                    at = sum(map(len, source.splitlines(keepends=True)[:line])) + column
                    source = source[:at] + addition + source[at:]
                    caret = at + len(addition)
                    self.native("text", addition)

                    def placement(d):
                        if enabled:
                            return assert_automatic_pdf_follow(d, previous["revision"], page)
                        return assert_pdfium_scroll_preserved(d, previous)

                    def completed(d):
                        if (d["path"] != str(path) or d["source_fingerprint"] != fingerprint(source)
                                or d["source_bytes"] != len(source.encode("utf-8"))
                                or d["cursor"] != [caret, caret] or not d["editor_keyboard_focused"]):
                            return False
                        try:
                            placement(d)
                            return True
                        except AssertionError:
                            return False

                    arrived = self.wait("edited TeX automatically compiles with its chosen scroll behavior",
                                        completed, timeout=60)
                    observed = placement(arrived["document"])
                    self.stable_state(completed)
                    self.record("preview.jump.automatic", follow_enabled=enabled, iteration=iteration,
                                source_fingerprint=fingerprint(source), cursor=[caret, caret],
                                previous_revision=previous["revision"], placement=observed, silent=True)
                    if getattr(self, "capture_review", False) and enabled and iteration == 0:
                        self.capture_viewport("main")
            return tab
        finally:
            if original["compilation_paused"]:
                self.key(15, "cmd+shift")
                self.wait("edit-follow check restores paused compilation",
                          lambda d: d["compilation_paused"])
            if self.snapshot()["document"]["tex_preview_follow_edits"] is not follow:
                old, new = ("true", "false") if not follow else ("false", "true")
                self.tex_settings([(f'"tex_preview_follow_edits": {old}',
                                    f'"tex_preview_follow_edits": {new}')])

    def jump_detached_partial_inverse(self, source, path):
        from desktop_ui_journeys import fingerprint
        first = source_region(source, "First target")
        last = source_region(source, "Last target")
        unchanged = lambda d: (d["path"] == str(path) and not d["dirty"]
                               and d["source_fingerprint"] == fingerprint(source)
                               and d["source_bytes"] == len(source.encode("utf-8")))
        self.jump_caret(source, last[0], 2)
        if not self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.open")
        self.click("preview.Pop out")
        self.wait("partial source preview detaches", lambda d: d["preview_popout"] and unchanged(d))
        self.native_wait("detached native preview exists", lambda windows:
                         any(window["AXTitle"] == "tiptoptyp Preview" for window in windows))
        if self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.Minimize controls")
            self.wait("detached navigation controls minimize", lambda d: not d["preview_controls_open"])
        self.jump_renderer_ready("InteractivePartial")
        self.wait("fresh detached preview begins on its first paper", lambda d:
                  d["preview_page"] == 0 and unchanged(d), timeout=20)
        self.native("raise", "tiptoptyp Preview")
        self.native_wait("detached preview owns native focus", lambda windows:
                         any(window["AXTitle"] == "tiptoptyp Preview" and window.get("AXFocused")
                             for window in windows))
        before = self.snapshot()["document"]
        if not cursor_in_region(before, last):
            raise AssertionError("detached inverse jump lost its distinct source origin")
        self.right_click_at(*self.jump_inverse_point("InteractivePartial", 0))
        self.wait_target("menu.Reveal in Source")
        self.stable_state(lambda d: unchanged(d) and d["preview_popout"] and d["cursor"] == before["cursor"])
        if getattr(self, "capture_review", False):
            self.capture_viewport("popup")
        self.click("menu.Reveal in Source")
        arrived = self.wait("detached inverse action focuses and reveals its source document", lambda d:
                            unchanged(d) and d["preview_popout"] and cursor_in_region(d, first)
                            and d["focused"] and d["editor_keyboard_focused"]
                            and not d["preview_keyboard_focused"], timeout=20)
        after = arrived["document"]
        self.jump_reveal_feedback("InteractivePartial", "source", arrived, region=first)
        self.record("preview.jump.detached", backend="InteractivePartial", page=0,
                    source_region=[first[1], first[2]], cursor=after["cursor"],
                    source_fingerprint=after["source_fingerprint"])
        self.native("close", "tiptoptyp Preview")
        self.wait("detached source check restores split view", lambda d:
                  not d["preview_popout"] and d["view_mode"] == "Split" and unchanged(d))

    def jump_effect_settings(self, options):
        """Edit effects through real form controls, never through inspection."""
        generation = self.snapshot()["document"]["preview_generation"]
        self.settings()
        for scope in ("source", "preview"):
            title = "Reveal in " + scope
            self.setting_section(title, title)
            for field, control in (("duration_ms", "duration"), ("size_percent", "size")):
                current = self.snapshot()["document"]["reveal_animation_options"][scope]
                if current[field] == options[scope][field]:
                    continue
                name = f"settings.reveal.{scope}.{control}"
                self.wait_target(name)
                self.double_click(name, minimum_height=18)
                self.wait_target(name, focused=True)
                self.key(0)
                self.native("text", str(options[scope][field]))
                self.key(36, "none")
                self.wait("reveal numeric option applies through Settings", lambda d:
                          d["reveal_animation_options"][scope][field] == options[scope][field])
            current = self.snapshot()["document"]["reveal_animation_options"][scope]
            if current["enabled"] != options[scope]["enabled"]:
                self.click(f"settings.reveal.{scope}.enabled")
                self.wait("reveal enabled option applies through Settings", lambda d:
                          d["reveal_animation_options"][scope]["enabled"] is options[scope]["enabled"])
        self.close_settings()
        self.click("editor.source")
        deadline = time.monotonic() + 4
        while True:
            state = self.snapshot()
            if state["document"]["preview_generation"] != generation:
                raise AssertionError("effect preferences restarted the preview server")
            renderer = state.get("renderer") or {}
            reveal = (renderer.get("value") or {}).get("reveal") or {}
            if renderer.get("age_ms", 1000) <= 500 and reveal.get("options") == options["preview"]:
                break
            if time.monotonic() >= deadline:
                raise AssertionError("reveal preferences did not reach the existing native preview")
            time.sleep(0.05)

    def jump_effect_options(self, source, path):
        """Long effects and independent Off controls preserve real navigation."""
        original = self.snapshot()["document"]["reveal_animation_options"]
        custom = {scope: {**value, "duration_ms": 1500, "size_percent": 150, "enabled": True}
                  for scope, value in original.items()}
        try:
            self.jump_effect_settings(custom)
            self.jump_renderer_ready("Interactive")
            self.jump_context_roundtrip("Interactive", source, path, options=custom)
            preview_off = {scope: {**value, "enabled": scope != "preview"}
                           for scope, value in custom.items()}
            self.jump_effect_settings(preview_off)
            self.jump_renderer_ready("Interactive")
            self.jump_context_roundtrip("Interactive", source, path, options=preview_off)
            source_off = {scope: {**value, "enabled": scope != "source"}
                          for scope, value in custom.items()}
            self.jump_effect_settings(source_off)
            self.jump_renderer_ready("Interactive")
            self.jump_context_roundtrip("Interactive", source, path, options=source_off)
        finally:
            self.jump_effect_settings(original)
        self.record("navigation.reveal.options", independent_off=True,
                    configured_duration_ms=1500, configured_size_percent=150, restored=original)

    def tex_preview_scroll(self):
        """Isolated automatic TeX recompiles, independent of Tinymist journeys."""
        self.focus_document_window()
        before = self.snapshot()["document"]
        if before["find_visible"]:
            self.click("find.close")
            self.wait("source Find closes before TeX scroll checks", lambda d: not d["find_visible"])
        self.click("editor.source")
        if before["line_wrap"]:
            self.key(6, "cmd+alt")
            self.wait("TeX scroll fixture uses logical source lines", lambda d: not d["line_wrap"])
        tab = self.jump_tex_automatic_follow()
        self.restore_partial_fixture_tabs(before, [tab])
        if before["line_wrap"]:
            self.click("editor.source")
            self.key(6, "cmd+alt")
            self.wait("TeX scroll journey restores line wrapping", lambda d: d["line_wrap"])
        if before["view_mode"] != "Split":
            self.key(19 if before["view_mode"] == "Code" else 21)
            self.wait("TeX scroll journey restores its original panels",
                      lambda d: d["view_mode"] == before["view_mode"])

    def preview_jump(self):
        self.focus_document_window()
        before = self.snapshot()["document"]
        if before["find_visible"]:
            self.click("find.close")
            self.wait("source Find closes before jump checks", lambda d: not d["find_visible"])
        self.click("editor.source")
        if before["line_wrap"]:
            self.key(6, "cmd+alt")
            self.wait("jump fixture uses logical source lines", lambda d: not d["line_wrap"])
        created = []
        tex_path = self.directory / "preview-jump.tex"
        created.append(self.jump_open_fixture(tex_path, JUMP_TEX_SOURCE))
        self.key(15)
        self.jump_renderer_ready("Pdfium")
        self.jump_context_roundtrip("Pdfium", JUMP_TEX_SOURCE, tex_path)
        typst_path = self.directory / "preview-jump.typ"
        created.append(self.jump_open_fixture(typst_path, JUMP_TYPST_SOURCE))
        for backend in ("InteractivePartial", "Interactive"):
            self.choose_backend(backend, save_pdf=False)
            self.jump_renderer_ready(backend)
            self.jump_context_roundtrip(backend, JUMP_TYPST_SOURCE, typst_path)
            if backend == "InteractivePartial":
                self.jump_detached_partial_inverse(JUMP_TYPST_SOURCE, typst_path)
        self.jump_effect_options(JUMP_TYPST_SOURCE, typst_path)
        # The original tab may be plain text, with no compiler controls. Restore
        # the preference while our small Typst fixture can still start its renderer.
        if self.snapshot()["document"]["preview_requested_backend"] != before["preview_requested_backend"]:
            self.choose_backend(before["preview_requested_backend"], save_pdf=False)
        self.restore_partial_fixture_tabs(before, created)
        if before["line_wrap"]:
            self.click("editor.source")
            self.key(6, "cmd+alt")
            self.wait("jump journey restores line wrapping", lambda d: d["line_wrap"])
        if before["view_mode"] != "Split":
            self.key(19 if before["view_mode"] == "Code" else 21)
            self.wait("jump journey restores its original panel arrangement", lambda d: d["view_mode"] == before["view_mode"])
