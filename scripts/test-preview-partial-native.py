#!/usr/bin/env python3
"""Opt-in pinned Tinymist comparison in a real macOS WKWebView.

Runs a fixed native view and bounded synthetic viewer operations, without
requiring Accessibility or changing saved application settings. It is separate
from composed-editor desktop journeys and does not establish their correctness.
No tools are downloaded. Run without builds or other profiling concurrently.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import plistlib
import re
import signal
import statistics
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
SCENE = ROOT / "scripts/fixtures/tinymist-partial-large.typ"
STRESS_SCENE = ROOT / "scripts/fixtures/tinymist-zoom-stress.typ"
SOURCE = ROOT / "scripts/test-preview-partial-native.swift"
NATIVE_BUILD_TIMEOUT = 240
PROCESS_STOP_TIMEOUT = 5
PHASES = ("resize", "scroll", "zoom", "drain")
PHASE_COUNTERS = ("rescale", "rerender", "viewport", "frameCount", "frameSamplesDropped")
PHASE_SPANS = ("rescaleSyncMs", "rerenderSyncMs")
PHASE_METRICS = ("durationMs", *PHASE_COUNTERS, *PHASE_SPANS, "frameGapP95Ms", "frameGapMaxMs")


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def read_command(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=False)
        return {"command": command, "exit_code": result.returncode,
                "output": (result.stdout + result.stderr).strip()[:12000]}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "unavailable": str(error)}


def stop_process_group(process):
    """Stop the isolated build and children even if its driver exits first."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=PROCESS_STOP_TIMEOUT)
    except subprocess.TimeoutExpired:
        pass
    # Waiting for the driver alone does not establish that swift-frontend died.
    # Kill any remaining descendants in its dedicated group before returning.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=PROCESS_STOP_TIMEOUT)


def build_native_probe(arguments, log):
    """An optimized build has a bounded allowance and owns its process tree."""
    process = subprocess.Popen(arguments, stdout=log, stderr=log, start_new_session=True)
    try:
        return_code = process.wait(timeout=NATIVE_BUILD_TIMEOUT)
    except BaseException:
        stop_process_group(process)
        raise
    if return_code:
        raise subprocess.CalledProcessError(return_code, arguments)


def native_binary_receipt(binary, source_hash):
    """Reuse only a frozen binary whose original run records its exact inputs."""
    binary = Path(binary).resolve()
    if not binary.is_file():
        raise FileNotFoundError(f"Missing reusable native probe: {binary}")
    metadata_path = next((parent / "metadata.json" for parent in binary.parents
                          if (parent / "metadata.json").is_file()), None)
    if metadata_path is None:
        raise ValueError("Reusable native probe needs metadata.json in its original ancestor run directory")
    if metadata_path.stat().st_size > 1024 * 1024:
        raise ValueError(f"Native reuse metadata exceeds its 1 MiB bound: {metadata_path}")
    try:
        metadata = json.loads(metadata_path.read_text())
    except (OSError, ValueError) as error:
        raise ValueError(f"Cannot read native reuse receipt: {metadata_path}") from error
    if not isinstance(metadata, dict) or metadata.get("native_probe_sha256") != source_hash:
        raise ValueError(f"Reusable native probe source differs from current Swift source: {metadata_path}")
    if metadata.get("native_probe_binary_sha256") != sha256(binary):
        raise ValueError(f"Reusable native probe binary differs from its recorded hash: {metadata_path}")
    build = metadata.get("native_build")
    if not isinstance(build, list) or not build or not all(isinstance(value, str) for value in build) or "-O" not in build:
        raise ValueError(f"Reusable native probe has no recorded optimized build: {metadata_path}")
    if not isinstance(metadata.get("swift_version"), dict) or not metadata["swift_version"]:
        raise ValueError(f"Reusable native probe has no recorded Swift metadata: {metadata_path}")
    return {"binary": binary, "metadata_path": metadata_path, "native_build": build,
            "swift_version": metadata["swift_version"]}


def start_server(program, fixture, mode, lane):
    working = lane / "workspace"
    working.mkdir()
    source = working / "main.typ"
    source.write_bytes(fixture.read_bytes())
    arguments = [str(program), "preview", "--no-open", "--root", str(working),
                 "--host", "127.0.0.1:0", "--data-plane-host", "127.0.0.1:0",
                 "--control-plane-host", "127.0.0.1:0", "--invert-colors", "never",
                 f"--partial-rendering={'true' if mode == 'partial' else 'false'}", str(source)]
    log_path = lane / "tinymist.log"
    log = log_path.open("w+")
    server = subprocess.Popen(arguments, stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            log.seek(0)
            match = re.search(r"Static file server listening on: ([\d.:]+)", log.read())
            if match:
                return server, log, "http://" + match[1], arguments
            if server.poll() is not None:
                raise RuntimeError(f"Tinymist exited before initialization; inspect {log_path}")
            time.sleep(0.05)
        raise TimeoutError(f"Tinymist did not start within 20 seconds; inspect {log_path}")
    except BaseException:
        stop_server(server)
        log.close()
        raise


def stop_server(server):
    if server.poll() is None:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)


def phase_receipts_valid(results):
    """Opt-in phases must partition the existing combined counters and spans."""
    samples = [sample for result in results for sample in result.get("samples", [])]
    if not samples:
        return False
    for sample in samples:
        phases = sample.get("phases", [])
        if len(phases) != len(PHASES) or [phase.get("phase") for phase in phases] != list(PHASES):
            return False
        for phase in phases:
            for name in PHASE_METRICS:
                value = phase.get(name)
                if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                    return False
            if any(not isinstance(phase[name], int) for name in PHASE_COUNTERS):
                return False
        for name in (*PHASE_COUNTERS, *PHASE_SPANS):
            value = sample.get(name)
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                return False
            total = sum(phase[name] for phase in phases)
            if not math.isclose(total, value, rel_tol=1e-9, abs_tol=1e-6):
                return False
    return True


def buffer_receipts_valid(results, count):
    if not results:
        return False
    for result in results:
        samples = result.get("samples", [])
        if len(samples) != 1:
            return False
        sample = samples[0]
        times = sample.get("times", [])
        if len(times) != count or any(isinstance(value, bool) or not isinstance(value, (int, float)) or
                                     not math.isfinite(value) or value < 0 for value in times):
            return False
        if any(sample.get(key) is not True for key in ("dependencies", "unchanged", "populated", "equivalent")) or sample.get("scripts") != 0:
            return False
    return True


def summarize_phases(samples):
    """Four fixed-size phase summaries; no event history is retained."""
    return {name: {"sample_count": len(samples),
        "median": {metric: statistics.median(sample["phases"][index][metric] for sample in samples)
                   for metric in PHASE_METRICS}}
        for index, name in enumerate(PHASES)} if samples else {}


def summarize(results):
    summary = {}
    for result in results:
        samples = result.get("samples", [])
        summary[result["mode"]] = {"passed": result.get("passed", False),
            "sample_count": len(samples),
            "median": {name: statistics.median(sample[name] for sample in samples)
                for name in ["workloadWallMs", "rescaleSyncMs", "rerenderSyncMs",
                             "frameGapP95Ms", "frameGapMaxMs", "rerender", "rescale"]} if samples else {}}
        if samples and all("phases" in sample for sample in samples) and phase_receipts_valid([result]):
            summary[result["mode"]]["phases"] = summarize_phases(samples)
    return summary


def comparable_starts(results):
    """Reject incomplete lanes or nonfinite/mismatched viewer positions."""
    if not results:
        return False
    starts = []
    for result in results:
        if not isinstance(result, dict):
            return False
        samples = result.get("samples")
        if not isinstance(samples, list) or not samples:
            return False
        for sample in samples:
            if not isinstance(sample, dict) or not isinstance(sample.get("startState"), dict):
                return False
            starts.append(sample["startState"])
    def finite_number(value):
        return not isinstance(value, bool) and isinstance(value, (float, int)) and math.isfinite(value)
    reference = starts[0]
    for state in starts:
        if state.get("viewport") != [900, 700] or state.get("page") != 18:
            return False
        if any(not finite_number(state.get(name)) for name in ("scale", "pageFraction", "scrollTop")):
            return False
        if abs(state["scale"] - 1) > 1e-6 or abs(state["pageFraction"] - 0.15) > 0.001:
            return False
        if abs(state["scrollTop"] - reference["scrollTop"]) > 1:
            return False
    return True


def self_test():
    start = {"viewport": [900, 700], "page": 18, "pageFraction": 0.15, "scale": 1, "scrollTop": 22395.6}
    lanes = lambda other: [{"samples": [{"startState": start}]}, {"samples": [{"startState": other}]}]
    assert comparable_starts(lanes(dict(start)))
    # Preserve the concrete drift seen in the first native comparison.
    for change in [{"scrollTop": 16221}, {"page": 13}, {"scale": 1.1},
                   {"pageFraction": 0.25}, {"viewport": [650, 700]}]:
        assert not comparable_starts(lanes(dict(start, **change)))
    assert not comparable_starts([])
    assert not comparable_starts([{"samples": [{"startState": {}}]}])
    for name in ("scale", "pageFraction", "scrollTop"):
        for value in (float("nan"), float("inf"), float("-inf"), True, False, None, "1"):
            assert not comparable_starts(lanes(dict(start, **{name: value})))
            assert not comparable_starts(list(reversed(lanes(dict(start, **{name: value})))))
    for partial in ({}, {"samples": []}, {"samples": None}, {"samples": [None]},
                    {"samples": [{"startState": None}]}, None):
        assert not comparable_starts([{"samples": [{"startState": start}]}, partial])
    buffer = {"samples": [{"times": [1, 2], "dependencies": True, "unchanged": True, "populated": True, "equivalent": True, "scripts": 0}]}
    assert buffer_receipts_valid([buffer], 2)
    assert not buffer_receipts_valid([], 2)
    assert not buffer_receipts_valid([buffer], 3)
    for change in [{"times": [True, 2]}, {"times": [float("nan"), 2]}, {"equivalent": False}, {"scripts": 1}]:
        assert not buffer_receipts_valid([{"samples": [dict(buffer["samples"][0], **change)]}], 2)
    phases = [{"phase": name, **{metric: 1 for metric in PHASE_METRICS}} for name in PHASES]
    sample = {"phases": phases, **{metric: 4 for metric in (*PHASE_COUNTERS, *PHASE_SPANS)}}
    assert phase_receipts_valid([{"samples": [sample]}])
    assert not phase_receipts_valid([])
    assert not phase_receipts_valid([{"samples": [dict(sample, phases=phases[:3])]}])
    for change in [{"phase": "zoom"}, {"rescale": -1}, {"rerenderSyncMs": float("nan")},
                   {"frameCount": True}, {"viewport": 1.5}]:
        invalid = [dict(phase) for phase in phases]
        invalid[0].update(change)
        assert not phase_receipts_valid([{"samples": [dict(sample, phases=invalid)]}])
    assert not phase_receipts_valid([{"samples": [dict(sample, rescale=5)]}])
    assert list(summarize_phases([sample])) == list(PHASES)
    assert summarize_phases([sample])["resize"]["median"]["rescale"] == 1
    assert "phases" not in summarize([{"mode": "full", "samples": []}])["full"]
    from unittest.mock import Mock, call, patch
    command, log = ["swiftc", "-O", "source.swift"], object()
    for waits in ([subprocess.TimeoutExpired(command, NATIVE_BUILD_TIMEOUT),
                   subprocess.TimeoutExpired(command, PROCESS_STOP_TIMEOUT), 0],
                  [subprocess.TimeoutExpired(command, NATIVE_BUILD_TIMEOUT), 0, 0]):
        process = Mock(pid=74812)
        process.wait.side_effect = waits
        with patch.object(subprocess, "Popen", return_value=process) as launch, patch.object(os, "killpg") as kill:
            try:
                build_native_probe(command, log)
            except subprocess.TimeoutExpired:
                pass
            else:
                raise AssertionError("Timed-out build was accepted")
            launch.assert_called_once_with(command, stdout=log, stderr=log, start_new_session=True)
            assert kill.call_args_list == [call(process.pid, signal.SIGTERM), call(process.pid, signal.SIGKILL)]
            assert process.wait.call_args_list == [call(timeout=NATIVE_BUILD_TIMEOUT),
                call(timeout=PROCESS_STOP_TIMEOUT), call(timeout=PROCESS_STOP_TIMEOUT)]
    process = Mock(pid=74812)
    process.wait.side_effect = [KeyboardInterrupt(), 0, 0]
    with patch.object(subprocess, "Popen", return_value=process), patch.object(os, "killpg", side_effect=ProcessLookupError):
        try:
            build_native_probe(command, log)
        except KeyboardInterrupt:
            pass
        else:
            raise AssertionError("Interrupted build was accepted")
    for return_code in (0, 1):
        process = Mock(pid=74812)
        process.wait.return_value = return_code
        with patch.object(subprocess, "Popen", return_value=process), patch.object(os, "killpg") as kill:
            try:
                build_native_probe(command, log)
            except subprocess.CalledProcessError as error:
                assert return_code == error.returncode == 1
            else:
                assert return_code == 0
            kill.assert_not_called()
            process.wait.assert_called_once_with(timeout=NATIVE_BUILD_TIMEOUT)
    with tempfile.TemporaryDirectory(prefix="ttt-native-reuse-contract-") as directory:
        run_directory = Path(directory) / "previous-run"
        binary = run_directory / "Probe.app/Contents/MacOS/probe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"frozen optimized native probe")
        metadata_path = run_directory / "metadata.json"
        source_hash = hashlib.sha256(b"source").hexdigest()
        metadata = {"native_probe_sha256": source_hash, "native_probe_binary_sha256": sha256(binary),
            "native_build": ["swiftc", "-O", "source.swift", "-o", str(binary)],
            "swift_version": {"command": ["swiftc", "--version"], "exit_code": 0, "output": "Recorded Swift"}}
        metadata_path.write_text(json.dumps(metadata))
        receipt = native_binary_receipt(binary, source_hash)
        assert receipt["metadata_path"] == metadata_path.resolve()
        assert receipt["native_build"] == metadata["native_build"]
        assert receipt["swift_version"] == metadata["swift_version"]
        def rejected():
            try:
                native_binary_receipt(binary, source_hash)
            except ValueError:
                return True
            return False
        metadata_path.write_text(json.dumps(dict(metadata, native_probe_sha256="changed source")))
        assert rejected()
        metadata_path.write_text(json.dumps(metadata))
        binary.write_bytes(b"changed native binary")
        assert rejected()
        binary.write_bytes(b"frozen optimized native probe")
        metadata_path.write_text(json.dumps(dict(metadata, native_build=["swiftc", "source.swift"])))
        assert rejected()
        metadata_path.unlink()
        assert rejected()


def run(arguments):
    if sys.platform != "darwin":
        raise RuntimeError("Native probe needs macOS with Swift, WebKit, and an unlocked desktop session")
    program = arguments.tinymist.resolve()
    fixture = (STRESS_SCENE if arguments.stress else arguments.fixture).resolve()
    after_adapter = arguments.adapter.resolve()
    baseline_adapter = arguments.baseline_adapter.resolve()
    for path in [program, fixture, after_adapter, baseline_adapter, SOURCE,
                 *([arguments.snapshot_buffer] if arguments.snapshot_buffer else []),
                 *([arguments.baseline_buffer] if arguments.baseline_buffer else [])]:
        if not path.is_file():
            raise FileNotFoundError(f"Missing probe input: {path}")
    if not re.search(r"#let page-count\s*=\s*48\b", fixture.read_text()):
        raise ValueError("This probe's coverage bounds and jumps require the shared 48-page fixture")
    reused = native_binary_receipt(arguments.native_binary, sha256(SOURCE)) if arguments.native_binary else None
    profiles = ROOT / ".tiptoptyp/profiles/partial-render-native"
    profiles.mkdir(parents=True, exist_ok=True)
    identifier = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ-")
    output = Path(tempfile.mkdtemp(prefix=identifier, dir=profiles))
    snapshots = ROOT / ".tiptoptyp/screenshots/agent-review/partial-render-native" / output.name
    snapshots.mkdir(parents=True)
    if reused:
        executable = reused["binary"]
        build_arguments = reused["native_build"]
        swift_metadata = reused["swift_version"]
    else:
        application = output / "Tinymist Partial Native QA.app"
        executable = application / "Contents/MacOS/partial-native-probe"
        executable.parent.mkdir(parents=True)
        (application / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "dev.tiptoptyp.partial-native-probe",
            "CFBundleName": "Tinymist Partial Native QA", "CFBundleExecutable": executable.name,
            "CFBundlePackageType": "APPL", "NSHighResolutionCapable": True}))
        build_arguments = ["swiftc", "-O", "-module-cache-path", str(output / "swift-module-cache"),
                           str(SOURCE), "-o", str(executable)]
        with (output / "swift-build.log").open("w") as build_log:
            build_native_probe(build_arguments, build_log)
        swift_metadata = read_command(["swiftc", "--version"])
    lanes = ["full", "partial"] if arguments.mode == "both" else [arguments.mode]
    if arguments.reverse:
        lanes.reverse()
    metadata = {
        "platform": platform.platform(), "architecture": platform.machine(),
        "scope": "Standalone native WKWebView, not the composed editor app; SVG renderer synchronous wall spans and active RAF gaps, not CPU/GPU time or compiler startup",
        "view_css_pixels": [900, 700], "actual_backing_scale_recorded_in_results": True,
        "renderer_binary": str(program), "renderer_sha256": sha256(program),
        "renderer_version": read_command([str(program), "--version"]),
        "fixture": str(fixture.relative_to(ROOT)), "fixture_sha256": sha256(fixture),
        "snapshot_buffer_sha256": sha256(arguments.snapshot_buffer) if arguments.snapshot_buffer else None,
        "baseline_buffer_sha256": sha256(arguments.baseline_buffer) if arguments.baseline_buffer else None,
        "stress_fixture": arguments.stress, "max_svg_nodes": 320000 if arguments.stress else 30000,
        "baseline_adapter": str(baseline_adapter), "baseline_adapter_sha256": sha256(baseline_adapter),
        "after_adapter": str(after_adapter), "after_adapter_sha256": sha256(after_adapter),
        "native_probe_sha256": sha256(SOURCE), "runner_sha256": sha256(Path(__file__)),
        "native_probe_binary_sha256": sha256(executable), "native_probe_binary": str(executable),
        "native_build": build_arguments, "swift_version": swift_metadata,
        "native_build_timeout_seconds": None if reused else NATIVE_BUILD_TIMEOUT,
        "native_build_cleanup_policy": "No build executed; original receipt linked above" if reused else
            "Dedicated process group; terminate then kill remaining descendants on timeout/interruption; two bounded 5-second driver waits",
        "native_probe_reused": bool(reused),
        "native_probe_reuse_metadata": str(reused["metadata_path"]) if reused else None,
        "revision": read_command(["git", "-C", str(ROOT), "rev-parse", "HEAD"]),
        "power_settings": read_command(["pmset", "-g"]),
        "warmup_seconds": arguments.warmup, "workload_warmups": 1,
        "measured_samples": arguments.samples, "lane_order": lanes,
        "per_sample_reset": "Before every workload warmup and measured sample:900×700 view,fit scale1,page18 plus15% offset,scrollLeft0,clear resize anchor;wait for2 stable RAF states before starting timing",
        "recipe": "36 resize steps (900→650→900),24 scroll steps (3000px out/back),10 Ctrl-wheel steps (−10×5,+10×5);20ms input spacing;800ms drain;48-page source unchanged during measurements",
        "frame_sample_bound": 2048, "error_sample_bound": 32,
        "phase_diagnostics_enabled": arguments.phase_diagnostics,
        "phase_diagnostics_policy": "When enabled: four resize/scroll/zoom/drain summaries reuse the single 2048-gap RAF buffer; counters and synchronous spans partition the combined totals. RAF gaps belong to the phase when their ending callback arrives, so boundary gaps can include work from the preceding phase. No extra RAF or idle loop.",
        "snapshots": str(snapshots), "screenshot_source": "WKWebView.takeSnapshot; no desktop capture",
        "comparison_limits": "Independent server/view per lane; warmed active-workload comparison, not cold startup; Swift scheduling included; do not compare absolute values with Chromium or other machines",
    }
    if arguments.snapshot_buffer:
        metadata.update(scope="Standalone optimized native WKWebView: synchronous idle SVG preparation only; excludes image rasterization/encoding, final SVG render latency and composed-editor cadence",
            workload_warmups=1, per_sample_reset="Page 18 at fit scale 1,900x700 view; source unchanged;50ms quiet interval between preparations",
            recipe="One preparation warmup then N measured preparations; DOM validation and optional baseline canonical equivalence outside timed spans",
            phase_diagnostics_enabled=False)
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    results = []
    for mode in lanes:
        lane = output / mode
        lane.mkdir()
        server, log, url, server_arguments = start_server(program, fixture, mode, lane)
        try:
            config = {"url": url, "adapter": str(baseline_adapter if mode == "full" else after_adapter),
                "result": str(lane / "result.json"), "snapshots": str(snapshots), "mode": mode,
                "warmupSeconds": arguments.warmup, "measuredSamples": arguments.samples, "pageCount": 48,
                "phaseDiagnostics": arguments.phase_diagnostics, "maxSvgNodes": metadata["max_svg_nodes"]}
            if arguments.baseline_buffer:
                config["baselineBuffer"] = str(arguments.baseline_buffer.resolve())
            if arguments.snapshot_buffer:
                config["snapshotBuffer"] = str(arguments.snapshot_buffer.resolve())
            configuration = lane / "config.json"
            configuration.write_text(json.dumps(config, indent=2) + "\n")
            with (lane / "native.log").open("w") as native_log:
                process = subprocess.run([str(executable), str(configuration)],
                    stdout=native_log, stderr=native_log, check=False, timeout=125)
            if not (lane / "result.json").is_file():
                raise RuntimeError(f"Native process exited {process.returncode} without a receipt; inspect {lane / 'native.log'}")
            receipt = json.loads((lane / "result.json").read_text())
            receipt["server_arguments"] = server_arguments
            receipt["process_exit_code"] = process.returncode
            results.append(receipt)
            print(f"Native {mode}: {'passed' if receipt['passed'] else 'failed'} ({lane / 'result.json'})", flush=True)
        finally:
            stop_server(server)
            log.close()
    if arguments.snapshot_buffer:
        passed = buffer_receipts_valid(results, arguments.samples) and all(
            result["passed"] and result["process_exit_code"] == 0 for result in results)
        summary = {result["mode"]: {**{key: value for key, value in result["samples"][0].items() if key != "times"},
                                  "medianPreparationMs": statistics.median(result["samples"][0]["times"])}
                   for result in results}
        receipt = {"passed": passed, "metadata": metadata, "results": results, "summary": summary}
        (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"passed": passed, "output": str(output / "result.json"), "summary": summary}, indent=2))
        return 0 if passed else 1
    scales = {result["backingScaleFactor"] for result in results}
    comparable = len(scales) == 1 and next(iter(scales)) > 0
    complete = len(results) == len(lanes) and all(len(result["samples"]) == arguments.samples for result in results)
    matched_starts = comparable_starts(results)
    valid_phases = not arguments.phase_diagnostics or phase_receipts_valid(results)
    passed = comparable and matched_starts and complete and valid_phases and all(result["passed"] and result["process_exit_code"] == 0 for result in results)
    receipt = {"passed": passed, "comparable_backing_scale": comparable, "complete": complete,
               "comparable_start_states": matched_starts,
               "metadata": metadata, "results": results, "summary": summarize(results)}
    if arguments.phase_diagnostics:
        receipt["phase_diagnostics_valid"] = valid_phases
    (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"passed": passed, "output": str(output / "result.json"), "summary": receipt["summary"]}, indent=2))
    return 0 if passed or arguments.measure_only else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tinymist", type=Path, default=ROOT / "toolchain/bin/tinymist-aarch64-apple-darwin")
    parser.add_argument("--native-binary", type=Path, help="Reuse a previously optimized probe only when its ancestor metadata.json matches current Swift source and binary hashes")
    parser.add_argument("--fixture", type=Path, default=SCENE)
    parser.add_argument("--baseline-buffer", type=Path, help="Optional frozen serializer for untimed canonical output equivalence in --snapshot-buffer mode")
    parser.add_argument("--snapshot-buffer", type=Path, help="Measure only idle buffered-SVG preparation, one warmup plus N samples at page 18; separate from active zoom latency")
    parser.add_argument("--stress", action="store_true", help="Use the 48-page unequal fractional paper fixture with dense matrices and nested math; separately bounded at 320,000 SVG nodes")
    parser.add_argument("--adapter", type=Path, default=ROOT / "src/preview_navigation.js")
    parser.add_argument("--baseline-adapter", type=Path, default=ROOT / "src/preview_navigation.js",
        help="Preserved original adapter for an old/new comparison; defaults to the current adapter's unchanged full mode")
    parser.add_argument("--mode", choices=["full", "partial", "both"], default="both")
    parser.add_argument("--reverse", action="store_true", help="Reverse lane order for repeated comparisons")
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--measure-only", action="store_true", help="Record contract failures without a nonzero final status")
    parser.add_argument("--phase-diagnostics", action="store_true", help="Add bounded resize/scroll/zoom/drain counters and RAF summaries using the existing sampler")
    parser.add_argument("--self-test", action="store_true", help="Check sample and phase admission without launching native tools")
    arguments = parser.parse_args()
    if not 0 <= arguments.warmup <= 15 or not 1 <= arguments.samples <= 10:
        parser.error("Warmup must be 0–15 seconds and samples must be 1–10")
    if arguments.baseline_buffer and not arguments.snapshot_buffer:
        parser.error("--baseline-buffer needs --snapshot-buffer")
    if arguments.snapshot_buffer and arguments.phase_diagnostics:
        parser.error("Buffer preparation and active phase diagnostics are separate workloads")
    try:
        self_test()
        if arguments.self_test:
            print("Native sample/phase/buffer admission, binary-reuse and build-cleanup self-tests passed")
            return 0
        return run(arguments)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Native probe unavailable or failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
