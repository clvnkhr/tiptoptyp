"""Real PTY theme negotiation, driven through the normal Settings controls.

This proves what a terminal application receives. It does not infer which
colors a particular TUI chooses to draw, or claim composed visual coverage.
The runner supplies an isolated default settings profile.
"""

import json
import os
from pathlib import Path
import shlex
import shutil
import sys


# A real child program, not an inspection command or simulated terminal. Its
# first output is the query burst, before any banner or render warmup. Receipt
# files are bounded and written only on protocol transitions, never on a timer.
PROBE_SOURCE = r'''import json
import os
from pathlib import Path
import re
import select
import sys
import time
import tty

receipt = Path(sys.argv[1])
state = {"pid": os.getpid(), "startup": None, "updates": [], "error": None}
palette = {}
initial = {"colors": {}, "scheme": None}
active = initial
buffer = b""
osc = re.compile(rb"\x1b\](10|11|4;\d+);rgb:([0-9a-fA-F]{4})/([0-9a-fA-F]{4})/([0-9a-fA-F]{4})(?:\x07|\x1b\\)")
scheme = re.compile(rb"\x1b\[\?997;([12])n")

def save():
    temporary = receipt.with_suffix(".tmp")
    temporary.write_text(json.dumps(state, sort_keys=True))
    temporary.replace(receipt)

def query_defaults():
    os.write(sys.stdout.fileno(), b"\x1b]10;?\x07\x1b]11;?\x07")

try:
    if not os.isatty(sys.stdin.fileno()):
        raise RuntimeError("theme probe needs a real PTY")
    tty.setraw(sys.stdin.fileno())
    burst = b"\x1b[?2031h\x1b]10;?\x07\x1b]11;?\x07\x1b[?996n"
    burst += b"".join(f"\x1b]4;{index};?\x07".encode() for index in range(256))
    os.write(sys.stdout.fileno(), burst)
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if not select.select([sys.stdin], [], [], 1)[0]:
            continue
        data = os.read(sys.stdin.fileno(), 16384)
        if not data:
            raise RuntimeError("PTY closed before the theme checks finished")
        buffer += data
        if len(buffer) > 65536:
            raise RuntimeError("unparsed protocol input exceeded the bound")
        while True:
            color = osc.search(buffer)
            report = scheme.search(buffer)
            if color is None and report is None:
                break
            if color is not None and (report is None or color.start() < report.start()):
                buffer = buffer[color.end():]
                name = color[1].decode()
                rgb = [int(color[index], 16) >> 8 for index in (2, 3, 4)]
                if name.startswith("4;"):
                    palette[int(name[2:])] = rgb
                else:
                    active["colors"][name] = rgb
            else:
                buffer = buffer[report.end():]
                value = int(report[1])
                if state["startup"] is None:
                    initial["scheme"] = value
                else:
                    if len(state["updates"]) >= 16:
                        raise RuntimeError("excessive palette notifications")
                    active = {"scheme": value, "colors": {}, "complete": False}
                    state["updates"].append(active)
                    save()
                    query_defaults()
            if state["startup"] is None and len(palette) == 256 and len(initial["colors"]) == 2 and initial["scheme"] is not None:
                initial["palette_count"] = len(palette)
                initial["palette"] = palette
                state["startup"] = initial
                save()
            elif state["startup"] is not None and active is not initial and len(active["colors"]) == 2 and not active["complete"]:
                active["complete"] = True
                save()
    raise RuntimeError("theme probe timed out")
except Exception as error:
    state["error"] = str(error)
    save()
    raise
'''


def terminal_theme(self):
    """Startup queries, dark transition, same-mode palette change, no churn."""
    document = self.snapshot()["document"]
    source = document["source_fingerprint"]
    original_panel = document["panel"]
    initial_count = document["terminal_count"]
    child_path = self.directory / "terminal-theme-probe.py"
    receipt_path = self.directory / "terminal-theme-receipt.json"
    child_path.write_text(PROBE_SOURCE)
    shutil.copyfile(child_path, self.evidence / child_path.name)

    def receipt():
        if not receipt_path.exists():
            return None
        result = json.loads(receipt_path.read_text())
        if result["error"]:
            raise AssertionError("terminal theme probe: " + result["error"])
        return result

    def choose_mode(mode):
        self.settings()
        self.setting_section("Appearance", "Appearance")
        self.wait_target("settings.theme." + mode)
        self.click("settings.theme." + mode)
        self.wait("interface becomes " + mode.lower(), lambda d: d["dark"] == (mode == "Dark"))
        self.close_settings()

    def unchanged(d):
        return d["source_fingerprint"] == source and d["terminal_count"] == initial_count + 1

    def light(rgb):
        return (2126 * rgb[0] + 7152 * rgb[1] + 722 * rgb[2]) >= 128 * 10000

    choose_mode("Light")
    if self.snapshot()["document"]["panel"] is None:
        self.key(23)
        self.wait("bottom panel opens", lambda d: d["panel"] is not None)
    self.click("panel.terminal")
    self.wait("Terminal selected for theme negotiation", lambda d: d["panel"] == "Terminal")
    self.click("terminal.new")
    self.wait("fresh light terminal starts", lambda d: unchanged(d) and d["terminal_active"] == initial_count, timeout=15)
    self.click("terminal.input")
    command = "exec " + " ".join(shlex.quote(str(value)) for value in (sys.executable, "-u", child_path, receipt_path))
    self.native("text", command)
    self.key(36, "none")
    self.wait("first PTY output receives all 258 palette replies and the light scheme", lambda d: unchanged(d) and bool((r := receipt()) and r["startup"]), timeout=15)
    startup = receipt()["startup"]
    if startup["scheme"] != 2 or not light(startup["colors"]["11"]):
        raise AssertionError("light terminal advertised the wrong startup theme")
    if startup["palette_count"] != 256 or receipt()["updates"]:
        raise AssertionError("startup queries were lost or duplicated as unsolicited reports")
    pid = receipt()["pid"]
    if pid in (os.getpid(), self.process.pid) or pid <= 1:
        raise AssertionError("theme receipt came from an invalid child process")
    os.kill(pid, 0)
    self.record("terminal.theme.startup", receipt=startup, child_pid=pid)

    choose_mode("Dark")
    self.wait("running PTY receives the dark mode 2031 update", lambda d: unchanged(d) and bool((r := receipt()) and len(r["updates"]) == 1 and r["updates"][0]["complete"]), timeout=10)
    dark = receipt()["updates"][0]
    if dark["scheme"] != 1 or light(dark["colors"]["11"]):
        raise AssertionError("dark transition reported inconsistent palette and scheme")
    self.record("terminal.theme.dark", receipt=dark)

    # Use the Settings JSON editor, including validation/save, to change the
    # host palette while retaining dark mode. Theme drafts are not test hooks.
    before = '"value": "tiptop-dark"'
    after = '"value": "gruvbox-dark"'
    self.tex_settings([(before, after)])
    self.wait("same-dark palette switch notifies the running PTY", lambda d: unchanged(d) and d["dark"] and bool((r := receipt()) and len(r["updates"]) == 2 and r["updates"][1]["complete"]), timeout=10)
    same_mode = receipt()["updates"][1]
    if same_mode["scheme"] != 1 or same_mode["colors"] == dark["colors"] or light(same_mode["colors"]["11"]):
        raise AssertionError("same-mode update did not expose the changed dark palette")
    self.record("terminal.theme.same_mode", receipt=same_mode)
    self.stable_state(lambda d: unchanged(d) and len(receipt()["updates"]) == 2, frames=30)
    shutil.copyfile(receipt_path, self.evidence / receipt_path.name)

    self.click("terminal.off")

    def stopped(d):
        if d["terminal_count"] != initial_count or d["source_fingerprint"] != source:
            return False
        try:
            os.kill(pid, 0)
            return False
        except ProcessLookupError:
            return True

    self.wait("off reaps the probe and preserves the source", stopped, timeout=15)
    self.tex_settings([(after, before)])
    self.settings()
    if self.snapshot()["document"]["settings_json"]["visible"]:
        self.click("settings.json.mode")
    self.setting_section("Appearance", "Appearance")
    self.click("settings.theme.Follow system")
    self.close_settings()
    if original_panel is None:
        self.key(23)
        self.wait("original hidden panel restored", lambda d: d["panel"] is None)
    elif original_panel != "Terminal":
        self.click("panel." + original_panel.lower())
        self.wait("original bottom subpanel restored", lambda d: d["panel"] == original_panel)
