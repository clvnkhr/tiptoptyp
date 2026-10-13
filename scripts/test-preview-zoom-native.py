#!/usr/bin/env python3
"""Production native zoom cache over the real pinned Tinymist frontend.

Owns a visible, floating, nonactivating window, sidecar and temporary fixture. No global input.
Pass --binary with an explicitly built desktop-ui-tests native_preview_zoom test.
This verifies native composition in its own WKWebView, not the editor window.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def run(binary, mode, output, refresh_race=False, transitions=False, fixture=None):
    source_fixture = (fixture or ROOT / "scripts/fixtures/tinymist-zoom-stress.typ").resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ttt-native-zoom-") as directory:
        folder = Path(directory)
        shutil.copyfile(source_fixture, folder / "main.typ")
        with (output / f"tinymist-{mode}.log").open("w+") as log:
            command = [str(ROOT / "toolchain/bin/tinymist-aarch64-apple-darwin"), "preview", "--no-open",
                       f"--partial-rendering={'true' if mode == 'partial' else 'false'}", "--root", directory,
                       "--host", "127.0.0.1:0", "--data-plane-host", "127.0.0.1:0", "--control-plane-host",
                       "127.0.0.1:0", "--invert-colors", "never", str(folder / "main.typ")]
            server = subprocess.Popen(command, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 20
                while True:
                    log.seek(0)
                    match = re.search(r"Static file server listening on: ([\d.:]+)", log.read())
                    if match:
                        break
                    if server.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f"Tinymist startup failed: {output / f'tinymist-{mode}.log'}")
                    time.sleep(.05)
                receipt = output / f"native-{mode}.json"
                # A reused output directory cannot supply a stale pass.
                receipt.unlink(missing_ok=True)
                image_dir = ROOT / ".tiptoptyp/screenshots/agent-review/zoom-edge"
                image_dir.mkdir(parents=True, exist_ok=True)
                environment = {**os.environ, "TIPTOPTYP_NATIVE_ZOOM_TEST": "1",
                               "TIPTOPTYP_NATIVE_ZOOM_URL": "http://" + match[1],
                               "TIPTOPTYP_NATIVE_ZOOM_RECEIPT": str(receipt),
                               "TIPTOPTYP_NATIVE_ZOOM_PNG": str(image_dir / f"native-{mode}-{output.name}.png")}
                native_command = [str(binary), "--background",
                                  *(["--refresh-race"] if refresh_race else []),
                                  *(["--transition-regression"] if transitions else [])]
                started = time.monotonic()
                started_utc = datetime.now(timezone.utc).isoformat()
                with (output / f"native-{mode}.log").open("w") as native_log:
                    process = subprocess.run(native_command, env=environment, stdout=native_log,
                                             stderr=native_log, timeout=100 if transitions else 50,
                                             check=False)
                # New regressions write each observation before asserting. Keep
                # a failed baseline receipt so matched before/after geometry and
                # feedback spans are inspectable; a process failure is no pass.
                result = json.loads(receipt.read_text()) if receipt.is_file() else {}
                checks = {
                    "native_process": process.returncode == 0,
                    "actual_tinymist": result.get("actual_tinymist") is True,
                    "actual_partial": result.get("partial") is (mode == "partial"),
                    "responsive_during_block": result.get("responsive_during_block") is True,
                    "one_final_vector_request": result.get("vector_requests") == 1,
                    "transition_regression": not transitions or result.get("transition_regression", {}).get("passed") is True,
                }
                metadata = {
                    "started_utc": started_utc,
                    "command": native_command, "returncode": process.returncode,
                    "process_seconds": time.monotonic() - started, "checks": checks,
                    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                    "tinymist_sha256": hashlib.sha256(Path(command[0]).read_bytes()).hexdigest(),
                    "fixture": str(source_fixture),
                    "fixture_sha256": hashlib.sha256((folder / "main.typ").read_bytes()).hexdigest(),
                    "viewport": [640, 420], "warmup_seconds": 3,
                    "input": "owned AppKit zoom keys; transitions additionally inject CaptureState zoom and WebView bounds",
                    "window_mode": "Prohibited throughout; floating level3 owned window ordered front without making key or activating app",
                    "not_established": "physical trackpad pinch or GPU presentation",
                    "receipt": str(receipt),
                }
                (output / f"metadata-{mode}.json").write_text(json.dumps(metadata, indent=2) + "\n")
                passed = all(checks.values())
                print(f"Native production cached zoom over real {mode} Tinymist: {'passed' if passed else 'FAILED'} — {receipt}")
                return passed
            finally:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--mode", choices=["full", "partial", "both"], default="both")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--fixture", type=Path, default=ROOT / "scripts/fixtures/tinymist-zoom-stress.typ",
                        help="Fixture for this measurement lane; keep identical within each before/after pair")
    parser.add_argument("--refresh-race", action="store_true")
    parser.add_argument("--transition-regression", action="store_true",
                        help="Check real mid-page divider anchors and repeated injected zoom/cache transitions; not physical pinch")
    parser.add_argument("--measure-only", action="store_true",
                        help="Keep failed before/after receipts and continue both modes without a nonzero exit; failures remain explicit")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"Missing native probe: {binary}")
    fixture = args.fixture.resolve()
    if not fixture.is_file():
        parser.error(f"Missing fixture: {fixture}")
    output = (args.output or ROOT / ".tiptoptyp/profiles" / ("zoom-native-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"))).resolve()
    passed = []
    for mode in (["full", "partial"] if args.mode == "both" else [args.mode]):
        passed.append(run(binary, mode, output, args.refresh_race, args.transition_regression, fixture))
    sys.exit(0 if all(passed) or args.measure_only else 1)
