#!/usr/bin/env python3
"""Real macOS editor journeys. Requires Accessibility and input-posting permission.

No handler calls, fixture scenes, silent skips, or retries of failed journeys.
Evidence is semantic/native state, NOT composed visual verification.
"""
import argparse
import hashlib
import json
import math
import os
import re
from pathlib import Path
import platform
import plistlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.dont_write_bytecode = True
from desktop_ui_journeys import EditorJourneys, PREVIEW_SOURCE, fingerprint
from desktop_ui_tex_journeys import TEX_JOURNEYS, TEX_SOURCE
from desktop_ui_image_journeys import IMAGE_JOURNEYS
from desktop_ui_terminal_theme import terminal_theme
from desktop_ui_newest_journeys import NEWEST_JOURNEYS
from desktop_ui_effects_journeys import effects_pacing_fixture
from desktop_ui_liquid_journeys import LIQUID_JOURNEYS, liquid_model_directory
from desktop_ui_ltex_journeys import LTEX_JOURNEYS, ltex_program, ltex_environment
from desktop_ui_settings_links_journeys import SETTINGS_LINK_JOURNEYS
from desktop_ui_tool_defaults_journeys import TOOL_DEFAULTS_JOURNEYS
from desktop_ui_activity_journeys import ACTIVITY_JOURNEYS
from desktop_ui_indentation_journeys import INDENTATION_JOURNEYS
from desktop_ui_latexdiff_journeys import LATEXDIFF_JOURNEYS
from desktop_ui_texdoc_journeys import TEXDOC_JOURNEYS, texdoc_program
from desktop_ui_citation_journeys import CITATION_JOURNEYS, CITATION_METADATA, CITATION_EXISTING, CITATION_SOURCE
from desktop_ui_asset_layout_journeys import ASSET_LAYOUT_JOURNEYS
from desktop_ui_inactive_preview_journeys import INACTIVE_PREVIEW_JOURNEYS

ROOT = Path(__file__).resolve().parent.parent

JOURNEY_NAMES = (
    "all", *ACTIVITY_JOURNEYS, *INDENTATION_JOURNEYS, *TOOL_DEFAULTS_JOURNEYS, *SETTINGS_LINK_JOURNEYS, *LTEX_JOURNEYS, *LIQUID_JOURNEYS, *LATEXDIFF_JOURNEYS, *TEXDOC_JOURNEYS, *CITATION_JOURNEYS, *ASSET_LAYOUT_JOURNEYS, *INACTIVE_PREVIEW_JOURNEYS, "file_versions", "command_palette", "preview_jump", "tex", *TEX_JOURNEYS, *IMAGE_JOURNEYS,
    *NEWEST_JOURNEYS, "drawing", "snippets", "ime", "panels", "terminal_find",
    "terminal_load", "terminal_theme", "find", "rectangle", "empty", "focus", "preview",
    "pdf_output", "editing", "search", "tabs", "tab_drag", "dialogs", "startup_preview",
    "diagnostics", "completion", "layout", "explorer_toggle", "explorer_scroll", "folding",
    "selection_wrap", "git_hunks", "preview_keyboard", "pdfium_resize", "closing",
    "partial_preview", "partial_zoom_stress", "partial_cache_edges", "partial_dividers", "controls",
    "settings_search", "editor_preferences", "typing_particles", "effects_pacing", "background",
    "background_fades", "context_menu",
)


def selected_journeys(name):
    # Optional local ML must never become a hidden prerequisite of "all".
    if name == 'all':
        return [*ACTIVITY_JOURNEYS, *INDENTATION_JOURNEYS, *NEWEST_JOURNEYS, "file_versions", "command_palette", "preview_jump", "terminal_theme", "panels", "terminal_find", "terminal_load", "find", "editing", "search", "tabs", "tab_drag", "dialogs", "diagnostics", "completion", "layout", "pdf_layout", "explorer_toggle", "explorer_scroll", "folding", "selection_wrap", "git_hunks", "preview_keyboard", "pdfium_resize", "closing", "focus", "settings_search", *TOOL_DEFAULTS_JOURNEYS, "editor_preferences", "background", "background_fades", "context_menu", "pdf_output", "preview", "partial_preview", "controls", "empty", "snippets", "ime", "drawing", *IMAGE_JOURNEYS, *TEX_JOURNEYS]
    return list(TEX_JOURNEYS) if name == 'tex' else [name]


def height(document):
    rect = document.get("panel_rect")
    if not rect:
        raise AssertionError("panel has no measured rectangle")
    if len(rect) != 4 or not all(math.isfinite(value) for value in rect):
        raise AssertionError("panel geometry is invalid")
    return rect[3] - rect[1]


def assert_stable_height(baseline, document):
    actual = height(document)
    if abs(actual - baseline) > 0.5:
        raise AssertionError(f"panel drift: {baseline} -> {actual}")


def assert_tinymist_palette(renderer, palette):
    if not renderer or renderer["age_ms"] > 500:
        raise AssertionError("missing or stale Tinymist DOM observation")
    value = renderer["value"]
    if not value or not value.get("ready"):
        raise AssertionError("Tinymist has no visible page")
    background, foreground = palette
    identity_palette = (background[:3] == [255, 255, 255]
                        and foreground[:3] == [0, 0, 0])
    filters = value["filters"]
    expected_filter = (filters == ["none"] if identity_palette else
                       len(filters) == 1 and "tiptoptyp-palette" in filters[0])
    if not expected_filter:
        raise AssertionError("Tinymist page must receive exactly one appropriate palette filter")
    expected = "rgb(" + ", ".join(map(str, background[:3])) + ")"
    if value["background"] != expected:
        raise AssertionError("Tinymist paper differs from the effective palette")
    for channel in range(3):
        if abs(value["slopes"][channel] - (background[channel] - foreground[channel]) / 255) > 1e-6:
            raise AssertionError("Tinymist transfer slope differs from palette")
        if abs(value["intercepts"][channel] - foreground[channel] / 255) > 1e-6:
            raise AssertionError("Tinymist transfer intercept differs from palette")


def observe_failed_interaction(journey, result):
    """Read bounded failure evidence without activating or retrying the app."""
    for field, command, arguments in (
        ("failure_foreground", "foreground", [str(journey.process.pid)]),
        ("failure_preflight", "preflight", []),
        ("failure_native_windows", "windows", [str(journey.process.pid)]),
    ):
        try:
            probe = subprocess.run([str(journey.driver), command, *arguments],
                                   capture_output=True, text=True, timeout=6)
            result.setdefault("failure_observation_exits", {})[command] = probe.returncode
            # Preflight emits useful lock/permission state even on exit 2.
            observation = json.loads(probe.stdout)
            if not isinstance(observation, dict):
                raise RuntimeError("native observation did not return an object")
            result[field] = observation
            journey.record("failure.observation", command=command, exit=probe.returncode,
                           stdout=probe.stdout, stderr=probe.stderr)
        except Exception as error:
            result.setdefault("failure_observation_errors", {})[command] = str(error)


class Journey(EditorJourneys):
    terminal_theme = terminal_theme

    def __init__(self, process, driver, directory, evidence):
        self.process, self.driver, self.directory, self.evidence = process, driver, directory, evidence
        self.sequence = 0
        self.viewport = None
        self.multiple_windows = False
        self.review_preview = False
        self.log = (evidence / "events.jsonl").open("w")
        self.record_lock = threading.Lock()

    def record(self, action, **detail):
        # Native input and frame observation can run concurrently. Keep each
        # complete evidence line and its flush together in the shared stream.
        with self.record_lock:
            self.log.write(json.dumps({"time": time.monotonic(), "action": action, **detail}) + "\n")
            self.log.flush()

    def native(self, command, *args):
        if command in {"click", "double-click", "drag", "drag-start", "drag-move", "drag-end", "key", "key-burst", "move", "wheel", "pdfium-zoom-wheel", "text", "paste", "right-click", "fullscreen"}:
            foreground = subprocess.run(
                [str(self.driver), "foreground", str(self.process.pid)],
                capture_output=True,
                text=True,
                timeout=6,
            )
            if foreground.returncode == 0 and json.loads(foreground.stdout)["pid"] != self.process.pid:
                self.record("foreground.recovered", observed=json.loads(foreground.stdout))
                subprocess.run([str(self.driver), "activate", str(self.process.pid)], check=True, timeout=6)
        result = subprocess.run([str(self.driver), command, str(self.process.pid), *map(str, args)],
                                capture_output=True, text=True, timeout=6)
        self.record(command, arguments=({"fixture_text_bytes": len(args[0].encode("utf-8"))} if command == "text" else args), stdout=result.stdout, stderr=result.stderr, exit=result.returncode)
        if result.returncode:
            raise RuntimeError(result.stderr or result.stdout)
        return json.loads(result.stdout)

    def snapshot(self, viewport=None):
        if self.process.poll() is not None:
            raise RuntimeError(f"app exited unexpectedly: {self.process.returncode}")
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(4)
            connection.connect(str(self.directory / "inspect.sock"))
            command = "snapshot" if viewport is None else f"snapshot {viewport}"
            connection.sendall((command + "\n").encode("ascii"))
            data = connection.makefile("rb").readline(1_000_000)
        state = json.loads(data)
        self.record("snapshot", state=state)
        if "error" in state:
            raise RuntimeError(state["error"])
        if state["pid"] != self.process.pid or state["request"] <= self.sequence:
            raise AssertionError("wrong process or stale inspection response")
        self.sequence = state["request"]
        observed_viewport = state["document"]["viewport"]
        if viewport is not None and observed_viewport != viewport:
            raise AssertionError("targeted snapshot received another document owner")
        viewport = observed_viewport
        if not self.multiple_windows and self.viewport is not None and viewport != self.viewport:
            raise AssertionError("single-window journey received another document owner")
        self.viewport = viewport
        return state

    def wait(self, description, predicate, timeout=6):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.snapshot()
            if predicate(state["document"]):
                return state
            time.sleep(0.05)
        raise AssertionError(f"timed out: {description}")

    def stable_state(self, predicate, frames=8):
        for _ in range(frames):
            if not predicate(self.snapshot()["document"]):
                raise AssertionError("UI state changed again after the requested transition")

    def key(self, code, flags="cmd"):
        self.native("key", code, flags)

    def click(self, name, *, require_hover=False):
        def target_in(state, require_fresh=True):
            target = state["targets"].get(name)
            if not target or not target["enabled"] or (require_fresh and target["age_ms"] > 500):
                raise AssertionError(f"missing, disabled or stale hit target: {name}")
            if name.startswith("panel.") and state["document"]["panel"] is None:
                raise AssertionError("cannot click a hidden panel")
            if name.startswith("find.") and not state["document"]["find_visible"]:
                raise AssertionError("cannot click hidden Find controls")
            return target

        # An idle viewport may not repaint. Its last position is sufficient
        # for a pointer move; only fresh geometry may authorize the click.
        target = target_in(self.snapshot(), require_fresh=False)
        deadline = time.monotonic() + 3
        while True:
            self.native("move", target["x"], target["y"])
            # Wait for scroll/layout to settle before posting a click. No action
            # has been attempted yet; this never retries a failed click.
            observed = target_in(self.snapshot())
            if (abs(observed["x"] - target["x"]) <= 0.5
                    and abs(observed["y"] - target["y"]) <= 0.5
                    and (not require_hover or observed.get("hovered") is True)):
                break
            if time.monotonic() >= deadline:
                raise AssertionError(f"target did not settle before click: {name}")
            target = observed
        # AppKit/WKWebView can suppress passive root hover. A standalone
        # Settings window can require hover acknowledgement before its first
        # click; root/native-child controls keep the geometry-only contract.
        self.record("target", name=name, target=observed)
        self.native("click", observed["x"], observed["y"])

    def right_click(self, name):
        def target_in(state, require_fresh=True):
            target = state["targets"].get(name)
            if not target or not target["enabled"] or (require_fresh and target["age_ms"] > 500):
                raise AssertionError(f"missing, disabled or stale hit target: {name}")
            return target

        # Same settle-then-post contract as click(): only fresh geometry may
        # authorize the event, and it is never retried once posted.
        target = target_in(self.snapshot(), require_fresh=False)
        deadline = time.monotonic() + 3
        while True:
            self.native("move", target["x"], target["y"])
            observed = target_in(self.snapshot())
            if abs(observed["x"] - target["x"]) <= 0.5 and abs(observed["y"] - target["y"]) <= 0.5:
                break
            if time.monotonic() >= deadline:
                raise AssertionError(f"target did not settle before right-click: {name}")
            target = observed
        self.record("target", name=name, target=observed)
        self.native("right-click", observed["x"], observed["y"])

    def right_click_at(self, x, y):
        # Raw coordinates for targets whose hit point is derived, not
        # published (e.g. a caret position inside math text). Settles the
        # pointer first so hover state is deterministic before posting.
        self.native("move", x, y)
        time.sleep(0.2)
        self.record("target", name="<point>", target={"x": x, "y": y})
        self.native("right-click", x, y)

    def double_click_at(self, x, y):
        # Raw-coordinate double-click mirroring right_click_at: used to
        # select the word under a caret point before right-clicking it.
        self.native("move", x, y)
        time.sleep(0.2)
        self.record("target", name="<point>", target={"x": x, "y": y})
        self.native("double-click", x, y)

    def drag(self, source_name, destination_name):
        state = self.snapshot()
        source = state["targets"].get(source_name)
        destination = state["targets"].get(destination_name)
        for name, target in ((source_name, source), (destination_name, destination)):
            if not target or not target["enabled"] or target["age_ms"] > 500:
                raise AssertionError(f"missing, disabled or stale drag target: {name}")
        self.native("move", source["x"], source["y"])
        deadline = time.monotonic() + 3
        while True:
            state = self.snapshot()
            current = state["targets"].get(source_name)
            if current and current["hovered"]:
                break
            if time.monotonic() >= deadline:
                raise AssertionError("drag source did not become current")
            time.sleep(0.05)
        source = state["targets"][source_name]
        self.record("drag", source=source_name, destination=destination_name,
                    source_target=source, destination_target=destination)
        self.native("drag", source["x"], source["y"], destination["x"], destination["y"])

    def double_click(self, name, minimum_height=0):
        def target_in(state, require_fresh=True):
            target = state["targets"].get(name)
            if not target or not target["enabled"] or (require_fresh and target["age_ms"] > 500):
                raise AssertionError(f"missing, disabled or stale double-click target: {name}")
            return target

        target = target_in(self.snapshot(), require_fresh=False)
        self.native("move", target["x"], target["y"])
        deadline = time.monotonic() + 3
        stable_since = None
        stable_frames = 0
        previous_frame = None
        while True:
            observed = target_in(self.snapshot())
            now = time.monotonic()
            same = all(abs(observed[field] - target[field]) <= 0.5
                       for field in ("x", "y", "width", "height"))
            visible = observed["height"] >= minimum_height and observed.get("hovered", False)
            if same and visible:
                if observed["frame"] != previous_frame:
                    stable_frames += 1
                    previous_frame = observed["frame"]
                if stable_since is None:
                    stable_since = now
                if stable_frames >= 2 and now - stable_since >= 0.15:
                    break
            else:
                stable_since = None
                stable_frames = 0
                previous_frame = None
                self.native("move", observed["x"], observed["y"])
                target = observed
            if now >= deadline:
                raise AssertionError(f"target did not settle before double-click: {name}")
            time.sleep(0.05)
        self.record("target", name=name, target=observed)
        self.native("double-click", observed["x"], observed["y"])

    def stable(self, baseline, frames=30):
        previous = None
        for _ in range(frames):
            document = self.snapshot()["document"]
            if previous is not None and document["frame"] <= previous:
                raise AssertionError("stability check did not advance the UI frame")
            previous = document["frame"]
            assert_stable_height(baseline, document)

    def panels(self):
        source = self.snapshot()["document"]["source_fingerprint"]
        self.key(23)  # Cmd+5: bottom panel, initially Problems
        state = self.wait("Problems opens", lambda d: d["panel"] == "Problems")
        baseline = height(state["document"])
        self.stable(baseline)

        def shell_pid(filename):
            destination = self.directory / filename
            self.wait_target("terminal.input")
            self.click("terminal.input")
            self.native("text", f"echo $$ > '{destination}'")
            self.key(36, "none")  # Return executes ordinary terminal input.
            self.wait("terminal command writes its shell PID", lambda d: destination.exists()
                      and destination.stat().st_size > 0 and not d["terminal_off"])
            pid = int(destination.read_text().strip())
            if pid <= 1 or pid in (self.process.pid, os.getpid()):
                raise AssertionError("terminal command returned an invalid shell PID")
            os.kill(pid, 0)
            self.record("terminal.shell", pid=pid, receipt=filename)
            if self.snapshot()["document"]["source_fingerprint"] != source:
                raise AssertionError("terminal typing changed the document")
            return pid

        def shell_gone(pid):
            try:
                os.kill(pid, 0)
                return False
            except ProcessLookupError:
                return True

        def stopped(document):
            return document["terminal_off"] and document["terminal_status"] == "Off" \
                and document["source_fingerprint"] == source

        self.click("panel.terminal")
        self.wait("Terminal selected", lambda d: d["panel"] == "Terminal")
        original_pid = shell_pid("terminal-first.pid")
        self.click("terminal.off")
        self.wait("off reaps the real shell", lambda d: stopped(d) and shell_gone(original_pid))
        self.stable_state(stopped, frames=30)
        off = self.snapshot()["targets"].get("terminal.off")
        if not off or off["enabled"]:
            raise AssertionError("terminal off control stays enabled after stopping")
        if self.capture_review:
            self.capture_viewport("main")

        for _ in range(3):
            self.click("panel.problems")
            self.wait("Problems selected without restarting the shell",
                      lambda d: d["panel"] == "Problems" and stopped(d))
            self.stable(baseline)
            self.click("panel.terminal")
            self.wait("Terminal remains off when reopened",
                      lambda d: d["panel"] == "Terminal" and stopped(d))
            self.key(23)
            self.wait("panel hidden without restarting the shell", lambda d: d["panel"] is None and stopped(d))
            self.key(23)
            self.wait("Terminal restored without restarting the shell",
                      lambda d: d["panel"] == "Terminal" and stopped(d))
            self.stable_state(stopped, frames=30)
            self.stable(baseline)
        self.click("terminal.restart")
        self.wait("Restart enables a new shell", lambda d: not d["terminal_off"])
        restarted_pid = shell_pid("terminal-restarted.pid")
        if restarted_pid == original_pid:
            raise AssertionError("Restart reused the terminated shell")
        self.click("terminal.off")
        self.wait("off reaps the restarted shell", lambda d: stopped(d) and shell_gone(restarted_pid))
        self.click("panel.problems")
        self.wait("Problems selected", lambda d: d["panel"] == "Problems" and stopped(d))
        self.key(23)
        self.wait("panel hidden", lambda d: d["panel"] is None and stopped(d))

        # Multiple shells: the add button opens further terminals, the
        # numbered switcher moves between them, and each pane owns its shell.
        # Off closes the shown pane; closing the last one clears the window.
        # The earlier flow leaves the terminal empty, so counting starts at 0.
        self.key(23)
        self.wait("panel restored", lambda d: d["panel"] is not None)
        self.click("panel.terminal")
        self.wait("Terminal restored for multi-shell checks",
                  lambda d: d["panel"] == "Terminal" and stopped(d))
        state = self.snapshot()["document"]
        count = state["terminal_count"]
        self.wait_target("terminal.new")
        self.click("terminal.new")
        self.wait("add button opens a shell",
                  lambda d: d["terminal_count"] == count + 1 and d["terminal_active"] == count,
                  timeout=15)
        second_pid = shell_pid("terminal-second.pid")
        self.click("terminal.new")
        self.wait("a third shell opens beside the second",
                  lambda d: d["terminal_count"] == count + 2 and d["terminal_active"] == count + 1,
                  timeout=15)
        third_pid = shell_pid("terminal-third.pid")
        if third_pid == second_pid:
            raise AssertionError("add button reused the running shell")
        self.click(f"terminal.{count}")
        self.wait("switcher returns to the first of the new terminals",
                  lambda d: d["terminal_active"] == count, timeout=15)
        self.click("terminal.input")
        first_pid = shell_pid("terminal-first-again.pid")
        if first_pid != second_pid:
            raise AssertionError("switching back did not return to the first shell")
        self.record("terminal.panes", first_pid=first_pid, third_pid=third_pid)

        # TUI mouse reporting: a real click inside vim must move its cursor,
        # which vim reports back through a position file on disk.
        target = self.directory / "tui-mouse.txt"
        target.write_text("".join(f"line {index:02}\n" for index in range(1, 21)))
        proof = self.directory / "tui-click.txt"
        if proof.exists():
            proof.unlink()
        self.click("terminal.input")
        self.native("text", f"vim -u NONE -N -n -i NONE -c 'set mouse=a' '{target}'")
        self.key(36, "none")
        time.sleep(1.5)
        self.click("terminal.input")
        time.sleep(0.3)
        self.native("text", f":call writefile([string(getcurpos())], '{proof}')")
        self.key(36, "none")
        self.native("text", ":q!")
        self.key(36, "none")
        deadline = time.monotonic() + 15
        while not proof.exists():
            if time.monotonic() >= deadline:
                raise AssertionError("vim never recorded the click position")
            time.sleep(0.1)
        match = re.search(r"\[\d+, (\d+),", proof.read_text())
        if not match or int(match.group(1)) <= 1:
            raise AssertionError(f"terminal click did not reach vim: {proof.read_text()!r}")
        self.record("terminal.mouse", line=int(match.group(1)))
        self.click(f"terminal.{count + 1}")
        self.wait("switcher returns to the third terminal",
                  lambda d: d["terminal_active"] == count + 1, timeout=15)
        # Focus lands a frame after the switch; clicking the surface first
        # matches the shell_pid pattern and avoids the race.
        self.click("terminal.input")
        self.native("text", "exit")
        self.key(36, "none")
        self.wait("exiting one shell leaves the other panes alone",
                  lambda d: "exited" in (d["terminal_status"] or "")
                  and d["terminal_count"] == count + 2, timeout=15)
        self.click("terminal.off")
        self.wait("off closes the exited pane",
                  lambda d: d["terminal_count"] == count + 1 and d["terminal_active"] == count,
                  timeout=15)
        self.click("terminal.off")
        self.wait("off closes the second pane",
                  lambda d: d["terminal_count"] == count, timeout=15)
        self.key(23)
        self.wait("panel hidden with all terminals closed", lambda d: d["panel"] is None)

    def terminal_find(self):
        source = self.snapshot()["document"]["source_fingerprint"]
        if self.snapshot()["document"]["panel"] is None:
            self.key(23)  # Cmd+5
            self.wait("bottom panel opens", lambda d: d["panel"] is not None)
        self.click("panel.terminal")
        self.wait("Terminal selected for scrollback search", lambda d: d["panel"] == "Terminal")
        initial_count = self.snapshot()["document"]["terminal_count"]

        def unchanged(document):
            return document["source_fingerprint"] == source and not document["find_visible"]

        def wait_find(description, query, matches, selected=None, focused=None, visible=False):
            def expected(document, ready=True):
                find = document["terminal_find"]
                return unchanged(document) and find["open"] and find["query"] == query \
                    and find["matches"] == matches and find["selected"] == selected \
                    and (not ready or not find["pending"]) \
                    and (focused is None or find["focused"] == focused) \
                    and (not visible or find["selected_visible"])
            self.wait(description, expected, timeout=15)
            # Reflow/output can schedule another bounded worker refresh. The
            # visible result and focus must remain stable while it is pending.
            self.stable_state(lambda document: expected(document, ready=False))

        def print_fixture(filename, lines):
            # Run a private script: echoing a command containing the query would
            # add a spurious match before the actual output under test.
            script = self.directory / filename
            receipt = script.with_suffix(".ready")
            script.write_text("printf '%s\\n' " + " ".join("'" + line + "'" for line in lines)
                              + "\n" + f"printf ready > '{receipt}'\n")
            self.wait_target("terminal.input")
            self.click("terminal.input")
            self.native("text", f"/bin/zsh '{script}'")
            self.key(36, "none")
            self.wait("isolated shell finishes its search fixture", lambda d: receipt.exists()
                      and receipt.read_text() == "ready" and unchanged(d), timeout=15)

        self.wait_target("terminal.new")
        self.click("terminal.new")
        first = initial_count
        self.wait("first search pane opens", lambda d: d["terminal_count"] == initial_count + 1
                  and d["terminal_active"] == first, timeout=15)
        filler = [f"scrollback filler {index:03}" for index in range(160)]
        print_fixture("search-output.zsh", ["NATIVE-FIND-MARKER first", *filler,
                      "Native-Find-Marker middle", *filler, "native-find-marker last"])
        self.key(3)  # Cmd+F belongs to the focused terminal, not document Find.
        self.wait("terminal Find opens with its field focused", lambda d: unchanged(d)
                  and d["terminal_find"]["open"] and d["terminal_find"]["focused"])
        query = "native-find-marker"
        self.native("text", query)
        wait_find("case-insensitive worker search includes all scrollback", query, 3, 0, True)
        find = self.snapshot()["document"]["terminal_find"]
        if find["selected_line"] >= find["scroll_offset"]:
            raise AssertionError("first terminal match did not begin outside the visible scrollback")
        if self.capture_review:
            self.capture_viewport("main")

        # Each key must step exactly once, including wraparound. Enter had a
        # double-consumption risk when handled before and after drawing the bar.
        for code, flags, selected in ((5, "cmd", 1), (5, "cmd", 2), (5, "cmd", 0),
                                      (5, "cmd+shift", 2), (36, "none", 0),
                                      (36, "shift", 2)):
            self.key(code, flags)
            wait_find("terminal match steps once and becomes visible", query, 3, selected,
                      True, visible=True)
        self.click("terminal.find.next")
        wait_find("next button wraps to the first match", query, 3, 0, visible=True)
        self.click("terminal.find.previous")
        wait_find("previous button wraps to the last match", query, 3, 2, True, visible=True)
        self.key(5)
        wait_find("match button retains terminal shortcut ownership", query, 3, 0, True, visible=True)
        self.key(5, "cmd+shift")
        wait_find("backward shortcut still belongs to terminal search", query, 3, 2, True, visible=True)
        if self.capture_review:
            self.capture_viewport("main")

        self.click("terminal.find.query")
        self.wait("terminal query owns typing", lambda d: d["terminal_find"]["focused"])
        self.key(0)  # Cmd+A selects only the query.
        self.native("text", "absent-search-token")
        wait_find("replacing the query leaves the source alone", "absent-search-token", 0,
                  focused=True)
        self.key(0)
        self.native("text", query)
        wait_find("query replacement restores the real matches", query, 3, 0, True)

        self.click("terminal.input")
        self.wait("grid takes focus away from search", lambda d: unchanged(d)
                  and d["terminal_find"]["open"] and not d["terminal_find"]["focused"])
        self.key(53, "none")  # Escape belongs to the idle shell in this state.
        self.stable_state(lambda d: unchanged(d) and d["terminal_find"]["open"]
                          and not d["terminal_find"]["focused"])
        self.key(3)
        wait_find("grid Cmd+F refocuses the existing terminal query", query, 3, 0, True)
        self.key(3)
        self.wait("focused Cmd+F closes terminal Find", lambda d: unchanged(d)
                  and not d["terminal_find"]["open"])
        self.stable_state(lambda d: unchanged(d) and not d["terminal_find"]["open"])
        self.click("terminal.input")
        self.key(3)
        wait_find("terminal query survives closing and reopening", query, 3, 0, True)
        self.key(53, "none")
        self.wait("focused Escape closes terminal Find", lambda d: unchanged(d)
                  and not d["terminal_find"]["open"])
        self.stable_state(lambda d: unchanged(d) and not d["terminal_find"]["open"])
        self.click("terminal.input")
        self.key(3)
        wait_find("terminal search is ready before switching panes", query, 3, 0, True)

        self.click("terminal.new")
        second = initial_count + 1
        self.wait("second pane has independent empty search", lambda d: unchanged(d)
                  and d["terminal_count"] == initial_count + 2 and d["terminal_active"] == second
                  and not d["terminal_find"]["open"] and d["terminal_find"]["query"] == "",
                  timeout=15)
        print_fixture("second-output.zsh", ["independent search result"])
        self.key(3)
        self.wait("second query receives keyboard focus", lambda d: d["terminal_find"]["focused"])
        self.native("text", "independent")
        wait_find("second pane searches its own shell", "independent", 1, 0, True)
        self.click(f"terminal.{first}")
        wait_find("first pane restores its own query and matches", query, 3, 0)
        self.click(f"terminal.{second}")
        wait_find("second pane restores its own query and matches", "independent", 1, 0)
        self.click("terminal.find.close")
        self.wait("one close-button click hides second search", lambda d: unchanged(d)
                  and not d["terminal_find"]["open"])
        self.stable_state(lambda d: unchanged(d) and not d["terminal_find"]["open"])
        self.click("terminal.off")
        self.wait("closing second search pane returns to the first", lambda d: unchanged(d)
                  and d["terminal_count"] == initial_count + 1 and d["terminal_active"] == first)
        wait_find("first search survives closing the other shell", query, 3, 0)
        self.click("terminal.off")
        self.wait("search panes close without changing the document", lambda d: unchanged(d)
                  and d["terminal_count"] == initial_count)
        self.key(23)
        self.wait("terminal search journey leaves the bottom panel hidden", lambda d: unchanged(d)
                  and d["panel"] is None)

    def terminal_load(self):
        """Real editor typing while an isolated process redraws the terminal."""
        import shlex

        text = "Terminal load fixture.\n\n"
        tab_count = self.scratch(text)
        if self.snapshot()["document"]["panel"] is None:
            self.key(23)
            self.wait("bottom panel opens for terminal load", lambda d: d["panel"] is not None)
        self.click("panel.terminal")
        self.wait("Terminal selected for redraw workload", lambda d: d["panel"] == "Terminal")
        initial_count = self.snapshot()["document"]["terminal_count"]
        producer = self.directory / "terminal-load.py"
        producer.write_text('''import json, os, shutil, sys, time
from pathlib import Path
rate = int(sys.argv[1])
start, stop = map(Path, sys.argv[2:4])
start.write_text(json.dumps({"pid": os.getpid(), "rate": rate}))
started = time.monotonic()
frames = 0
sys.stdout.write("\\x1b[?1049h\\x1b[?25l")
while time.monotonic() - started < 8:
    cols, rows = shutil.get_terminal_size((100, 20))
    cols, rows = min(cols, 240), min(rows, 80)
    output = []
    for row in range(rows):
        message = f"task {row:02} frame {frames:05} Thinking about the next response; checking sources. "
        line = (message * (cols // len(message) + 1))[:cols]
        output.append(f"\\x1b[{row + 1};1H\\x1b[38;5;{33 + (row + frames) % 6}m\\x1b[48;5;{234 + row % 3}m" + line)
    sys.stdout.write("".join(output))
    sys.stdout.flush()
    frames += 1
    time.sleep(1 / rate)
sys.stdout.write("\\x1b[0m\\x1b[?25h\\x1b[?1049l")
sys.stdout.flush()
stop.write_text(json.dumps({"pid": os.getpid(), "rate": rate, "frames": frames, "seconds": time.monotonic() - started}))
''')
        shutil.copy2(producer, self.evidence / producer.name)
        for rate in (30, 60):
            self.click("terminal.new")
            self.wait("fresh redraw pane opens", lambda d: d["terminal_count"] == initial_count + 1,
                      timeout=15)
            start = self.directory / f"terminal-load-{rate}.start.json"
            stop = self.directory / f"terminal-load-{rate}.stop.json"
            command = "exec " + " ".join(map(shlex.quote, (sys.executable, "-u", str(producer), str(rate), str(start), str(stop))))
            self.click("terminal.input")
            self.native("text", command)
            self.key(36, "none")
            self.wait("bounded full-screen redraw producer starts", lambda d: start.exists()
                      and start.stat().st_size > 0 and d["terminal_status"] is None,
                      timeout=15)
            started = json.loads(start.read_text())
            if started["pid"] <= 1 or started["pid"] in (self.process.pid, os.getpid()):
                raise AssertionError("invalid terminal redraw producer PID")
            os.kill(started["pid"], 0)
            self.record("terminal.load.start", **started)
            self.click("editor.source")
            self.key(125)  # Cmd+Down places the caret at the end of the scratch.
            before = self.wait("editor owns keyboard during terminal output",
                               lambda d: d["editor_keyboard_focused"] and not d["terminal_find"]["focused"])["document"]
            revision = before["terminal_content_revision"]
            if revision is None:
                raise AssertionError("terminal redraw pane has no content revision")
            for index, character in enumerate("Smooth typing under load"):
                # One native text call per character: the adapter otherwise
                # batches up to 16 UTF-16 units into each posted key event.
                self.native("text", character)
                text += character
                if index % 8 == 7:
                    self.wait("typed source and caret remain correct under terminal load",
                              lambda d: d["source_fingerprint"] == fingerprint(text)
                              and d["cursor"] == [len(text), len(text)]
                              and d["editor_keyboard_focused"])
            state = self.wait("worker output and editor typing both advance",
                              lambda d: d["source_fingerprint"] == fingerprint(text)
                              and d["cursor"] == [len(text), len(text)]
                              and d["editor_keyboard_focused"]
                              and d["terminal_content_revision"] > revision)["document"]
            if stop.exists():
                raise AssertionError("redraw producer finished before the typing workload")
            self.record("terminal.load.typing", rate=rate,
                        source_bytes=state["source_bytes"], cursor=state["cursor"],
                        worker_revision_before=revision,
                        worker_revision_after=state["terminal_content_revision"])
            self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(text)
                              and d["cursor"] == [len(text), len(text)]
                              and d["editor_keyboard_focused"])
            self.wait("bounded redraw producer writes its stop receipt", lambda d: stop.exists()
                      and stop.stat().st_size > 0, timeout=15)
            stopped = json.loads(stop.read_text())
            if stopped["pid"] != started["pid"] or stopped["frames"] < 2:
                raise AssertionError("terminal redraw did not complete the recorded workload")
            self.record("terminal.load.stop", **stopped)
            def producer_gone():
                try:
                    os.kill(started["pid"], 0)
                    return False
                except ProcessLookupError:
                    return True

            self.click("terminal.off")
            self.wait("redraw pane closes and reaps its process without altering source",
                      lambda d: d["terminal_count"] == initial_count and producer_gone()
                      and d["source_fingerprint"] == fingerprint(text))
        self.key(23)
        self.wait("terminal load leaves bottom panel hidden", lambda d: d["panel"] is None)
        self.close_scratch(tab_count)

    def find(self):
        for _ in range(3):
            self.key(3)  # Cmd+F
            self.wait("Find visible", lambda d: d["find_visible"] and d["find_focused"])
            self.key(3)
            self.wait("focused Cmd+F closes Find", lambda d: not d["find_visible"])
            self.stable_state(lambda d: not d["find_visible"])
        self.key(3)
        self.wait("Find reopens", lambda d: d["find_visible"] and d["find_focused"])
        owner = next(window["AXTitle"] for window in self.native("windows")["windows"] if window["AXTitle"].endswith(" — tiptoptyp"))
        before = self.snapshot()["document"]["native_size"]
        requested = [before[0] - 60, before[1] - 40]
        self.native("resize-focused", *requested)
        self.wait("accessibility resize targets the document behind Find", lambda d: d["native_size"] == requested and d["find_visible"])
        second = [requested[0] - 20, requested[1] - 20]
        self.native("resize-focused", *second)
        self.wait("a second resize still targets the owner", lambda d: d["native_size"] == second and d["find_focused"])
        self.native("minimize-focused")
        self.native_wait("minimizing focused Find minimizes its owner", lambda ws: any(w["AXTitle"] == owner and w.get("AXMinimized") for w in ws))
        self.native("restore", owner)
        self.native("raise", owner)
        self.native_wait("owner restores", lambda ws: any(w["AXTitle"] == owner and w.get("AXFocused") and not w.get("AXMinimized") for w in ws))
        if not self.snapshot()["document"]["find_visible"]:
            self.key(3)
        self.wait("Find survives or reopens after minimize", lambda d: d["find_visible"])
        for expected in (True, False, True, False):
            self.click("find.replace")
            self.wait("Replace toggles", lambda d: d["find_visible"] and d["replace_visible"] == expected)
            sized = self.wait("Find popup settles at its content height", lambda d: d["find_native_height"] is not None and d["find_native_height"] == d["find_measured_height"])["document"]
            self.stable_state(lambda d: d["find_visible"] and d["replace_visible"] == expected and d["find_native_height"] == sized["find_native_height"], frames=30)
        self.click("find.close")
        self.wait("one click closes Find", lambda d: not d["find_visible"])
        self.stable_state(lambda d: not d["find_visible"])

    def empty(self):
        # The aggregate run may have opened dialog fixtures alongside the
        # launch document. Close every tab explicitly so this journey tests
        # the final-tab contract rather than depending on journey ordering.
        while self.snapshot()["document"]["tabs"]:
            before = self.snapshot()["document"]["tabs"]
            self.key(13)  # Cmd+W closes the active tab.
            self.wait("tab closes or asks for confirmation",
                      lambda d: d["tabs"] < before or d["modal_open"])
            if self.snapshot()["document"]["modal_open"]:
                self.wait_target("modal.discard")
                self.click("modal.discard")
                self.wait("dirty tab discarded", lambda d: d["tabs"] < before and not d["modal_open"])
        self.wait("window remains after final tab closes", lambda d: d["tabs"] == 0)
        self.key(45)  # Cmd+N
        self.wait("New creates an empty document", lambda d: d["tabs"] == 1 and d["source_bytes"] == 0 and d["path"] is None)

    def settings(self):
        self.key(43)
        return self.native_wait("Settings focused", lambda ws: any("Settings" in w["AXTitle"] and w.get("AXFocused") for w in ws))

    def close_settings(self):
        windows = self.native("windows")["windows"]
        self.native("close", next(w["AXTitle"] for w in windows if "Settings" in w["AXTitle"]))
        self.wait("Settings closed and document focused", lambda d: not d["settings_visible"] and d["focused"])

    def setting_section(self, query, title):
        # JSON edits preserve that mode when Settings closes. A form-section
        # search must explicitly return to the form before using its field.
        if self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
            self.wait("Settings form ready", lambda d: not d["settings_json"]["visible"])
        self.click("settings.search")
        self.wait_target("settings.search", focused=True)
        self.key(0)
        self.native("text", query)
        self.wait_target("settings.result." + title)
        self.click("settings.result." + title)

    def wait_target(self, name, focused=False):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            target = self.snapshot()["targets"].get(name)
            if target and target["enabled"] and target["age_ms"] <= 500 \
                    and (not focused or target.get("focused", False)):
                return
            time.sleep(0.05)
        readiness = "visible and focused" if focused else "visible"
        raise AssertionError(f"target did not become {readiness}: {name}")

    def preview(self):
        fixture = self.directory / "journey.typ"
        # Earlier dialog/renderer journeys may leave a TeX or arbitrary
        # extension tab active. The preview contract is about the fixture's
        # designated tab, so select it explicitly before exercising backend
        # and palette transitions.
        state = self.snapshot()["document"]
        if state["path"] != str(fixture):
            preview_tab = state.get("preview_tab_id")
            if preview_tab is None:
                raise AssertionError("preview fixture has no designated tab")
            self.click(f"tab.{preview_tab}")
        self.wait("Typst fixture opened", lambda d: d["path"] == str(fixture), timeout=20)
        self.key(35, "cmd+alt")  # Pin this tab for preview.
        self.key(15)  # Compile.
        for backend in ("Pdfium", "Interactive"):
            self.settings()
            self.setting_section("backend", "Typst preview backend")
            self.wait_target("settings.backend." + backend)
            self.click("settings.backend." + backend)
            self.close_settings()
            self.key(15)
            self.wait("requested renderer actually ready", lambda d: d["preview_path"] == str(fixture)
                      and d["preview_requested_backend"] == backend
                      and d["preview_backend"] == ("PDFium" if backend == "Pdfium" else "Tinymist")
                      and d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"], timeout=60)
            for dark, comfy in ((False, False), (True, False), (True, True), (False, True), (False, False)):
                self.settings()
                self.native("resize-focused", 800, 900)
                self.setting_section("appearance", "Appearance")
                self.wait_target("settings.theme." + ("Dark" if dark else "Light"))
                self.click("settings.theme." + ("Dark" if dark else "Light"))
                self.wait("interface theme applied", lambda d: d["dark"] == dark)
                if self.snapshot()["document"]["comfy"] != comfy:
                    self.wait_target("settings.comfy")
                    self.click("settings.comfy")
                    self.wait_target("settings.comfy.text")
                    self.click("settings.comfy.text")
                self.close_settings()
                field = "pdfium_palette" if backend == "Pdfium" else "webview_palette"
                self.wait("renderer palette follows theme/comfy", lambda d: d["dark"] == dark and d["comfy"] == comfy
                          and d["page_dark"] == dark and d[field] == d["expected_palette"])
                if backend == "Interactive":
                    deadline = time.monotonic() + 10
                    while True:
                        snapshot = self.snapshot()
                        try:
                            assert_tinymist_palette(snapshot.get("renderer"), snapshot["document"]["expected_palette"])
                            break
                        except AssertionError:
                            if time.monotonic() >= deadline:
                                raise
                            time.sleep(0.05)
                self.record("preview.transition", backend=backend, dark=dark, comfy=comfy)
                if dark and comfy:
                    self.review_native_preview(backend)

    def native_wait(self, description, predicate, timeout=6):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            windows = self.native("windows")["windows"]
            if predicate(windows):
                return windows
            time.sleep(0.05)
        raise AssertionError(f"timed out: {description}")

    def focus(self):
        self.multiple_windows = True
        original = self.native_wait("original window focused", lambda ws: any(w.get("AXFocused") for w in ws))
        original = next(w["AXTitle"] for w in original if w.get("AXFocused"))
        self.key(45, "cmd+shift")
        windows = self.native_wait("second document focused", lambda ws: any(w.get("AXFocused") and w["AXTitle"] != original for w in ws))
        second = next(w["AXTitle"] for w in windows if w.get("AXFocused"))
        for title in (original, second, original):
            self.native("raise", title)
            self.native_wait("document raised", lambda ws: any(w["AXTitle"] == title and w.get("AXFocused") for w in ws))
            self.key(43)  # Cmd+, opens Settings from the most recently used owner.
            windows = self.native_wait("Settings focused", lambda ws: any("Settings" in w["AXTitle"] and w.get("AXFocused") for w in ws))
            settings = next(w["AXTitle"] for w in windows if "Settings" in w["AXTitle"] and w.get("AXFocused"))
            self.native("close", settings)
            self.native_wait("Settings returns focus to its last owner", lambda ws: not any("Settings" in w["AXTitle"] for w in ws) and any(w["AXTitle"] == title and w.get("AXFocused") for w in ws))
        self.key(46)  # Cmd+M
        self.native_wait("document minimized", lambda ws: any(w["AXTitle"] == original and w.get("AXMinimized") for w in ws))
        self.native("restore", original)
        self.native("raise", original)
        self.native_wait("document restored", lambda ws: any(w["AXTitle"] == original and w.get("AXFocused") and not w.get("AXMinimized") for w in ws))
        finder = self.native("finder")["pid"]
        deadline = time.monotonic() + 6
        while self.native("foreground")["pid"] != finder:
            if time.monotonic() >= deadline:
                raise AssertionError("Finder did not become foreground")
            time.sleep(0.05)
        self.native("activate")
        self.native("raise", original)
        self.native_wait("focus after app switch", lambda ws: any(w["AXTitle"] == original and w.get("AXFocused") for w in ws))
        self.native("close", second)
        self.native_wait("second document closes once", lambda ws: len(ws) == 1 and ws[0]["AXTitle"] == original)
        self.native("raise", original)
        self.multiple_windows = False
        self.viewport = None
        self.wait("restored editor keeps rendering after children close", lambda d: d["focused"] and d["minimized"] is not True)
        self.stable_state(lambda d: d["minimized"] is not True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--journey", choices=JOURNEY_NAMES, default="all")
    parser.add_argument("--liquid-model-dir", type=Path, help="Liquid journeys only: read-only prepared model reference; downloads optional assets into a temporary cache")
    parser.add_argument("--ltex-executable", type=Path, help="LTeX journey only: path to an installed ltex-ls-plus launcher")
    parser.add_argument("--release", action="store_true", help="use the optimized production profile for large-document journeys")
    parser.add_argument("--executable", type=Path, help="reuse a preserved build for paired benchmarks; record its version and binary hash")
    parser.add_argument("--review-preview", action="store_true", help="pause at affected native preview states for independent on-screen review; not an automated visual pass")
    parser.add_argument("--capture-review", action="store_true", help="capture affected settings/controls framebuffers for separate visual inspection")
    parser.add_argument("--trace-preview", action="store_true", help="retain native preview geometry traces")
    parser.add_argument("--effects-preview-snapshots", choices=("on", "off"), default="on",
                        help="effects_pacing only: enable/disable idle Tinymist zoom snapshots (default: on)")
    parser.add_argument("--review-failure", action="store_true", help="retain a failed fixture for up to three minutes of independent inspection")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--observe-only", action="store_true", help="check real-app inspection without input; does not pass the interaction suite")
    modes.add_argument("--prepare-only", action="store_true", help="build runner and editor without launching")
    args = parser.parse_args()
    if args.journey in LTEX_JOURNEYS:
        try:
            args.ltex_executable = ltex_program(args.ltex_executable)
        except ValueError as error:
            parser.error(str(error))
    elif args.ltex_executable is not None:
        parser.error('--ltex-executable requires --journey ltex')
    if args.journey in LIQUID_JOURNEYS:
        try:
            args.liquid_model_dir = liquid_model_directory(args.liquid_model_dir)
        except ValueError as error:
            parser.error(str(error))
    elif args.liquid_model_dir is not None:
        parser.error('--liquid-model-dir requires a Liquid journey (liquid_selection or liquid_streaming)')
    if args.journey != "effects_pacing" and args.effects_preview_snapshots != "on":
        parser.error("--effects-preview-snapshots off requires --journey effects_pacing")
    if args.journey in TEXDOC_JOURNEYS:
        try:
            texdoc_program()
        except ValueError as error:
            parser.error(str(error))
    if sys.platform != "darwin":
        parser.error("requires macOS and a logged-in desktop; this is not a headless test")
    evidence_root = ROOT / ".tiptoptyp/desktop-ui-tests"
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="run-", dir=evidence_root))
    driver = evidence_root / "desktop-ui-driver"
    result = {"passed": False, "journeys": [], "platform": platform.platform(),
              "evidence": "native input and read-only state; no visual composition verification"}
    if args.journey == "effects_pacing":
        result["effects_preview_snapshots_requested"] = args.effects_preview_snapshots == "on"
    process = None
    journey = None
    try:
        subprocess.run(["swiftc", str(ROOT / "scripts/desktop-ui-driver.swift"), "-o", str(driver)], check=True, timeout=90)
        if not args.prepare_only and not args.observe_only:
            import pwd
            shell = os.environ.get("SHELL") or pwd.getpwuid(os.getuid()).pw_shell
            if args.journey in ("all", "panels", "terminal_find", "terminal_load", "terminal_theme", "problems_focus", "command_palette") and Path(shell).resolve() != Path("/bin/zsh"):
                raise RuntimeError("terminal journey currently requires /bin/zsh so startup files can be isolated with ZDOTDIR")
            preflight = subprocess.run([str(driver), "preflight"], capture_output=True, text=True, timeout=6)
            result["preflight"] = preflight.stdout.strip()
            if preflight.returncode:
                state = json.loads(preflight.stdout)
                if state.get("locked") or not state.get("desktop"):
                    raise RuntimeError("desktop is locked or unavailable; unlock it before native interaction checks. " + preflight.stdout)
                raise RuntimeError("desktop driver lacks Accessibility/input permission; enable it for the launching host in macOS Privacy & Security. " + preflight.stdout)
        result["build_profile"] = "release" if args.release else "debug"
        if args.executable:
            executable = str(args.executable.resolve(strict=True))
            if not os.access(executable, os.X_OK):
                raise RuntimeError("preserved benchmark executable is not runnable")
            result["reused_binary"] = executable
            result["build_profile_note"] = "Declared profile of preserved binary; verify against its original build receipt. Current source does not establish this binary's provenance."
        else:
            build_command = ["cargo", "build", "--locked", "--features", "desktop-ui-tests", "--bin", "tiptoptyp", "--message-format=json"]
            if args.release:
                build_command.append("--release")
            build = subprocess.run(build_command,
                                   cwd=ROOT, stdout=subprocess.PIPE, text=True, timeout=900)
            artifacts = [json.loads(line) for line in build.stdout.splitlines()]
            if build.returncode:
                for item in artifacts:
                    if item.get("reason") == "compiler-message" and item.get("message", {}).get("rendered"):
                        print(item["message"]["rendered"], file=sys.stderr, end="")
                raise RuntimeError("editor build failed; compiler diagnostics are printed above")
            executable = next(item["executable"] for item in reversed(artifacts)
                              if item.get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == "tiptoptyp")
        result["version"] = subprocess.check_output([executable, "--version"], text=True).strip()
        result["binary_sha256"] = hashlib.sha256(Path(executable).read_bytes()).hexdigest()
        result["git_head"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        result["git_diff"] = subprocess.check_output(["git", "diff", "--stat"], cwd=ROOT, text=True)
        if args.prepare_only:
            result["prepared_only"] = True
            return
        # Short socket path (Darwin limit), private permissions, no real documents/settings.
        with tempfile.TemporaryDirectory(prefix="ttt-ui-", dir="/tmp") as temporary:
            directory = Path(temporary).resolve()
            app = directory / "Journey.app/Contents"
            (app / "MacOS").mkdir(parents=True)
            binary = app / "MacOS/tiptoptyp"
            shutil.copy2(executable, binary)
            (app / "Info.plist").write_bytes(plistlib.dumps({
                "CFBundleExecutable": "tiptoptyp", "CFBundleIdentifier": "dev.tiptoptyp.journey." + directory.name,
                "CFBundleName": "tiptoptyp UI Journey", "CFBundlePackageType": "APPL", "NSHighResolutionCapable": True,
            }))
            fixture = directory / ("journey.tex" if args.journey == "tex" or args.journey in (*TEX_JOURNEYS, *LTEX_JOURNEYS) else "journey.typ" if args.journey in ("all", *LIQUID_JOURNEYS, *TEXDOC_JOURNEYS, *CITATION_JOURNEYS, *INACTIVE_PREVIEW_JOURNEYS, *IMAGE_JOURNEYS, *NEWEST_JOURNEYS, "partial_preview", "partial_zoom_stress", "partial_cache_edges", "partial_dividers", "preview", "pdf_output", "preview_keyboard", "pdfium_resize", "controls", "file_versions", "tabs", "tab_drag", "dialogs", "startup_preview", "diagnostics", "completion", "layout", "pdf_layout", "explorer_toggle", "explorer_scroll", "background", "background_fades") else "journey.txt")
            fixture.write_text(
                TEX_SOURCE
                if fixture.suffix == ".tex"
                else PREVIEW_SOURCE
                if fixture.suffix == ".typ"
                else "Desktop journey fixture.\n"
            )
            if args.journey in CITATION_JOURNEYS:
                fixture.write_text(CITATION_SOURCE, encoding="utf-8")
                (directory / "references.bib").write_text(CITATION_EXISTING, encoding="utf-8")
                (directory / "citation-metadata.bib").write_text(CITATION_METADATA, encoding="utf-8")
            if args.journey == "effects_pacing":
                fixture = directory / "effects-pacing.txt"
                fixture.write_text(effects_pacing_fixture())
                (directory / "effects-pacing.typ").write_text(effects_pacing_fixture(typst=True))
            if args.journey == "explorer_toggle":
                # Large wrapped source, with the same small three-page preview.
                with fixture.open("a") as stream:
                    stream.write("".join(f"// Long source row {index:04}: " + "layout transition coverage " * 6 + "\n" for index in range(1800)))
            if args.journey in ("all", "explorer_scroll"):
                for index in range(80):
                    (directory / f"file-{index:03}.typ").write_text(f"= File {index:03}\n")
            if args.journey == "explorer_scroll":
                fixture.write_text("\n".join(f"= Section {index:03}" for index in range(80)) + "\n")
            if args.journey in ("all", "git_hunks"):
                subprocess.run(["git", "init", "-q", str(directory)], check=True)
                original = "".join(f"Line {index:03} original text.\n" for index in range(180))
                hunk_fixture = directory / "hunks.typ"
                hunk_fixture.write_text(original)
                subprocess.run(["git", "-C", str(directory), "add", "hunks.typ"], check=True)
                subprocess.run(["git", "-C", str(directory), "-c", "user.name=UI Fixture",
                                "-c", "user.email=fixture@example.invalid", "commit", "-qm", "Fixture"], check=True)
                hunk_fixture.write_text(original.replace("005 original", "005 changed")
                    .replace("085 original", "085 changed").replace("165 original", "165 changed"))
                if args.journey == "git_hunks":
                    fixture = hunk_fixture
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("TIPTOPTYP_UI_", "TIPTOPTYP_PROFILE", "TIPTOPTYP_DESKTOP_TEST"))}
            environment["TIPTOPTYP_DESKTOP_TEST_DIR"] = str(directory)
            if args.journey in LTEX_JOURNEYS:
                environment = ltex_environment(environment, directory, args.ltex_executable)
            if args.journey in CITATION_JOURNEYS:
                environment["TIPTOPTYP_UI_TEST_CITATION_METADATA"] = str(directory / "citation-metadata.bib")
            if args.journey == "effects_pacing":
                environment["TIPTOPTYP_UI_TEST_FX_TIMING"] = "1"
            if args.trace_preview:
                environment["TIPTOPTYP_UI_TRACE"] = "1"
            # macOS default-shell startup and history belong to this fixture.
            environment["ZDOTDIR"] = str(directory)
            environment["HISTFILE"] = str(directory / "shell-history")
            # /etc/zshrc can replace HISTFILE. Disable exit-time writes after
            # system startup so a reaped shell cannot race fixture removal.
            (directory / ".zshrc").write_text("unset HISTFILE\n")
            (directory / "empty-shell-rc").write_text("")
            environment["ENV"] = str(directory / "empty-shell-rc")
            environment["BASH_ENV"] = str(directory / "empty-shell-rc")
            with (evidence / "app.log").open("w") as log:
                launch_arguments = [] if args.journey == "startup_preview" else [str(fixture)]
                process = subprocess.Popen([str(binary), *launch_arguments], cwd=directory, env=environment,
                                           stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    deadline = time.monotonic() + 15
                    while not (directory / "inspect.sock").exists():
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("editor did not start; see app.log")
                        time.sleep(0.05)
                    journey = Journey(process, driver, directory, evidence)
                    journey.review_preview = args.review_preview
                    journey.capture_review = args.capture_review
                    journey.liquid_model_dir = args.liquid_model_dir
                    journey.ltex_executable = args.ltex_executable
                    journey.effects_preview_snapshots = args.effects_preview_snapshots == "on"
                    journey.explorer_scroll_checks_contents = args.journey == "explorer_scroll"
                    journey.command_palette_check_empty = args.journey == "command_palette"
                    # The socket binds before the native UI is initialized. Initial startup
                    # readiness is distinct from retrying a failed journey.
                    # Handshake is bounded and only tolerates startup-not-ready replies.
                    deadline = time.monotonic() + 15
                    while True:
                        try:
                            journey.snapshot()
                            break
                        except RuntimeError as error:
                            if str(error) not in ("UI not initialized", "UI did not publish a fresh frame") or time.monotonic() >= deadline:
                                raise
                            time.sleep(0.05)
                    if not args.observe_only:
                        document_title = None
                        registered = journey.native_wait("native document window registered", lambda windows: any(
                            ((args.journey == "startup_preview" and
                              (window.get("AXTitle", "").endswith(" — tiptoptyp") or
                               window.get("AXTitle") == "tiptoptyp UI Journey"))
                             or window.get("AXTitle") == fixture.name + " — tiptoptyp")
                            and window.get("AXMinimized") is not True for window in windows), timeout=15)
                        if args.journey == "startup_preview":
                            document_title = next((window["AXTitle"] for window in registered
                                                   if window.get("AXTitle", "").endswith(" — tiptoptyp")
                                                   and window.get("AXMinimized") is not True),
                                                  None)
                        if args.journey != "startup_preview":
                            journey.native("activate")
                        # A renderer child can leave the process key without
                        # changing the document's AX focus flag. Raise the
                        # actual owner once before posting the first event so
                        # native input is delivered to the same window the
                        # semantic observer reports.
                        if document_title:
                            journey.native("raise", document_title)
                            journey.wait("document window focused", lambda d: d["focused"])
                        elif args.journey == "startup_preview":
                            # Some AppKit launches expose only the bundle
                            # title until the first root hit-test. Focus the
                            # published editor target instead of attempting
                            # an AXRaise on that non-document host window.
                            journey.click("editor.source")
                            journey.wait("document window focused", lambda d: d["focused"])
                        else:
                            journey.native("raise", fixture.name + " — tiptoptyp")
                            journey.wait("document window focused", lambda d: d["focused"])
                    journey.wait(
                        "fixture document loaded",
                        (lambda d: d["tabs"] == 1 and d["path"] is None and d["source_bytes"] > 0)
                        if args.journey == "startup_preview"
                        else (lambda d: d["tabs"] == 1 and d["path"] == str(fixture)),
                        timeout=15,
                    )
                    state = journey.snapshot()
                    if state["build"] != result["version"]:
                        raise AssertionError("running build differs from tested binary")
                    result["initial_state"] = state
                    if args.journey in ("partial_preview", "partial_zoom_stress", "partial_cache_edges", "partial_dividers") and not args.observe_only:
                        journey.partial_preview_default()
                    if args.observe_only:
                        # A fresh reply must follow an actual UI pass, and the socket
                        # must reject commands other than its one read-only operation.
                        previous = state["document"]["frame"]
                        if journey.snapshot()["document"]["frame"] <= previous:
                            raise AssertionError("inspection returned a stale frame")
                        with socket.socket(socket.AF_UNIX) as connection:
                            connection.settimeout(4)
                            connection.connect(str(directory / "inspect.sock"))
                            connection.sendall(b"close-window\n")
                            rejected = json.loads(connection.makefile("rb").readline())
                        if rejected.get("error") != "only snapshot is supported":
                            raise AssertionError("inspection accepted a mutation")
                        result["observation_passed"] = True
                        return
                    if args.capture_review:
                        journey.capture_initial_review()
                    for name in selected_journeys(args.journey):
                        journey.record("journey.start", name=name)
                        getattr(journey, name)()
                        result["journeys"].append(name)
                        journey.record("journey.passed", name=name)
                    result["native_windows"] = journey.native("windows")
                    result["passed"] = True
                except Exception as error:
                    print(f"Journey failed: {error}", flush=True)
                    if journey and process.poll() is None and not args.observe_only:
                        observe_failed_interaction(journey, result)
                    if args.review_failure and process.poll() is None:
                        resume = evidence / "failure.reviewed"
                        print(f"Failure review: app={directory / 'Journey.app'}; resume={resume}", flush=True)
                        deadline = time.monotonic() + 180
                        while not resume.exists() and time.monotonic() < deadline and process.poll() is None:
                            time.sleep(0.2)
                    raise
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait(timeout=5)
                    if (directory / "settings.ron").exists():
                        shutil.copy2(directory / "settings.ron", evidence / "settings.ron")
    except Exception as error:
        result["failure"] = str(error)
        raise
    finally:
        if journey:
            journey.log.close()
        (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"Desktop journey evidence: {evidence}", flush=True)


if __name__ == "__main__":
    main()
