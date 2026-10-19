"""Real-app input acceptance for the separate buffered Tinymist mode."""
from pathlib import Path
import math
import time


def assert_partial_coverage(renderer, pages=48):
    if not renderer or renderer.get("age_ms", 1000) > 500:
        raise AssertionError("missing or stale partial-render observation")
    value = renderer.get("value")
    if not value or not value.get("ready") or not value.get("partial"):
        raise AssertionError("actual Tinymist partial renderer is not ready")
    visible = value.get("visible", [])
    if value.get("pages") != pages or not visible:
        raise AssertionError("partial renderer has no visible document pages")
    if not isinstance(visible, list) or any(
            not isinstance(page, dict) or type(page.get("index")) is not int
            or not 0 <= page["index"] < pages or page.get("dummy") is not False
            or page.get("ink") is not True for page in visible):
        raise AssertionError("a visible partial-render page has no identified SVG ink")
    if len({page["index"] for page in visible}) != len(visible):
        raise AssertionError("partial-render visible pages contain duplicate indices")
    populated, canvases = value.get("populated"), value.get("canvases")
    if (type(populated) is not int or type(canvases) is not int or canvases != 0
            or not len(visible) <= populated <= min(pages, len(visible) + 6)):
        raise AssertionError("partial rendering escaped its bounded SVG buffer")
    return value


def assert_zoom_burst(before, after, vector_requests, count=20):
    """Require current native evidence; an old schema must not skip coalescing."""
    for state in (before, after):
        if (any(type(state.get(field)) is not int or state[field] < 0
                for field in ("inputs", "hits"))
                or any(type(state.get(field)) is not bool for field in ("deferred", "active"))
                or any(field not in state for field in ("pending", "in_flight"))):
            raise AssertionError("zoom burst has missing or invalid native evidence")
    if (after["inputs"] < before["inputs"] + count or after["active"]
            or after["deferred"] or after["pending"] is not None
            or after["in_flight"] is not None):
        raise AssertionError("native burst has not handed off its latest input")
    if (type(vector_requests) is not int or vector_requests != 1
            or after["hits"] != before["hits"] + count):
        raise AssertionError("cached burst must transform all inputs and send exactly one final vector request")


def assert_partial_default(document, renderer):
    if (document.get("preview_requested_backend") != "InteractivePartial"
            or document.get("preview_backend") != "Tinymist partial"):
        raise AssertionError("fresh Typst settings did not select the buffered backend")
    value = (renderer or {}).get("value") or {}
    if ((renderer or {}).get("age_ms", 1000) > 500 or not value.get("ready")
            or value.get("partial") is not True):
        raise AssertionError("fresh Typst preview did not load the actual buffered renderer")


def assert_zoom_geometry(renderer, baseline, expected):
    value = (renderer or {}).get("value") or {}
    if ((renderer or {}).get("age_ms", 1000) > 500 or not value.get("ready")):
        raise AssertionError("missing or stale zoom geometry")
    numbers = [value.get("zoom"), value.get("paper_scale"),
               baseline.get("zoom"), baseline.get("paper_scale"), expected]
    if any(isinstance(n, bool) or not isinstance(n, (int, float))
           or not math.isfinite(n) or n <= 0 for n in numbers):
        raise AssertionError("invalid zoom geometry")
    if abs(value["zoom"] - expected) > 1e-6:
        raise AssertionError("zoom inputs did not accumulate to the requested scale")
    actual = value["paper_scale"] / baseline["paper_scale"]
    requested = expected / baseline["zoom"]
    if abs(actual / requested - 1) > 0.005:
        raise AssertionError("rendered paper did not grow/shrink with the requested zoom")
    return value


def source_point(value, x, y):
    scale=value["paper_scale"]
    paper=value["paper_bounds"]
    pages=value["page_bounds"]
    # Document edges can include fractional SVG insets or an outer gutter.
    # Keep the exact signed source offset; do not clamp it to the paper edge.
    if pages and pages[0] and y < pages[0][1]:
        return {"x": (x-paper[0])/scale, "page": 0, "local": (y-pages[0][1])/scale}
    if pages and pages[-1] and y > pages[-1][1] + pages[-1][3]:
        return {"x": (x-paper[0])/scale, "page": len(pages)-1,
                "local": (y-pages[-1][1])/scale}
    for index,bounds in enumerate(pages):
        if not bounds:continue
        if bounds[1]-1e-4 <= y <= bounds[1]+bounds[3]+1e-4:
            return {"x":(x-paper[0])/scale,"page":index,"local":(y-bounds[1])/scale}
        end=bounds[1]+bounds[3]
        if index+1<len(pages) and pages[index+1] and end<y<pages[index+1][1]:
            return {"x":(x-paper[0])/scale,"before":index,"after":index+1,"fraction":(y-end)/(pages[index+1][1]-end)}
    raise AssertionError("anchor must lie inside populated page geometry")


def project_source_point(value, point):
    pages=value["page_bounds"]
    if "page" in point:y=pages[point["page"]][1]+point["local"]*value["paper_scale"]
    else:
        before=pages[point["before"]]
        end=before[1]+before[3]
        y=end+point["fraction"]*(pages[point["after"]][1]-end)
    return [value["paper_bounds"][0]+point["x"]*value["paper_scale"],y]


def _zoom_snapshot_idle(value, *, allow_capture=False):
    snapshot = value.get("zoom_snapshot")
    if not isinstance(snapshot, dict):
        return False
    return (snapshot.get("enabled") is True and snapshot.get("cached") is True
            and all(snapshot.get(field) is False
                    for field in ("active", "admitting", "handoff_pending"))
            and "request" in snapshot and snapshot["request"] is None
            and type(snapshot.get("capturing")) is bool
            and type(snapshot.get("frame_ready")) is bool
            and (allow_capture or (not snapshot["capturing"] and snapshot["frame_ready"])))


class PartialPreviewJourneys:
    def partial_preview_default(self):
        # Called only for this journey's fresh isolated app profile. The `all`
        # journey intentionally changes backend before reaching partial_preview.
        self.wait("fresh profile selects buffered Typst preview", lambda d:
                  d["preview_requested_backend"] == "InteractivePartial"
                  and d["preview_backend"] == "Tinymist partial")
        deadline = time.monotonic() + 30
        while True:
            state = self.snapshot()
            try:
                assert_partial_default(state["document"], state.get("renderer"))
                self.record("partial.default", requested="InteractivePartial", actual_partial=True)
                return
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)

    def partial_dividers(self):
        """Independent divider coverage; never sends the system's Ctrl-wheel gesture."""
        self.partial_preview_default()
        for mode in ["partial", "full"]:
            if mode == "full":
                self.choose_backend("Interactive", save_pdf=False)
                self.wait("full preview ready", lambda d: d["preview_backend"] == "Tinymist" and d["preview_native_ready"], timeout=30)
            self.cached_divider_resize(mode)

    def partial_cache_edges(self):
        self.partial_preview_default()
        for mode in ["partial","full"]:
            if mode == "full":
                self.choose_backend("Interactive",save_pdf=False)
                self.wait("full preview ready",lambda d:d["preview_backend"] == "Tinymist" and d["preview_native_ready"],timeout=30)
            self.cached_edge_wheel(mode)
            self.cached_divider_resize(mode)
            self.cursor_wheel_zoom(mode)

    def partial_zoom_stress(self):
        self.partial_preview(stress=True)

    def zoom_burst(self, baseline, mode):
        # One native producer avoids process-launch gaps between gesture inputs.
        # No inspection or artificial app repaint occurs while it posts keys.
        self.click("preview.page")
        self.wait("burst preview owns keyboard", lambda d: d["preview_keyboard_focused"])
        deadline = time.monotonic() + 15
        while True:
            state = self.snapshot()
            cache = ((state.get("renderer") or {}).get("value") or {}).get("zoom_snapshot") or {}
            if cache.get("cached") and not cache.get("active"):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("zoom burst did not have an idle native image")
            time.sleep(.05)
        time.sleep(3)
        native_before = state["document"]["preview_zoom_overlay"]
        requests_before = cache["hits"] + cache["misses"]
        started = time.monotonic()
        self.native("key-burst", 24, "cmd", 20, 18)
        producer_done = time.monotonic()
        expected = min(10, baseline["zoom"] * 1.1 ** 20)
        deadline = producer_done + 20
        while True:
            state = self.snapshot()
            native = state["document"]["preview_zoom_overlay"]
            value = ((state.get("renderer") or {}).get("value") or {})
            cache = value.get("zoom_snapshot") or {}
            try:
                assert_zoom_geometry(state.get("renderer"), baseline, expected)
                if mode == "partial":
                    assert_partial_coverage(state.get("renderer"))
                elif value.get("partial") or value.get("populated") != 48:
                    raise AssertionError("full burst lost document ink")
                if native["inputs"] < native_before["inputs"] + 20 or native.get("in_flight") is not None or native.get("pending") is not None or native["active"]:
                    raise AssertionError("native burst has not handed off its latest input")
                break
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.02)
        vector_requests = cache["hits"] + cache["misses"] - requests_before
        assert_zoom_burst(native_before, native, vector_requests)
        self.record("partial.zoom-burst", mode=mode, count=20, interval_ms=18, warmup_seconds=3,
                    viewport=state["document"]["native_size"], dark=state["document"]["dark"], page_dark=state["document"]["page_dark"],
                    producer_ms=(producer_done-started)*1000,
                    last_producer_to_sharp_ms=(time.monotonic()-producer_done)*1000,
                    total_ms=(time.monotonic()-started)*1000,
                    vector_requests=vector_requests,
                    native_before=native_before, native_after=native)
        self.key(29, "cmd")
        self.wait("burst fit restores scale", lambda d: abs(d["preview_zoom"]-1) < .001)

    def cached_edge_wheel(self, mode):
        self.click("preview.page")
        self.key(115)  # Home: the first document edge.
        self.wait("preview reaches top edge", lambda d: d["preview_page"] == 0)
        deadline = time.monotonic() + 15
        stable = None
        quiet_since = None
        while True:
            before = self.snapshot()
            native = before["document"]["preview_zoom_overlay"]
            renderer = before.get("renderer") or {}
            value = renderer.get("value") or {}
            scroll = value.get("scroll")
            now = time.monotonic()
            quiescent = (
                native.get("cached") is True
                and all(native.get(field) is False for field in
                        ("active", "capturing", "prepared", "deferred"))
                and all(field in native and native[field] is None
                        for field in ("pending", "in_flight"))
                and isinstance(native.get("token"), str) and native["token"]
                and renderer.get("age_ms", 1000) <= 500 and value.get("ready") is True
                # Native completion can precede the last observed JS two-frame
                # handoff. Both owners must be quiet before fixing the token.
                and _zoom_snapshot_idle(value)
                and isinstance(scroll, list) and len(scroll) == 2
                and all(type(n) in (int, float) and math.isfinite(n) for n in scroll)
                and 0 <= scroll[1] < 1)
            if quiescent:
                state = (native["token"], native.get("scale"), native.get("view_size"),
                         scroll, value.get("zoom"), value.get("paper_scale"),
                         value.get("paper_bounds"))
                if state != stable:
                    stable, quiet_since = state, now
                # Sharp handoff keeps the old image while a 150 ms idle timer
                # prepares its replacement. Token identity is meaningful only
                # after that refresh has settled, not merely after zoom ends.
                elif now - quiet_since >= .25:
                    break
            else:
                stable = quiet_since = None
            if now > deadline:
                raise AssertionError("top edge has no idle cached preview")
            time.sleep(.05)
        target=before["targets"]["preview.page"]
        self.native("move",target["x"],target["y"])
        self.native("wheel",target["x"],target["y"],8)
        time.sleep(.12)
        after=self.snapshot()
        current=after["document"]["preview_zoom_overlay"]
        if current.get("cached") is not True or current.get("token") != native.get("token"):
            raise AssertionError("a clamped top-edge wheel discarded idle preview pixels")
        renderer = after.get("renderer") or {}
        value = renderer.get("value") or {}
        age = renderer.get("age_ms")
        scroll = value.get("scroll")
        # A background capture can prepare replacement pixels without starting
        # a gesture; the full-mode native receipt demonstrates this valid state.
        if (any(current.get(field) is not False for field in ("active", "prepared", "deferred"))
                or any(field not in current or current[field] is not None
                       for field in ("pending", "in_flight"))
                or type(current.get("capturing")) is not bool
                or type(age) not in (int, float) or not math.isfinite(age) or not 0 <= age <= 500
                or value.get("ready") is not True
                or not _zoom_snapshot_idle(value, allow_capture=True)
                or not isinstance(scroll, list) or len(scroll) != 2
                or any(type(n) not in (int, float) or not math.isfinite(n) for n in scroll)
                or not 0 <= scroll[1] < 1):
            raise AssertionError("a clamped top-edge wheel did not retain an idle top-edge preview")
        self.record("partial.cached-edge-wheel",mode=mode,before=native,after=current)

    def cursor_wheel_zoom(self, mode):
        environment = self.native("preflight")
        if environment.get("system_scroll_zoom") is True:
            reason = "macOS scroll-to-zoom is enabled; global Ctrl-wheel cannot establish app-only input delivery"
            self.record("partial.cursor-wheel.unavailable", mode=mode, reason=reason)
            raise RuntimeError("Native cursor-wheel check unavailable: " + reason + ". No zoom input was posted.")
        self.click("preview.page")
        self.key(115)
        self.wait("cursor zoom starts on page one",lambda d:d["preview_page"]==0)
        deadline=time.monotonic()+15
        while True:
            before=self.snapshot();native=before["document"]["preview_zoom_overlay"]
            value=((before.get("renderer") or {}).get("value") or {})
            if native.get("cached") and not native.get("active") and value.get("ready"):break
            if time.monotonic()>deadline:raise AssertionError("cursor zoom has no idle image")
            time.sleep(.05)
        width,height=native["view_size"]
        target=before["targets"]["preview.page"]
        # Actual positioned native wheel input, not a zoom-handler invocation.
        client=[width*.3,height*.4]
        screen=[target["x"]-width*.5+client[0],target["y"]-height*.5+client[1]]
        point=source_point(value,*client)
        self.native("move",*screen)
        self.native("zoom-wheel",*screen,1)
        deadline=time.monotonic()+15
        while True:
            after=self.snapshot();overlay=after["document"]["preview_zoom_overlay"]
            sharp=((after.get("renderer") or {}).get("value") or {})
            if overlay["inputs"]>native["inputs"] and not overlay.get("active") and overlay.get("pending") is None and overlay.get("in_flight") is None and sharp.get("ready"):break
            if time.monotonic()>deadline:raise AssertionError("positioned wheel did not complete native zoom")
            time.sleep(.05)
        actual=project_source_point(sharp,point)
        error=[a-b for a,b in zip(actual,client)]
        if any(abs(e)>1.1 for e in error):raise AssertionError(f"zoom moved the original document point away from the cursor: {error}")
        anchor=overlay.get("source_anchor")
        if not anchor or abs(anchor["clientX"]-client[0])>1 or abs(anchor["clientY"]-client[1])>1:raise AssertionError("native wheel anchor used the center instead of the cursor")
        if overlay["hits"]<=native["hits"]:raise AssertionError("positioned wheel did not use cached pixels")
        self.record("partial.cursor-wheel",mode=mode,client=client,source_point=point,anchor_error_css_pixels=error,native_before=native,native_after=overlay)
        self.key(29,"cmd")
        self.wait("cursor zoom restores fit",lambda d:abs(d["preview_zoom"]-1)<.001)

    def cached_divider_resize(self, mode):
        # Real divider input and intermediate native observations. Kept separate
        # from timing: inspection itself must not become a performance workload.
        from concurrent.futures import ThreadPoolExecutor
        # Repeat without reloading: late duplicate DOM resize notifications must
        # neither cancel a sharp handoff nor leave an ownerless active gesture.
        for delta in (220, -220) * 3:
            self.wait_target("divider.editor")
            deadline = time.monotonic() + 15
            while True:
                before = self.snapshot()
                native = before["document"]["preview_zoom_overlay"]
                if native.get("cached") and not native.get("active"):
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError("divider resize has no settled native image")
                time.sleep(.05)
            original=((before.get("renderer") or {}).get("value") or {})
            top=original["viewport_bounds"][1]
            # Independent live-document source point at the visible top. This
            # does not derive the desired anchor from the cached transform.
            top_point=source_point(original,original["viewport_bounds"][0],top)
            border = before["targets"]["divider.editor"]
            source = before["document"]["source_fingerprint"]
            active_frames = 0
            with ThreadPoolExecutor(max_workers=1) as worker:
                drag = worker.submit(self.native, "drag", border["x"], border["y"],
                                     border["x"] + delta, border["y"])
                while not drag.done():
                    observed = self.snapshot()["document"]["preview_zoom_overlay"]
                    active_frames += int(observed.get("active", False))
                drag.result()
            deadline = time.monotonic() + 15
            while True:
                after = self.snapshot()
                overlay = after["document"]["preview_zoom_overlay"]
                renderer = after.get("renderer") or {}
                if (not overlay.get("active") and overlay.get("in_flight") is None
                        and overlay.get("pending") is None and renderer.get("age_ms",1000)<500
                        and (renderer.get("value") or {}).get("ready")):
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError("divider bitmap did not hand off to current sharp SVG")
                time.sleep(.05)
            if overlay["hits"] <= native["hits"] or overlay["handoffs"] <= native["handoffs"]:
                raise AssertionError("divider resized without using the native cached image")
            if after["document"]["source_fingerprint"] != source:
                raise AssertionError("preview divider changed source")
            if abs(after["targets"]["divider.editor"]["x"]-border["x"]-delta)>2:
                raise AssertionError("preview divider did not follow the drag")
            mask=overlay.get("mask_bounds")
            view=overlay.get("view_size")
            if not mask or not view or mask[:2] != [0,0] or any(abs(a-b)>.01 for a,b in zip(mask[2:],view)):
                raise AssertionError("cached preview left part of the native viewport unmasked")
            sharp = renderer.get("value") or {}
            top_error=project_source_point(sharp,top_point)[1]-sharp["viewport_bounds"][1]
            if abs(top_error)>1.1:raise AssertionError(f"divider moved the original top-of-view source point: {top_error}px")
            anchor = overlay.get("source_anchor")
            if anchor:
                bounds = sharp.get("page_bounds") or []
                scale = sharp.get("paper_scale")
                paper = sharp.get("paper_bounds")
                if not bounds or not scale or not paper:
                    raise AssertionError("divider handoff has no sharp page geometry")
                point = anchor["y"]
                if "page" in point:
                    projected_y = bounds[point["page"]][1] + point["local"] * scale
                else:
                    before = bounds[point["before"]]
                    end = before[1] + before[3]
                    projected_y = end + point["fraction"] * (bounds[point["after"]][1] - end)
                anchor_error = [paper[0] + anchor["x"] * scale - anchor["clientX"],
                                projected_y - anchor["clientY"]]
            else:
                target_scroll = overlay.get("target_scroll")
                sharp_scroll = sharp.get("scroll")
                if not target_scroll or not sharp_scroll:
                    raise AssertionError("divider handoff has no native/sharp scroll geometry")
                anchor_error = [sharp_scroll[0]-target_scroll["left"], sharp_scroll[1]-target_scroll["top"]]
            if any(abs(error)>1.1 for error in anchor_error):
                raise AssertionError(f"divider sharp handoff moved away from cached content: {anchor_error}")
            self.record("partial.cached-divider",mode=mode,delta=delta,
                        active_frames=active_frames,anchor_error_css_pixels=anchor_error,top_source_error_css_pixels=top_error,
                        native_before=native,native_after=overlay)

        # Releasing the divider must leave keyboard ownership transferable.
        # A stuck native mask/focus must not consume source typing shortcuts.
        self.click("editor.source")
        self.wait("editor owns keyboard after divider release",lambda d:
                  d["editor_keyboard_focused"] and not d["preview_keyboard_focused"])
        self.record("partial.divider-focus-release",mode=mode)

    def partial_preview(self, stress=False):
        self.focus_document_window()
        before = self.snapshot()["document"]
        name = "tinymist-zoom-stress.typ" if stress else "tinymist-partial-large.typ"
        source = (Path(__file__).resolve().parent / "fixtures" / name).read_text()
        # ISO paper dimensions are fractional in points, unlike the old fixture.
        source = source.replace("width: 500pt, height: 680pt", "width: 210mm, height: 297mm")
        fixture = self.directory / ("partial-zoom-stress.typ" if stress else "partial-preview.typ")
        fixture.write_text(source)
        # Native preview creation can finish just after the initial root focus.
        # Establish the intended input owner before posting the Open command.
        self.click("editor.source")
        self.wait("editor owns keyboard before native Open", lambda d: d["editor_keyboard_focused"])
        self.open_harper_fixture(fixture)
        created_tabs = [self.snapshot()["document"]["active_tab_id"]]
        self.key(35, "cmd+alt")  # Designate this tab as the preview entry.
        self.key(20)  # Split view.
        self.choose_backend("InteractivePartial", save_pdf=False)
        self.wait("large partial preview has all page skeletons", lambda d:
                  d["preview_path"] == str(fixture) and d["preview_pages"] == 48, timeout=60)

        def covered(description, expected=None):
            deadline = time.monotonic() + 15
            while True:
                state = self.snapshot()
                try:
                    value = assert_partial_coverage(state.get("renderer"))
                    if expected is not None:
                        assert_zoom_geometry(state.get("renderer"), baseline, expected)
                    self.record("partial.coverage", phase=description,
                                populated=value["populated"], visible=value["visible"], canvases=value["canvases"])
                    return state
                except AssertionError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.05)

        covered("initial")
        def cached_state(description, predicate):
            deadline = time.monotonic() + 15
            while True:
                state = self.snapshot()
                renderer = state.get("renderer") or {}
                cache = (renderer.get("value") or {}).get("zoom_snapshot") or {}
                if renderer.get("age_ms", 1000) <= 500 and predicate(cache):
                    self.record("partial.zoom-cache", phase=description, state=cache)
                    return cache
                if time.monotonic() >= deadline:
                    raise AssertionError(description + ": viewport cache did not reach the required state")
                time.sleep(.05)
        initial_cache = cached_state("initial-higher-resolution", lambda cache: cache.get("enabled") and cache.get("cached"))
        if math.prod(initial_cache["pixels"]) > 4_000_000:
            raise AssertionError("viewport image exceeds the four-million-pixel cap")
        self.settings()
        self.setting_section("Smooth Tinymist zoom", "Smooth Tinymist zoom")
        self.click("settings.preview.zoom_snapshot")
        self.close_settings()
        cached_state("disabled-releases-image", lambda cache: not cache.get("enabled") and not cache.get("cached") and not cache.get("active"))
        self.settings()
        self.setting_section("Smooth Tinymist zoom", "Smooth Tinymist zoom")
        self.click("settings.preview.zoom_snapshot")
        self.close_settings()
        cached_state("reenabled-captures", lambda cache: cache.get("enabled") and cache.get("cached"))
        self.click("preview.page")
        self.wait("native partial preview owns keyboard", lambda d: d["preview_keyboard_focused"])
        self.key(119, "none")  # End: far outside the original buffer.
        self.wait("End reaches last page", lambda d: d["preview_page"] == 47, timeout=15)
        covered("end")
        self.click("preview.open")
        self.wait("native controls open for distant return", lambda d: d["preview_controls_open"])
        self.click("preview.Outline")
        self.wait("partial outline opens", lambda d: d["preview_outline"])
        self.click("preview.outline.0")
        self.wait("outline returns through deferred pages", lambda d: d["preview_page"] == 0, timeout=15)
        covered("outline-first")
        self.click("preview.Minimize controls")
        self.click("preview.page")
        target = self.snapshot()["targets"]["preview.page"]
        for direction in (-40, 40, -80, 80):
            self.native("wheel", target["x"], target["y"], direction)
            covered("wheel")
        self.click("preview.open")
        self.wait("native controls open in partial mode", lambda d: d["preview_controls_open"])
        self.click("preview.Fit page width")
        self.wait("partial preview fits width", lambda d: abs(d["preview_zoom"] - 1) < 0.001)
        baseline = covered("fit")["renderer"]["value"]
        for action, count in (("Zoom in", 6), ("Zoom out", 6)):
            for step in range(count):
                self.click("preview." + action)
                expected = 1.1 ** (step + 1 if action == "Zoom in" else 5 - step)
                self.wait("partial zoom steps accumulate", lambda d: abs(d["preview_zoom"] - expected) < 0.001)
                covered("zoom-" + action + "-" + str(step), expected)
        self.click("preview.Minimize controls")
        self.click("preview.page")
        # Admit an idle image before inputs; measure new keyboard hits rather
        # than accidentally accepting a hit from earlier button interactions.
        keyboard_cache = cached_state("keyboard-idle-image", lambda cache: cache.get("cached") and not cache.get("active"))
        native_before = self.snapshot()["document"]["preview_zoom_overlay"]
        # Native keyboard events exercise the native owner rather than JS calls.
        for step in range(4):
            self.key(24, "cmd")
            expected = 1.1 ** (step + 1)
            self.wait("native partial keyboard zoom accumulates", lambda d: abs(d["preview_zoom"] - expected) < 0.001)
            covered("keyboard-zoom-" + str(step), expected)
        cached_state("native-keyboard-cache-hit", lambda cache: cache.get("hits", 0) > keyboard_cache["hits"] and cache.get("handoffs", 0) > keyboard_cache["handoffs"] and not cache.get("active"))
        native_after = self.snapshot()["document"]["preview_zoom_overlay"]
        if native_after["inputs"] < native_before["inputs"] + 4 or native_after["hits"] <= native_before["hits"]:
            raise AssertionError("keyboard zoom did not pass through the native cached-image compositor")
        self.record("partial.native-zoom", before=native_before, after=native_after)
        self.key(29, "cmd")
        self.wait("native partial fit restores scale", lambda d: abs(d["preview_zoom"] - 1) < 0.001)
        covered("keyboard-fit", 1)
        if stress:
            self.zoom_burst(baseline, "partial")
            covered("burst-fit", 1)
            self.cached_edge_wheel("partial")
            self.cached_divider_resize("partial")
        self.click("preview.open")
        self.click("preview.Minimize controls")
        # The dense stress case exceeds full WebKit's bounded probe budget with
        # both preserved and current adapters. Keep that failure in the audit;
        # smoke-test full zoom on the standard 48-page fixture, then return to
        # the dense entry to prove that backend switching restores its buffer.
        stress_tab = None
        if stress:
            stress_tab = self.snapshot()["document"]["active_tab_id"]
            full_fixture = self.directory / "full-preview.typ"
            full_source = (Path(__file__).resolve().parent / "fixtures" / "tinymist-partial-large.typ").read_text()
            full_fixture.write_text(full_source.replace("width: 500pt, height: 680pt", "width: 210mm, height: 297mm"))
            self.click("editor.source")
            self.wait("editor owns keyboard before full fixture Open", lambda d: d["editor_keyboard_focused"])
            self.open_harper_fixture(full_fixture)
            created_tabs.append(self.snapshot()["document"]["active_tab_id"])
            self.key(35, "cmd+alt")
        # Both Tinymist modes remain independently selectable and effective.
        self.choose_backend("Interactive", save_pdf=False)
        self.wait("complete renderer remains available", lambda d:
                  d["preview_backend"] == "Tinymist" and d["preview_pages"] == 48, timeout=60)
        deadline = time.monotonic() + 15
        while True:
            state = self.snapshot()
            renderer = state.get("renderer") or {}
            value = renderer.get("value") or {}
            if (renderer.get("age_ms", 1000) <= 500 and value.get("ready")
                    and not value.get("partial") and value.get("pages") == 48
                    and value.get("populated") == 48):
                self.record("partial.full-restored", pages=value["pages"], populated=value["populated"], fixture="standard" if stress else "fractional")
                break
            if time.monotonic() >= deadline:
                raise AssertionError("actual complete Tinymist renderer did not replace partial mode")
            time.sleep(0.05)
        full_baseline = value
        full_cache = cached_state("full-idle-image", lambda cache: cache.get("cached") and not cache.get("active"))
        self.click("preview.page")
        self.wait("full preview owns keyboard", lambda d: d["preview_keyboard_focused"])
        full_native_before = self.snapshot()["document"]["preview_zoom_overlay"]
        for step in range(3):
            self.key(24, "cmd")
            expected = 1.1 ** (step + 1)
            self.wait("full keyboard zoom accumulates", lambda d: abs(d["preview_zoom"] - expected) < 0.001)
            deadline = time.monotonic() + 15
            while True:
                state = self.snapshot()
                try:
                    value = assert_zoom_geometry(state.get("renderer"), full_baseline, expected)
                    if value.get("partial") or value.get("populated") != 48:
                        raise AssertionError("full zoom changed renderer or lost document ink")
                    self.record("partial.full-zoom", step=step, zoom=value["zoom"], paper_scale=value["paper_scale"])
                    break
                except AssertionError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.05)
        cached_state("full-keyboard-cache-hit", lambda cache: cache.get("hits", 0) > full_cache["hits"] and cache.get("handoffs", 0) > full_cache["handoffs"] and not cache.get("active"))
        full_native_after = self.snapshot()["document"]["preview_zoom_overlay"]
        if full_native_after["inputs"] < full_native_before["inputs"] + 3 or full_native_after["hits"] <= full_native_before["hits"]:
            raise AssertionError("full keyboard zoom did not pass through the native cached-image compositor")
        self.record("partial.full-native-zoom", before=full_native_before, after=full_native_after)
        self.key(29, "cmd")
        self.wait("full fit restores scale", lambda d: abs(d["preview_zoom"] - 1) < 0.001)
        if stress:
            self.zoom_burst(full_baseline, "full")
            self.cached_edge_wheel("full")
            self.cached_divider_resize("full")
        # A divider drag makes the root native view the input owner. Reclaim
        # the preview before sending a viewer-only navigation shortcut.
        self.click("preview.page")
        self.wait("full preview owns keyboard after divider drag", lambda d: d["preview_keyboard_focused"])
        self.key(119, "none")
        self.wait("full page tracker reaches last paper", lambda d: d["preview_page"] == 47, timeout=15)
        self.record("partial.full-page-tracker", page=47)
        self.choose_backend("InteractivePartial", save_pdf=False)
        if stress_tab is not None:
            self.click("tab." + str(stress_tab))
            self.key(35, "cmd+alt")
            self.wait("dense entry returns after full mode", lambda d:
                      d["preview_path"] == str(fixture) and d["preview_pages"] == 48, timeout=60)
        covered("reopened-partial")
        self.restore_partial_fixture_tabs(before, created_tabs)

    def restore_partial_fixture_tabs(self, before, created_tabs):
        # Closing the active scratch tab can select an original tab rather than
        # the other scratch tab. Select each known fixture explicitly so count
        # based cleanup never closes a member of the original session.
        self.focus_document_window()
        for tab_id in reversed(created_tabs):
            self.click("tab." + str(tab_id))
            self.click("editor.source")
            self.close_scratch(self.snapshot()["document"]["tabs"] - 1)
        if before["preview_tab_id"] is not None:
            self.click("tab." + str(before["preview_tab_id"]))
            self.key(35, "cmd+alt")
        if before["active_tab_id"] is not None:
            self.click("tab." + str(before["active_tab_id"]))
        # Restore the backend only after the original entry is selected. Do not
        # start another dense full render just to close its fixture immediately.
        if self.snapshot()["document"]["preview_requested_backend"] != before["preview_requested_backend"]:
            self.choose_backend(before["preview_requested_backend"], save_pdf=False)
