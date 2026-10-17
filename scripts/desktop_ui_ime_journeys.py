"""Native IME geometry and CJK input; input sources are always restored."""
import json
import math
import select
import subprocess
import time
from contextlib import contextmanager

PINYIN = "com.apple.inputmethod.SCIM.ITABC"


def assert_native_ime_at_caret(state, composing=False):
    document = state["document"]
    native = document.get("native_ime_area")
    target = state["targets"].get("editor.ime_cursor" if composing else "editor.caret")
    if not document["editor_keyboard_focused"] or not native or not native["first_responder"]:
        raise AssertionError("source editor is not the actual AppKit text-input owner")
    if native["source"] != "NSTextInputClient.firstRectForCharacterRange":
        raise AssertionError("IME area was not read from the native text-input client")
    if not target or target["age_ms"] > 500 or target["frame"] != document["frame"]:
        raise AssertionError("missing fresh source caret geometry")
    viewport = state["targets"].get("editor.source")
    if not viewport or viewport["age_ms"] > 500 or viewport["frame"] != document["frame"]:
        raise AssertionError("missing fresh visible source viewport")
    geometry = [viewport[key] for key in ("x", "y", "width", "height")] + [target["x"], target["y"]]
    if not all(isinstance(value, (int, float)) and not isinstance(value, bool)
               and math.isfinite(value) for value in geometry) \
            or viewport["width"] <= 0 or viewport["height"] <= 0:
        raise AssertionError("source viewport contains invalid geometry")
    if not viewport["x"] - viewport["width"] / 2 <= target["x"] <= viewport["x"] + viewport["width"] / 2 \
            or not viewport["y"] - viewport["height"] / 2 <= target["y"] <= viewport["y"] + viewport["height"] / 2:
        raise AssertionError("IME caret is outside the visible source viewport")
    rect = native["rect"]
    if len(rect) != 4 or not all(isinstance(v, (int, float)) and not isinstance(v, bool)
                                 and math.isfinite(v) for v in rect):
        raise AssertionError("native IME area contains invalid geometry")
    left, top, width, height = rect
    scale = document["interface_scale"]
    if not (0 <= width <= 4 * scale and 4 <= height <= 100 * scale):
        raise AssertionError("native IME hint encloses the editor instead of its caret")
    if abs(target["x"] - (left + width / 2)) > 2 * scale \
            or not top - 2 * scale <= target["y"] <= top + height + 2 * scale:
        raise AssertionError("native IME candidate anchor does not follow the visible caret")
    return rect


def pinyin_source_matches(document, before, after, committed):
    # A Pinyin preedit may expose a collapsed caret. Exact phonetic text followed
    # by conversion to Chinese proves composition without assuming a selection.
    from desktop_ui_journeys import fingerprint
    candidates = ("你好",) if committed else ("nihao", "ni hao", "ni'hao")
    return any(document["source_fingerprint"] == fingerprint(before + text + after)
               and document["source_bytes"] == len((before + text + after).encode("utf-8"))
               for text in candidates)


def finish_input_source_scope(process):
    """Restore on success, failure, app crash or Python pipe closure."""
    try:
        output, error = process.communicate("restore\n", timeout=6)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate(timeout=2)
        raise RuntimeError("input-source adapter timed out; restoration could not be verified") from None
    if process.returncode:
        raise RuntimeError(error or "input-source restoration failed")
    try:
        receipt = json.loads(output.strip())
    except (ValueError, TypeError):
        raise RuntimeError("input-source adapter did not acknowledge restoration") from None
    if not isinstance(receipt, dict) or receipt.get("restored") is not True \
            or not isinstance(receipt.get("original"), str) or not receipt["original"] \
            or receipt.get("current") != receipt["original"]:
        raise RuntimeError("original input source was not restored; any external source choice was preserved")
    return receipt


class ImeJourneys:
    @contextmanager
    def pinyin_input(self):
        self.focus_document_window()
        available = self.native("input-sources")
        if not any(source["id"] == PINYIN and source["enabled"] and source["selectable"]
                   for source in available["sources"]):
            raise RuntimeError("native CJK journey unavailable: Pinyin - Simplified is not already enabled; no input settings were changed")
        process = subprocess.Popen(
            [str(self.driver), "input-source-scope", str(self.process.pid), PINYIN],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        try:
            if not select.select([process.stdout], [], [], 6)[0]:
                raise RuntimeError("input-source adapter did not acknowledge selection")
            receipt = json.loads(process.stdout.readline())
            if receipt.get("selected") != PINYIN or receipt.get("original") != available["selected"]:
                raise RuntimeError("input-source adapter selected an unexpected mode")
            self.record("ime.source.selected", source=PINYIN, original=receipt["original"])
            yield
        finally:
            receipt = finish_input_source_scope(process)
            self.record("ime.source.restored", source=receipt["current"])

    def wait_native_ime(self, label, composing=False):
        deadline = time.monotonic() + 6
        while True:
            state = self.snapshot()
            try:
                rect = assert_native_ime_at_caret(state, composing=composing)
                self.record("ime.native_hint", label=label, rect=rect,
                            frame=state["document"]["frame"])
                return state
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)

    def set_ascii_punctuation(self, enabled):
        if self.snapshot()["document"]["ascii_punctuation"] is enabled:
            return
        self.settings()
        self.setting_section("CJK punctuation", "Convert CJK punctuation to ASCII when typing")
        self.click("settings.editor.ascii")
        self.wait("CJK punctuation preference applied", lambda d:
                  d["ascii_punctuation"] is enabled)
        self.close_settings()
        self.click("editor.source")

    def set_ime_interface_scale(self, percent):
        current = round(self.snapshot()["document"]["interface_scale"] * 100)
        if current == percent:
            return
        self.tex_settings(((f'"ui_scale_percent": {current}',
                            f'"ui_scale_percent": {percent}'),))
        self.wait("interface scale changed through Settings", lambda d:
                  abs(d["interface_scale"] - percent / 100) < 0.001)

    def ime(self):
        # A scrolled long buffer makes the former full-editor IME rectangle
        # extend far below the window. Query AppKit itself after each caret move.
        lines = [f"L{index:03}.\n" for index in range(200)]
        source = "".join(lines)
        count = self.scratch(source)
        self.key(126, "cmd")
        self.wait("IME fixture caret starts at document top", lambda d: d["cursor"] == [0, 0])
        # Scroll explicitly after the large paste; source/caret agreement alone
        # is insufficient if both describe a position outside the viewport.
        for _ in range(30):
            scroll = self.snapshot()["scrolls"]["editor"]
            if scroll["offset"] < 1:
                break
            self.native("move", scroll["x"], scroll["y"])
            self.native("wheel", scroll["x"], scroll["y"], 100)
        self.wait_native_ime("document-top")
        self.native("key-burst", 125, "none", 40, 10)
        offset = sum(len(line) for line in lines[:40])
        self.wait("IME caret scrolls to the middle of a large buffer", lambda d:
                  d["cursor"] == [offset, offset])
        self.wait_native_ime("scrolled-line-40")
        with self.pinyin_input():
            # Physical keys go through the selected IME. Unicode injection or
            # clipboard text would only test committed strings, not composition.
            for keycode in (45, 34, 4, 0, 31):  # n i h a o
                self.key(keycode, "none")
            self.wait("exact phonetic Pinyin input reached the document", lambda d:
                      pinyin_source_matches(d, source[:offset], source[offset:], committed=False))
            self.wait_native_ime("pinyin-composing", composing=True)
            if self.review_preview:
                self.review_native_preview("cjk-ime-candidates")
            self.key(49, "none")  # Space chooses the first Pinyin candidate.
            self.wait("Space converts Pinyin into the exact Chinese candidate", lambda d:
                      pinyin_source_matches(d, source[:offset], source[offset:], committed=True))
            self.wait_native_ime("pinyin-committed")
        original_scale = round(self.snapshot()["document"]["interface_scale"] * 100)
        try:
            self.set_ime_interface_scale(125)
            self.wait_native_ime("interface-125-percent")
        finally:
            self.set_ime_interface_scale(original_scale)
        self.close_scratch(count)

        # Test native committed text separately from IME conversion so this
        # preference check does not depend on the user's IME punctuation mode.
        original = self.snapshot()["document"]["ascii_punctuation"]
        try:
            for enabled, expected in ((False, "。！"), (True, ".!")):
                self.set_ascii_punctuation(enabled)
                count = self.scratch()
                self.native("text", "。！")
                self.source_is(expected)
                self.wait_native_ime("punctuation-" + str(enabled).lower())
                self.close_scratch(count)
        finally:
            self.set_ascii_punctuation(original)
