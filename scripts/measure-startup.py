#!/usr/bin/env python3
"""Measure fresh native UI readiness; no screenshot or preview-readiness claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import socket
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def measure(binary, fixture, evidence, ordinal, sample=False):
    with tempfile.TemporaryDirectory(prefix="ttt-start-", dir="/tmp") as temporary:
        directory = Path(temporary).resolve()
        contents = directory / "Startup.app/Contents"
        (contents / "MacOS").mkdir(parents=True)
        executable = contents / "MacOS/tiptoptyp"
        executable.symlink_to(binary)
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "tiptoptyp", "CFBundleIdentifier": "dev.tiptoptyp.startup",
            "CFBundleName": "Startup probe", "CFBundlePackageType": "APPL",
            "NSHighResolutionCapable": True,
        }))
        (directory / "typst.toml").write_text("")
        path = directory / ("startup." + fixture)
        if fixture != "empty":
            path.write_text("= Startup\nSmall startup fixture.\n" if fixture == "typ"
                            else "\\documentclass{article}\n\\begin{document}\nStartup.\n\\end{document}\n")
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("TIPTOPTYP_UI_", "TIPTOPTYP_PROFILE", "TIPTOPTYP_DESKTOP_TEST", "GIT_"))}
        environment.update(TIPTOPTYP_DESKTOP_TEST_DIR=str(directory), GIT_CEILING_DIRECTORIES=str(directory.parent))
        with (evidence / f"{ordinal}.log").open("w") as log:
            started = time.perf_counter()
            process = subprocess.Popen([str(executable), *([] if fixture == "empty" else [str(path)])],
                                       cwd=directory, env=environment, stdout=log, stderr=log)
            sampler = subprocess.Popen(["/usr/bin/sample", str(process.pid), "1", "1", "-mayDie", "-file", str(evidence / f"{ordinal}-cpu.txt")], stdout=log, stderr=log) if sample else None
            try:
                deadline = time.monotonic() + 20
                while True:
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError(f"startup failed; inspect {ordinal}.log")
                    endpoint = directory / "inspect.sock"
                    if endpoint.exists():
                        with socket.socket(socket.AF_UNIX) as connection:
                            connection.settimeout(4)
                            connection.connect(str(endpoint))
                            connection.sendall(b"snapshot\n")
                            response = json.loads(connection.makefile().readline())
                        if "error" not in response:
                            break
                        if response["error"] not in ("UI not initialized", "UI did not publish a fresh frame"):
                            raise RuntimeError(response["error"])
                    time.sleep(0.01)
                elapsed = (time.perf_counter() - started) * 1000
                document = response["document"]
                if "editor.source" not in response["targets"]:
                    raise RuntimeError("fresh snapshot did not contain an editor")
                if fixture != "empty" and (document["path"] != str(path) or document["source_bytes"] != path.stat().st_size):
                    raise RuntimeError("fresh snapshot did not contain the requested fixture")
                return {"milliseconds": elapsed, "frame": document["frame"],
                        "native_size": document["native_size"], "dark": document["dark"],
                        "backend": document["preview_backend"], "source_bytes": document["source_bytes"]}
            finally:
                if sampler is not None:
                    sampler.wait(timeout=8)
                driver = ROOT / ".tiptoptyp/desktop-ui-tests/desktop-ui-driver"
                if process.poll() is None:
                    subprocess.run([str(driver), "key", str(process.pid), "12", "cmd"],
                                   capture_output=True, timeout=6, check=False)
                    try:
                        process.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        process.terminate()
                        process.wait(timeout=6)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--fixture", choices=["empty", "typ", "tex"], default="empty")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--sample", action="store_true", help="diagnostic CPU sampling; keep these runs separate from timing comparisons")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin" or not 1 <= args.runs <= 20:
        parser.error("requires macOS and 1–20 runs")
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = [args.binary.resolve(), *([args.compare.resolve()] if args.compare else [])]
    driver = ROOT / ".tiptoptyp/desktop-ui-tests/desktop-ui-driver"
    subprocess.run(["swiftc", str(ROOT / "scripts/desktop-ui-driver.swift"), "-o", str(driver)], check=True, timeout=90)
    preflight = json.loads(subprocess.check_output([str(driver), "preflight"], text=True))
    result = {"desktop": preflight, "ui_scale_percent": 100, "metric": "process spawn to first fresh editor-state snapshot; excludes preview readiness and screen presentation",
              "cache": "fresh process and fresh settings/workspace; filesystem caches are not flushed",
              "platform": platform.platform(), "fixture": args.fixture, "sampled": args.sample,
              "compiler": subprocess.check_output(["rustc", "--version"], text=True).strip(),
              "build_contract": "release with desktop-ui-tests; caller must build both binaries with matching flags",
              "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "binaries": [{"path": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                            "version": subprocess.check_output([str(binary), "--version"], text=True).strip(), "runs": []}
                           for binary in binaries]}
    for ordinal in range(args.runs + 1):  # First launch is retained separately as warmup.
        for index, binary in enumerate(binaries):
            observation = measure(binary, args.fixture, args.output, f"{ordinal}-{index}", args.sample)
            result["binaries"][index]["runs"].append(observation)
            print(index, ordinal, f"{observation['milliseconds']:.2f} ms", flush=True)
    for binary in result["binaries"]:
        binary["median_ms_excluding_first"] = statistics.median(run["milliseconds"] for run in binary["runs"][1:])
    (args.output / "measurement.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps([binary["median_ms_excluding_first"] for binary in result["binaries"]]))


if __name__ == "__main__":
    main()
