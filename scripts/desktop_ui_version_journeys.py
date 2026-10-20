"""Native macOS local-version journeys; only disposable named files are edited."""

import math
import re


def version_history(document):
    """Require the live browser contract; missing inspection never means closed."""
    state = document.get("version_history")
    required = {"visible", "loading", "count", "left", "right", "selected", "can_restore", "error", "rendering", "preview"}
    if not isinstance(state, dict) or not required.issubset(state) or any(
            type(state.get(key)) is not bool for key in ("visible", "loading", "rendering")):
        raise AssertionError("local version browser inspection is unavailable")
    count = state.get("count")
    if type(count) is not int or count < 0:
        raise AssertionError("invalid local version count")
    for side in ("left", "right"):
        value = state.get(side)
        if value is not None and (not isinstance(value, str)
                                  or re.fullmatch(r"[0-9a-f]{16}", value) is None):
            raise AssertionError("invalid local version fingerprint: " + side)
    selected = state.get("selected")
    if not isinstance(selected, list) or len(selected) != 2 or any(
            index is not None and (type(index) is not int or not 0 <= index < count)
            for index in selected):
        raise AssertionError("invalid local version selection")
    enabled = state.get("can_restore")
    if not isinstance(enabled, list) or len(enabled) != 2 or any(type(item) is not bool
                                                                             for item in enabled):
        raise AssertionError("invalid local version restore availability")
    preview = state["preview"]
    if preview is not None:
        if not isinstance(preview, dict) or type(preview.get("side")) is not int \
                or preview["side"] not in (0, 1) or type(preview.get("pages")) is not int \
                or preview["pages"] < 0 or not isinstance(preview.get("source_fingerprint"), str) \
                or re.fullmatch(r"[0-9a-f]{16}", preview["source_fingerprint"]) is None:
            raise AssertionError("invalid historical PDF preview identity")
    if state.get("error") is not None:
        raise AssertionError("local version browser failed: " + str(state["error"]))
    if not state["visible"] and (state["loading"] or state["rendering"] or preview is not None
                                  or count or selected != [None, None]
                                  or enabled != [False, False]
                                  or any(state[side] is not None for side in ("left", "right"))):
        raise AssertionError("closed local version browser retained live state")
    return state


def version_pair(document, older, newer):
    from desktop_ui_journeys import fingerprint
    state = version_history(document)
    return (state["visible"] and not state["loading"]
            and state["left"] == fingerprint(older) and state["right"] == fingerprint(newer)
            and state["selected"][0] is not None and state["selected"][1] is None)


def docked_history(state):
    """Require the real code-panel comparison and a separate visible preview."""
    document = state["document"]
    if not version_history(document)["visible"] or document.get("view_mode") != "Split":
        raise AssertionError("version comparison is not in the split workspace")
    targets = []
    for name in ("versions.panel", "versions.diff", "preview.page"):
        target = state.get("targets", {}).get(name)
        if not isinstance(target, dict) or target.get("viewport") != document.get("viewport") \
                or target.get("frame") != document.get("frame") \
                or type(target.get("age_ms")) is not int or not 0 <= target["age_ms"] <= 500 \
                or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                       for key in ("x", "y", "width", "height")) \
                or target["width"] <= 0 or target["height"] <= 0:
            raise AssertionError("missing or stale docked history geometry: " + name)
        targets.append(target)
    panel, diff, preview = targets
    left = lambda r: r["x"] - r["width"] / 2
    right = lambda r: r["x"] + r["width"] / 2
    top = lambda r: r["y"] - r["height"] / 2
    bottom = lambda r: r["y"] + r["height"] / 2
    if right(panel) > left(preview) + 1 \
            or left(diff) < left(panel) - 1 or right(diff) > right(panel) + 1 \
            or top(diff) < top(panel) - 1 or bottom(diff) > bottom(panel) + 1:
        raise AssertionError("version diff escaped the code panel or covered its preview")
    return True


def version_pdf_ready(document, side, source, pages):
    from desktop_ui_journeys import fingerprint
    state = version_history(document)
    preview = state["preview"]
    return (state["visible"] and not state["rendering"] and preview is not None
            and preview["side"] == side and preview["source_fingerprint"] == fingerprint(source)
            and preview["pages"] == pages and pages > 0
            and document.get("preview_pages") == pages
            and document.get("preview_surface_visible") is False)


def version_restored(document, path, source, tabs, tab):
    from desktop_ui_journeys import fingerprint
    return (not version_history(document)["visible"] and document["path"] == str(path)
            and document["tabs"] == tabs and document["active_tab_id"] == tab
            and document["source_fingerprint"] == fingerprint(source) and not document["dirty"]
            and document.get("view_mode") == "Split" and path.read_text() == source)


class VersionJourneys:
    def open_version_history(self):
        self.focus_document_window()
        self.key(122, "cmd+alt")  # The app's File-menu shortcut, not a handler call.
        self.wait_target("menu.Revert To…")
        self.click("menu.Revert To…")
        return self.wait("local version browser loads its actual native revisions", lambda d:
                         d["view_mode"] == "Split" and version_history(d)["visible"]
                         and not version_history(d)["loading"]
                         and version_history(d)["count"] > 0, timeout=20)["document"]

    def choose_saved_version(self, source, current):
        """Select actual menu rows; revision indices are not assumed stable."""
        from desktop_ui_journeys import fingerprint
        count = version_history(self.snapshot()["document"])["count"]
        for index in range(count):
            self.click("versions.select.older")
            row = "versions.choice.older." + str(index)
            self.wait_target(row)
            self.click(row)
            selected = self.wait("selected native revision comparison finishes", lambda d:
                                 version_history(d)["visible"] and not version_history(d)["loading"]
                                 and version_history(d)["selected"] == [index, None],
                                 timeout=20)["document"]
            if version_pair(selected, source, current):
                self.record("versions.selected", index=index,
                            older_fingerprint=fingerprint(source),
                            newer_fingerprint=fingerprint(current))
                return selected
        raise AssertionError("the saved source was not retained in the native revision store")

    def render_version_pdf(self, side, path, older, current):
        """Render the selected side into the live preview without a Save panel."""
        from desktop_ui_journeys import fingerprint
        source = (older, current)[side]
        self.click("versions." + ("older" if side == 0 else "newer") + ".pdf")
        self.wait("selected version is rendered in the visible PDF preview", lambda d:
                  version_pdf_ready(d, side, source, (2, 3)[side]) and version_pair(d, older, current)
                  and d["path"] == str(path) and path.read_text() == current
                  and d["source_fingerprint"] == fingerprint(current) and not d["dirty"]
                  and version_history(d)["can_restore"] == [True, False], timeout=60)
        self.wait_target("preview.page")
        observed = self.snapshot()
        docked_history(observed)
        self.record("versions.pdf_preview", side=side, source_fingerprint=fingerprint(source),
                    pages=version_history(observed["document"])["preview"]["pages"])

    def file_versions(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()["document"]
        path = self.directory / "versions.typ"
        if path.exists():
            raise AssertionError("version journey requires a fresh disposable document")
        initial = "= Version fixture\nOriginal paragraph.\n"
        first = "= Version fixture\nFirst saved paragraph.\n#pagebreak()\nFirst saved appendix.\n"
        second = "= Version fixture\nSecond saved paragraph.\n#pagebreak()\nSecond saved appendix.\n#pagebreak()\nThird saved page.\n"
        path.write_text(initial)
        # The existing helper drives macOS's Open panel by full path.
        self.open_harper_fixture(path)
        loaded = self.snapshot()["document"]
        tabs, tab = loaded["tabs"], loaded["active_tab_id"]
        if tabs != before["tabs"] + 1:
            raise AssertionError("named version fixture did not open its own tab")
        for name, source in (("first", first), ("second", second)):
            self.click("editor.source")
            self.key(0)
            self.paste_source(source)
            self.key(1)  # Cmd+S: native revisions must be created by actual saves.
            self.wait(name + " source is durably saved", lambda d:
                      d["path"] == str(path) and not d["dirty"]
                      and d["source_fingerprint"] == fingerprint(source)
                      and path.read_text() == source, timeout=20)
            self.record("versions.saved", version=name, source_fingerprint=fingerprint(source))

        self.key(35, "cmd+alt")  # The live preview belongs to this named fixture.
        self.wait("current version has a complete three-page live preview", lambda d:
                  d["preview_tab_id"] == tab and d["preview_path"] == str(path)
                  and d["preview_pages"] == 3 and d["source_fingerprint"] == fingerprint(second),
                  timeout=45)
        self.key(19)  # History opened from code-only mode must reveal the preview alongside it.
        self.wait("version fixture starts in code-only view", lambda d: d["view_mode"] == "Code")
        opened = self.open_version_history()
        if opened["view_mode"] != "Split":
            raise AssertionError("opening local history did not preserve a visible preview pane")
        if version_history(opened)["count"] < 2:
            raise AssertionError("two explicit saves did not retain previous native revisions")
        self.choose_saved_version(first, second)
        self.stable_state(lambda d: version_pair(d, first, second)
                          and version_history(d)["can_restore"] == [True, False])
        self.wait_target("versions.diff")
        self.click("versions.diff")
        self.native("text", "must-not-edit")
        self.stable_state(lambda d: version_pair(d, first, second)
                          and d["source_fingerprint"] == fingerprint(second)
                          and path.read_text() == second, frames=8)
        self.record("versions.readonly_typing_ignored", source_fingerprint=fingerprint(second))
        self.render_version_pdf(0, path, first, second)
        windows = self.native("windows")["windows"]
        if any("Revert To" in window.get("AXTitle", "") for window in windows):
            raise AssertionError("version comparison opened a separate native window")
        if self.capture_review:
            self.capture_viewport("main")
        self.render_version_pdf(1, path, first, second)
        self.click("versions.older.restore")
        self.wait("Restore replaces the complete buffer and disk with the older version", lambda d:
                  version_restored(d, path, first, tabs, tab), timeout=20)
        self.wait("restored older source becomes the live two-page preview", lambda d:
                  version_restored(d, path, first, tabs, tab) and d["preview_pages"] == 2, timeout=45)

        self.open_version_history()
        self.choose_saved_version(second, first)
        self.click("versions.older.restore")
        self.wait("the pre-restore newer version remains recoverable", lambda d:
                  version_restored(d, path, second, tabs, tab), timeout=20)
        self.wait("restored newer source becomes the live three-page preview", lambda d:
                  version_restored(d, path, second, tabs, tab) and d["preview_pages"] == 3, timeout=45)

        for code, flags, name in ((53, "none", "Escape"), (13, "cmd", "Cmd+W"),
                                   (None, None, "Close button")):
            self.open_version_history()
            if name == "Escape":
                self.choose_saved_version(first, second)
                self.render_version_pdf(0, path, first, second)
            if code is None:
                self.click("versions.close")
            else:
                self.key(code, flags)
            self.wait(name + " closes only the version browser", lambda d:
                      version_restored(d, path, second, tabs, tab) and d["focused"], timeout=15)
            self.wait("closing local history restores the current live preview", lambda d:
                      version_restored(d, path, second, tabs, tab) and d["preview_pages"] == 3,
                      timeout=45)
            self.stable_state(lambda d: version_restored(d, path, second, tabs, tab), frames=4)
        self.click("editor.source")
        self.key(13)
        self.wait("version journey closes only its disposable document tab", lambda d:
                  d["tabs"] == before["tabs"] and d["path"] == before["path"]
                  and d["source_fingerprint"] == before["source_fingerprint"], timeout=15)
