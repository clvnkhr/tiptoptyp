#!/usr/bin/env python3
"""Build and run the isolated macOS window contract as a real app bundle.

A bare cargo-test executable cannot reliably activate on macOS. The report is
required even when LaunchServices returns success; missing desktop capability,
a failed assertion, or a timeout is a failure, never a silent skip.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-only", action="store_true", help="prepare the app for a desktop test controller without launching it")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("this desktop lane requires macOS and a logged-in WindowServer session")
    root = Path(__file__).resolve().parent.parent
    build = subprocess.run([
        "cargo", "test", "--locked", "--features", "native-ui-tests", "--test",
        "native_window_lifecycle", "--no-run", "--message-format=json",
    ], cwd=root, check=True, stdout=subprocess.PIPE, text=True)
    executable = None
    for line in build.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "native_window_lifecycle":
            executable = message.get("executable")
    if not executable:
        raise RuntimeError("Cargo did not produce the native test executable")
    evidence = root / ".tiptoptyp/native-window-tests"
    evidence.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=evidence))
    app = run / "NativeWindowContract.app"
    contents = app / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    binary = contents / "MacOS/NativeWindowContract"
    # A symlink back into target/ does not provide a reliable main bundle.
    shutil.copy2(executable, binary)
    report = run / "result.txt"
    (contents / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleExecutable": binary.name,
        "CFBundleIdentifier": "dev.tiptoptyp.native-window-contract." + run.name.replace("_", "-"),
        "CFBundleName": "NativeWindowContract",
        "CFBundlePackageType": "APPL",
        "NSHighResolutionCapable": True,
        "LSEnvironment": {"TIPTOPTYP_NATIVE_TEST_REPORT": str(report)},
    }))
    print(app, flush=True)
    if args.prepare_only:
        return
    try:
        subprocess.run(["/usr/bin/open", "-W", "-n", str(app)], check=True, timeout=45)
    except subprocess.TimeoutExpired:
        # Only this uniquely named fixture is ours to terminate.
        processes = subprocess.check_output(["ps", "-axo", "pid=,args="], text=True)
        for line in processes.splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) == 2 and fields[1] == str(binary):
                os.kill(int(fields[0]), signal.SIGTERM)
        raise RuntimeError(f"native fixture timed out; evidence: {run}") from None
    result = report.read_text().strip() if report.exists() else "missing native test report"
    print(result)
    if result != "completed=true cancellations=1 closed=true failure=None":
        raise RuntimeError(f"native window contract failed; evidence: {run}")


if __name__ == "__main__":
    main()
