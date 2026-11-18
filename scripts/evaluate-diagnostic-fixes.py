#!/usr/bin/env python3
"""Evaluate the production local-model/request/proposal code on public fixtures.

Build the probe in release mode, then pass --probe and a prepared --cache-dir.
First request includes integrity checks and model startup; repeats use the same
loaded process. No user document is read and inference never contacts a service.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform
import select
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def cpu_model():
    try:
        return subprocess.check_output(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"],
                                       text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unavailable"


def expected_source(case):
    lines = case["source"].splitlines(keepends=True)
    if not lines or case["source"].endswith("\n"):
        lines.append("")
    original = lines[case["line"] - 1]
    ending = "\r\n" if original.endswith("\r\n") else "\n" if original.endswith("\n") else ""
    lines[case["line"] - 1] = case["replacement"] + ending
    return "".join(lines)


def matched(case, response):
    if not response.get("ok"):
        return False
    result = response["result"]
    if case.get("expect_no_fix"):
        return not result["proposal_valid"] and result["proposal_error"] == "No change suggested."
    return result["proposal_valid"] and result["corrected"] == expected_source(case)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--threads", type=int, choices=range(1, 9), default=4)
    parser.add_argument("--repeats", type=int, choices=range(1, 11), default=5)
    args = parser.parse_args()
    probe = args.probe.resolve(strict=True)
    cache = args.cache_dir.resolve(strict=True)
    fixtures = [ROOT / "scripts/fixtures/diagnostic-fixes.json", ROOT / "scripts/fixtures/diagnostic-fixes-heldout.json"]
    cases = []
    for fixture in fixtures:
        group = "heldout" if "heldout" in fixture.stem else "initial"
        cases.extend((group, case) for case in json.loads(fixture.read_text())["cases"])
    metadata = json.loads(subprocess.check_output([str(probe), "--metadata"], text=True))
    for key, path in {
        "backend_sha256": ROOT / "src/diagnostic_fix.rs",
        "setup_sha256": ROOT / "src/diagnostic_fix/setup.rs",
        "request_sha256": ROOT / "src/diagnostic_fix_request.rs",
        "catalog_sha256": ROOT / "resources/diagnostic-fix/manifest.json",
    }.items():
        if metadata[key] != sha256(path):
            raise RuntimeError(f"The probe has stale production code: {path.name}; rebuild it")
    report = {
        "format": 1, "date_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "profile": "release", "platform": platform.platform(), "architecture": platform.machine(),
        "hardware": cpu_model(),
        "runtime_device_policy": "llama.cpp defaults; no forced CPU/GPU offload policy. Effective offloaded layer count was not measured.",
        "threads": args.threads, "probe_sha256": sha256(probe), "implementation": metadata,
        "fixture_sha256": {str(path.relative_to(ROOT)): sha256(path) for path in fixtures},
        "workload": "Exact production diagnostic requests, local generation and proposal validation. First request includes cached-file SHA checks and model process startup; network downloads excluded. Repeats use the identical first fixture and the same loaded process. No viewport or theme applies to this worker-only probe.",
        "limitations": "Small authored corpus, not general accuracy. OS filesystem/Metal caches are uncontrolled; no physical Intel measurement. Compiler fixture checks are separate; model proposals are never executed by this runner.",
        "results": [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    stderr = args.output.with_suffix(".log")
    with stderr.open("w") as log:
        process = subprocess.Popen([str(probe), "--cache-dir", str(cache), "--threads", str(args.threads)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1)
        try:
            work = cases + [("repeat", {**cases[0][1], "id": f"repeat-{index}"}) for index in range(args.repeats)]
            for group, case in work:
                process.stdin.write(json.dumps(case) + "\n")
                process.stdin.flush()
                if not select.select([process.stdout], [], [], 150)[0]:
                    raise TimeoutError("The production model did not finish within its bounded request budget")
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError("The production probe exited before responding")
                response = json.loads(line)
                if response.get("id") != case["id"]:
                    raise RuntimeError("The production probe responded to a different fixture")
                response.update(group=group, matched_expected=matched(case, response))
                report["results"].append(response)
                args.output.write_text(json.dumps(report, indent=2) + "\n")
                print(json.dumps(response), flush=True)
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=10)
    report["summary"] = {}
    for group in ("initial", "heldout"):
        rows = [row for row in report["results"] if row["group"] == group]
        report["summary"][group] = {"matched": sum(row["matched_expected"] for row in rows), "total": len(rows)}
    report["summary"]["cold_first_ms"] = report["results"][0]["elapsed_ms"]
    warm = [row["elapsed_ms"] for row in report["results"] if row["group"] == "repeat"]
    report["summary"]["warm_identical_median_ms"] = statistics.median(warm)
    report["summary"]["warm_identical_min_ms"] = min(warm)
    report["summary"]["warm_identical_max_ms"] = max(warm)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["summary"]), flush=True)


if __name__ == "__main__":
    main()
