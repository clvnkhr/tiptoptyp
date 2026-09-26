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
from pathlib import Path
import platform
import plistlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


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
    if len(value["filters"]) != 1 or "tiptoptyp-palette" not in value["filters"][0]:
        raise AssertionError("Tinymist page must receive exactly one palette filter")
    background, foreground = palette
    expected = "rgb(" + ", ".join(map(str, background[:3])) + ")"
    if value["background"] != expected:
        raise AssertionError("Tinymist paper differs from the effective palette")
    for channel in range(3):
        if abs(value["slopes"][channel] - (background[channel] - foreground[channel]) / 255) > 1e-6:
            raise AssertionError("Tinymist transfer slope differs from palette")
        if abs(value["intercepts"][channel] - foreground[channel] / 255) > 1e-6:
            raise AssertionError("Tinymist transfer intercept differs from palette")


class Journey:
    def __init__(self, process, driver, directory, evidence):
        self.process, self.driver, self.directory, self.evidence = process, driver, directory, evidence
        self.sequence = 0
        self.viewport = None
        self.multiple_windows = False
        self.review_preview = False
        self.log = (evidence / "events.jsonl").open("w")

    def record(self, action, **detail):
        self.log.write(json.dumps({"time": time.monotonic(), "action": action, **detail}) + "\n")
        self.log.flush()

    def native(self, command, *args):
        result = subprocess.run([str(self.driver), command, str(self.process.pid), *map(str, args)],
                                capture_output=True, text=True, timeout=6)
        self.record(command, arguments=args, stdout=result.stdout, stderr=result.stderr, exit=result.returncode)
        if result.returncode:
            raise RuntimeError(result.stderr or result.stdout)
        return json.loads(result.stdout)

    def snapshot(self):
        if self.process.poll() is not None:
            raise RuntimeError(f"app exited unexpectedly: {self.process.returncode}")
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(4)
            connection.connect(str(self.directory / "inspect.sock"))
            connection.sendall(b"snapshot\n")
            data = connection.makefile("rb").readline(1_000_000)
        state = json.loads(data)
        self.record("snapshot", state=state)
        if "error" in state:
            raise RuntimeError(state["error"])
        if state["pid"] != self.process.pid or state["request"] <= self.sequence:
            raise AssertionError("wrong process or stale inspection response")
        self.sequence = state["request"]
        viewport = state["document"]["viewport"]
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

    def click(self, name):
        state = self.snapshot()
        target = state["targets"].get(name)
        if not target or not target["enabled"] or target["age_ms"] > 500:
            raise AssertionError(f"missing, disabled or stale hit target: {name}")
        if name.startswith("panel.") and state["document"]["panel"] is None:
            raise AssertionError("cannot click a hidden panel")
        if name.startswith("find.") and not state["document"]["find_visible"]:
            raise AssertionError("cannot click hidden Find controls")
        self.record("target", name=name, target=target)
        self.native("click", target["x"], target["y"])

    def stable(self, baseline, frames=30):
        previous = None
        for _ in range(frames):
            document = self.snapshot()["document"]
            if previous is not None and document["frame"] <= previous:
                raise AssertionError("stability check did not advance the UI frame")
            previous = document["frame"]
            assert_stable_height(baseline, document)

    def panels(self):
        self.key(23)  # Cmd+5: bottom panel, initially Problems
        state = self.wait("Problems opens", lambda d: d["panel"] == "Problems")
        baseline = height(state["document"])
        self.stable(baseline)
        for _ in range(3):
            self.click("panel.terminal")
            self.wait("Terminal selected", lambda d: d["panel"] == "Terminal")
            self.click("panel.problems")
            self.wait("Problems selected", lambda d: d["panel"] == "Problems")
            self.stable(baseline)
            self.key(23)
            self.wait("panel hidden", lambda d: d["panel"] is None)
            self.key(23)
            self.wait("Problems restored", lambda d: d["panel"] == "Problems")
            self.stable(baseline)
        self.key(23)
        self.wait("panel hidden", lambda d: d["panel"] is None)

    def find(self):
        for _ in range(3):
            self.key(3)  # Cmd+F
            self.wait("Find visible", lambda d: d["find_visible"] and d["find_focused"])
            self.key(3)
            self.wait("focused Cmd+F closes Find", lambda d: not d["find_visible"])
            self.stable_state(lambda d: not d["find_visible"])
        self.key(3)
        self.wait("Find reopens", lambda d: d["find_visible"] and d["find_focused"])
        for expected in (True, False, True, False):
            self.click("find.replace")
            self.wait("Replace toggles", lambda d: d["find_visible"] and d["replace_visible"] == expected)
            self.stable_state(lambda d: d["find_visible"] and d["replace_visible"] == expected)
        self.click("find.close")
        self.wait("one click closes Find", lambda d: not d["find_visible"])
        self.stable_state(lambda d: not d["find_visible"])

    def empty(self):
        self.key(13)  # Cmd+W closes the fixture tab
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
        self.click("settings.search")
        self.key(0)
        self.native("text", query)
        self.wait_target("settings.result." + title)
        self.click("settings.result." + title)

    def wait_target(self, name):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            target = self.snapshot()["targets"].get(name)
            if target and target["enabled"] and target["age_ms"] < 100:
                return
            time.sleep(0.05)
        raise AssertionError(f"target did not become visible: {name}")

    def preview(self):
        fixture = self.directory / "journey.typ"
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
                self.setting_section("appearance", "Appearance")
                self.wait_target("settings.theme." + ("Dark" if dark else "Light"))
                self.click("settings.theme." + ("Dark" if dark else "Light"))
                self.wait("interface theme applied", lambda d: d["dark"] == dark)
                if self.snapshot()["document"]["comfy"] != comfy:
                    self.wait_target("settings.comfy")
                    self.click("settings.comfy")
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
                if self.review_preview and dark and comfy:
                    resume = self.evidence / (backend + ".reviewed")
                    print(f"Visual review: {backend}; app={self.directory / 'Journey.app'}; resume={resume}", flush=True)
                    deadline = time.monotonic() + 180
                    while not resume.exists():
                        if self.process.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError("visual review not completed within three minutes")
                        time.sleep(0.2)

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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--journey", choices=["all", "panels", "find", "empty", "focus", "preview"], default="all")
    parser.add_argument("--review-preview", action="store_true", help="pause at each dark/comfy renderer for independent on-screen review; not an automated visual pass")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--observe-only", action="store_true", help="check real-app inspection without input; does not pass the interaction suite")
    modes.add_argument("--prepare-only", action="store_true", help="build runner and editor without launching")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("requires macOS and a logged-in desktop; this is not a headless test")
    evidence_root = ROOT / ".tiptoptyp/desktop-ui-tests"
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="run-", dir=evidence_root))
    driver = evidence_root / "desktop-ui-driver"
    result = {"passed": False, "journeys": [], "platform": platform.platform(),
              "evidence": "native input and read-only state; no visual composition verification"}
    process = None
    journey = None
    try:
        subprocess.run(["swiftc", str(ROOT / "scripts/desktop-ui-driver.swift"), "-o", str(driver)], check=True, timeout=90)
        if not args.prepare_only and not args.observe_only:
            import pwd
            shell = os.environ.get("SHELL") or pwd.getpwuid(os.getuid()).pw_shell
            if args.journey in ("all", "panels") and Path(shell).resolve() != Path("/bin/zsh"):
                raise RuntimeError("terminal journey currently requires /bin/zsh so startup files can be isolated with ZDOTDIR")
            preflight = subprocess.run([str(driver), "preflight"], capture_output=True, text=True, timeout=6)
            result["preflight"] = preflight.stdout.strip()
            if preflight.returncode:
                raise RuntimeError("desktop driver lacks Accessibility/input permission; enable it for the launching host in macOS Privacy & Security. " + preflight.stdout)
        build = subprocess.run(["cargo", "build", "--locked", "--features", "desktop-ui-tests", "--bin", "tiptoptyp", "--message-format=json"],
                               cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True, timeout=900)
        artifacts = [json.loads(line) for line in build.stdout.splitlines()]
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
            fixture = directory / ("journey.typ" if args.journey in ("all", "preview") else "journey.txt")
            fixture.write_text('#set page(width: 200pt, height: 200pt, margin: 20pt)\n= Preview journey\nBlack text on white paper.\n' if fixture.suffix == ".typ" else "Desktop journey fixture.\n")
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("TIPTOPTYP_UI_", "TIPTOPTYP_PROFILE", "TIPTOPTYP_DESKTOP_TEST"))}
            environment["TIPTOPTYP_DESKTOP_TEST_DIR"] = str(directory)
            # macOS default-shell startup and history belong to this fixture.
            environment["ZDOTDIR"] = str(directory)
            environment["HISTFILE"] = str(directory / "shell-history")
            (directory / "empty-shell-rc").write_text("")
            environment["ENV"] = str(directory / "empty-shell-rc")
            environment["BASH_ENV"] = str(directory / "empty-shell-rc")
            with (evidence / "app.log").open("w") as log:
                process = subprocess.Popen([str(binary), str(fixture)], cwd=directory, env=environment,
                                           stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    deadline = time.monotonic() + 15
                    while not (directory / "inspect.sock").exists():
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("editor did not start; see app.log")
                        time.sleep(0.05)
                    journey = Journey(process, driver, directory, evidence)
                    journey.review_preview = args.review_preview
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
                        journey.native("activate")
                        journey.wait("document window focused", lambda d: d["focused"])
                    journey.wait("fixture document loaded", lambda d: d["tabs"] == 1 and d["path"] == str(fixture), timeout=15)
                    state = journey.snapshot()
                    if state["build"] != result["version"]:
                        raise AssertionError("running build differs from tested binary")
                    result["initial_state"] = state
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
                    for name in (["panels", "find", "focus", "preview", "empty"] if args.journey == "all" else [args.journey]):
                        journey.record("journey.start", name=name)
                        getattr(journey, name)()
                        result["journeys"].append(name)
                        journey.record("journey.passed", name=name)
                    result["native_windows"] = journey.native("windows")
                    result["passed"] = True
                except Exception:
                    if journey and process.poll() is None and not args.observe_only:
                        try:
                            result["failure_native_windows"] = journey.native("windows")
                        except Exception as error:
                            result["native_inspection_error"] = str(error)
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
