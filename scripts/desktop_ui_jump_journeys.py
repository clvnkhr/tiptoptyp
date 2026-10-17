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


def assert_reveal(value, page=None, region=None):
    """Require actual visible, finite feedback at the observed destination."""
    if not isinstance(value, dict) or value.get("active") is not True or value.get("pending") is True:
        raise AssertionError("destination reveal is not visibly animating")
    center, viewport = value.get("center"), value.get("viewport")
    elapsed = value.get("elapsed_ms")
    if (not isinstance(center, list) or len(center) != 2
            or not isinstance(viewport, list) or len(viewport) != 4
            or any(isinstance(number, bool) or not isinstance(number, (int, float))
                   or not math.isfinite(number) for number in [*center, *viewport, elapsed])
            or not 0 <= elapsed < 900
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
    def jump_reveal_feedback(self, backend, destination, arrival=None, page=None, region=None):
        def observation(state):
            if destination == "source":
                return state["document"].get("source_reveal"), True
            if backend == "Pdfium":
                return state["document"].get("preview_reveal"), True
            renderer = state.get("renderer") or {}
            return (renderer.get("value") or {}).get("reveal"), renderer.get("age_ms", 1000) <= 500

        state = arrival or self.snapshot()
        deadline = time.monotonic() + 4
        while True:
            value, fresh = observation(state)
            if fresh and isinstance(value, dict) and value.get("active") is True and value.get("pending") is not True:
                first = assert_reveal(value, page=page, region=region)
                break
            if time.monotonic() >= deadline:
                raise AssertionError("navigation reached its destination without visible reveal feedback")
            time.sleep(0.025)
            state = self.snapshot()
        deadline = time.monotonic() + 2
        while True:
            state = self.snapshot()
            value, fresh = observation(state)
            if fresh and reveal_retired(value):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("destination reveal did not retire after its finite animation")
            if fresh and isinstance(value, dict) and value.get("active") is True and value.get("pending") is not True:
                assert_reveal(value, page=page, region=region)
            time.sleep(0.025)
        for _ in range(4):
            value, fresh = observation(self.snapshot())
            if not fresh or not reveal_retired(value):
                raise AssertionError("destination reveal reappeared after it retired")
        self.record("navigation.reveal", backend=backend, destination=destination,
                    first_visible=first, retired=True)

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

    def jump_context_roundtrip(self, backend, source, path):
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
        self.jump_reveal_feedback(backend, "preview", arrived, page=2)
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
            self.jump_reveal_feedback(backend, "source", arrived, region=region)
            self.stable_state(lambda d: unchanged(d) and cursor_in_region(d, region)
                              and d["editor_keyboard_focused"])
            self.record("preview.jump", backend=backend, page=page,
                        source_region=[region[1], region[2]], cursor=after["cursor"],
                        source_fingerprint=after["source_fingerprint"])
        if path.read_text() != source:
            raise AssertionError("source navigation rewrote its fixture on disk")

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
