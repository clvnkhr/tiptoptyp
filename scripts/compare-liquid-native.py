#!/usr/bin/env python3
"""Compare the production native Liquid modules with the pinned Python reference.

This developer-only runner uses JSONL for both implementations, records hashes,
keeps inference single-file and serial, and never changes model setup. Prepare
and verify the model first. Do not run builds or GUI benchmarks concurrently.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import stat
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "assets/liquid/correction-cases.json"
CATALOG_ROOT = ROOT / "resources/liquid"
MODEL_ID = "lfm25-encoder-350m-onnxruntime-v1"


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Worker:
    def __init__(self, command, log, *, native=False):
        self.log = Path(log)
        self.errors = self.log.open("w")
        if sys.platform == "darwin":
            command = ["/usr/bin/time", "-l", *map(str, command)]
        self.started = time.perf_counter_ns()
        kwargs = {}
        if native:
            kwargs["env"] = {**os.environ, "PATH": "/nonexistent-liquid-no-python"}
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.errors, bufsize=0, start_new_session=True, **kwargs)
        self.sequence = 0
        os.set_blocking(self.process.stdout.fileno(), False)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)

    def request(self, text, timeout=180):
        started = time.perf_counter_ns()
        self.sequence += 1
        request = memoryview((json.dumps(dict(id=self.sequence, text=text)) + "\n").encode())
        while request:
            request = request[os.write(self.process.stdin.fileno(), request):]
        deadline = time.monotonic() + timeout
        line = bytearray()
        while not line.endswith(b"\n"):
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise TimeoutError(f"Liquid request timed out; see {self.log}")
            try:
                part = os.read(self.process.stdout.fileno(), 65_537 - len(line))
            except BlockingIOError:
                continue
            if not part:
                raise ValueError(f"Missing Liquid response: {self.log}")
            line.extend(part)
            if len(line) > 65_536 or b"\n" in line[:-1]:
                raise ValueError(f"Invalid Liquid response: {self.log}")
        elapsed = (time.perf_counter_ns() - started) / 1e6
        response = json.loads(line)
        if response.get("id") != self.sequence or not response.get("ok"):
            raise ValueError(f"Liquid failed: {response}")
        if not isinstance(response.get("corrected"), str):
            raise ValueError("Liquid omitted corrected text")
        return response["corrected"], elapsed

    def close(self):
        self.selector.close()
        self.process.stdin.close()
        try:
            status = self.process.wait(30)
        except subprocess.TimeoutExpired:
            self.kill_group()
            self.process.wait()
            raise
        finally:
            self.errors.close()
            self.process.stdout.close()
        if status:
            raise ValueError(f"Liquid worker exited {status}; see {self.log}")
        data = self.log.read_text()
        match = re.search(r"(\d+)\s+maximum resident set size", data)
        return int(match[1]) if match else None

    def kill_group(self):
        import signal
        try:
            os.killpg(self.process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def abort(self):
        self.selector.close()
        self.kill_group()
        self.process.wait()
        self.process.stdin.close()
        self.process.stdout.close()
        self.errors.close()


def native_command(args):
    return [args.native_probe, "--cache-dir", args.native_cache, "--threads", str(args.threads)]


def regular_directory(path):
    if not stat.S_ISDIR(path.lstat().st_mode):
        raise ValueError(f"Liquid cache folder must be a regular directory: {path}")


def read_catalog(path, limit):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
        raise ValueError(f"Liquid catalog must be a bounded regular file: {path}")
    return json.loads(path.read_bytes())


def verified_asset(root, relative, entry):
    path = root / relative
    for parent in path.relative_to(root).parents:
        regular_directory(root / parent)
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != entry["bytes"]:
        raise ValueError(f"Liquid cache asset has the wrong size or type: {relative}")
    actual = sha(path)
    if actual != entry["sha256"]:
        raise ValueError(f"Liquid cache asset failed SHA-256 verification: {relative}")
    if path.name == "liquid-onnx" and not metadata.st_mode & 0o111:
        raise ValueError(f"Liquid cache helper is not executable: {relative}")
    return actual


def verify_prepared_cache(directory, *, target, catalog_root=None):
    """Reject first-use setup before starting any measured worker.

    The production client may download missing or damaged assets. A comparison
    must instead use a completely verified cache matching the current catalogs;
    file verification and ordinary native model loading still occur in its first
    measured request. Other-architecture assets are unnecessary.
    """
    catalog_root = CATALOG_ROOT if catalog_root is None else catalog_root
    if target not in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        raise ValueError(f"Unsupported native Liquid target: {target}")
    directory = Path(directory)
    if not directory.is_absolute():
        raise ValueError("Use an absolute native Liquid cache directory")
    try:
        catalog_path = catalog_root / "manifest.json"
        notices_path = catalog_root / "notices.json"
        catalog = read_catalog(catalog_path, 64 * 1024)
        notices = read_catalog(notices_path, 16 * 1024)
        if catalog["format"] != 3 or catalog["model_id"] != MODEL_ID or notices["format"] != 1:
            raise ValueError("Liquid comparison catalogs have an unsupported format or model")
        root = directory / "onnxruntime" / MODEL_ID
        for path in (directory, directory / "onnxruntime", root, root / "runtime",
                     root / "runtime" / target, root / "licenses"):
            regular_directory(path)
        installed_path = root / "manifest.json"
        if read_catalog(installed_path, 64 * 1024) != catalog:
            raise ValueError("Liquid installed manifest differs from the current download catalog")
        expected = {"liquid.onnx", "tokenizer.json", f"runtime/{target}/liquid-onnx",
                    f"runtime/{target}/libonnxruntime.dylib"}
        assets = {}
        for entry in catalog["files"]:
            architecture = entry.get("target")
            if architecture is None or architecture == target:
                relative = f"runtime/{target}/{entry['name']}" if architecture else entry["name"]
                if relative not in expected or relative in assets:
                    raise ValueError("Liquid catalog contains an unexpected or duplicate cache asset")
                assets[relative] = verified_asset(root, relative, entry)
        if assets.keys() != expected:
            raise ValueError("Liquid catalog must contain the model and complete current-architecture runtime")
        expected_notices = {"LICENSE.txt", "PROVENANCE.json", f"licenses/{target}.txt"}
        notice_assets = {}
        for entry in notices["files"]:
            if entry.get("target") in (None, target):
                if entry["name"] not in expected_notices or entry["name"] in notice_assets:
                    raise ValueError("Liquid catalog contains an unexpected or duplicate notice")
                notice_assets[entry["name"]] = verified_asset(root, entry["name"], entry)
        if notice_assets.keys() != expected_notices:
            raise ValueError("Liquid catalog must contain the model and current-architecture notices")
        return dict(target=target, catalog_sha256=sha(catalog_path),
                    notices_catalog_sha256=sha(notices_path),
                    installed_manifest_sha256=sha(installed_path),
                    asset_sha256=assets, notice_sha256=notice_assets)
    except (OSError, KeyError, TypeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(
            "Native Liquid cache is not prepared for comparison. Complete a first-use correction "
            f"or redownload Liquid before benchmarking; setup is excluded from timing. {error}"
        ) from error


def verified_native_inputs(args):
    # Query the compiled executable, not the Python driver's architecture: an
    # Intel probe can run under Rosetta on an ARM host.
    metadata = json.loads(subprocess.check_output(
        [args.native_probe, "--metadata"], text=True, timeout=10))
    if not isinstance(metadata, dict) or metadata.get("schema_version") != 1:
        raise ValueError("Native probe returned unsupported metadata")
    preflight = verify_prepared_cache(args.native_cache, target=metadata.get("target", ""))
    for field in ("catalog_sha256", "notices_catalog_sha256"):
        if metadata.get(field) != preflight[field]:
            raise ValueError("Native probe has stale compiled Liquid catalogs; rebuild it before comparison")
    preflight["probe_metadata"] = metadata
    return preflight


def verify(args):
    preflight = verified_native_inputs(args)
    cases = json.loads(FIXTURES.read_text())["cases"]
    worker = Worker(native_command(args), args.output / "native-parity.stderr", native=True)
    rows = []
    try:
        for case in cases:
            actual, elapsed = worker.request(case["text"])
            rows.append(dict(id=case["id"], expected=case["reference_corrected"],
                             actual=actual, match=actual == case["reference_corrected"],
                             elapsed_ms=elapsed))
        rss = worker.close()
    except BaseException:
        worker.abort()
        raise
    report = dict(schema_version=2, driver_architecture=platform.machine(),
                  native_binary_architecture=command_output(["file", "-b", args.native_probe]),
                  native_python_path_disabled=True,
                  native_cache_preflight=preflight, native_network_setup_excluded=True,
                  cases=len(rows), matched=sum(r["match"] for r in rows),
                  mismatches=[r for r in rows if not r["match"]], max_rss_bytes=rss,
                  fixture_sha256=sha(FIXTURES), native_binary_sha256=sha(args.native_probe),
                  results=rows)
    (args.output / "native-parity.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "results"}))
    if report["mismatches"]:
        raise ValueError("Native corrections differ from the pinned reference")


def command_output(command):
    return subprocess.check_output(command, text=True).strip()


def require_correction(runtime, case, actual):
    if actual != case["reference_corrected"]:
        raise ValueError(f"{runtime} correction differs for {case['id']}: {actual!r}")


def benchmark(args):
    if not args.python or not args.python_cache:
        raise ValueError("Benchmark needs --python and --python-cache")
    preflight = verified_native_inputs(args)
    cases = json.loads(FIXTURES.read_text())["cases"]
    by_text = {case["text"]: case for case in cases}
    texts = ["She go to school every day.", "This is is the result.",
             "The nonautonomous semigroup is well-posed."]
    longer = next(c for c in cases if 60 <= len(c["reference_passes"][0]["input_ids"]) <= 80)
    workload = [by_text[text] for text in texts] + [longer]
    commands = dict(native=native_command(args), python=[args.python, "-u",
        ROOT / "scripts/liquid-grammar-worker.py", "--cache-dir", args.python_cache,
        "--threads", str(args.threads)])
    runs = []
    for index, name in enumerate(["python", "native", "native", "python"]):
        worker = Worker(commands[name], args.output / f"{index}-{name}.stderr", native=name == "native")
        try:
            actual, first = worker.request(workload[0]["text"])
            cold = (time.perf_counter_ns() - worker.started) / 1e6
            require_correction(name, workload[0], actual)
            for _ in range(args.warmup):
                for case in workload:
                    actual, _ = worker.request(case["text"])
                    require_correction(name, case, actual)
            rows = []
            for _ in range(args.repeats):
                for case in workload:
                    actual, elapsed = worker.request(case["text"])
                    require_correction(name, case, actual)
                    rows.append(dict(id=case["id"], elapsed_ms=elapsed))
            rss = worker.close()
        except BaseException:
            worker.abort()
            raise
        runs.append(dict(runtime=name, first_request_ms=first, fresh_process_first_result_ms=cold,
                         max_rss_bytes=rss, samples=rows))
        print(f"Completed {index + 1}/4: {name}", flush=True)
    summary = {}
    for name in commands:
        relevant = [run for run in runs if run["runtime"] == name]
        summary[name] = dict(
            median_ms=statistics.median(row["elapsed_ms"] for run in relevant for row in run["samples"]),
            first_result_ms=[r["fresh_process_first_result_ms"] for r in relevant],
            max_rss_bytes=[r["max_rss_bytes"] for r in relevant],
            cases={case["id"]: statistics.median(row["elapsed_ms"] for run in relevant
                   for row in run["samples"] if row["id"] == case["id"]) for case in workload})
    sources = ["src/liquid_runtime.rs", "src/liquid_process.rs", "src/liquid_text.rs", "build_support/liquid_manifest.rs",
               "build_support/payload_codec.rs", "Cargo.lock", "scripts/liquid-grammar-worker.py",
               "scripts/grammar_compare/models.py", "scripts/liquid-grammar-requirements.txt",
               "scripts/liquid-native-probe/Cargo.lock", "scripts/liquid-native-probe/src/main.rs",
               "src/liquid_notices.rs", "src/liquid_resources.rs", "resources/liquid/manifest.json",
               "resources/liquid/notices.json", "scripts/compare-liquid-native.py"]
    report = dict(schema_version=2, platform=platform.platform(), machine=platform.machine(),
                  cpu=command_output(["sysctl", "-n", "machdep.cpu.brand_string"]) if sys.platform == "darwin" else None,
                  rustc=command_output(["rustc", "--version"]),
                  native_binary_sha256=sha(args.native_probe),
                  native_cache_preflight=preflight, native_network_setup_excluded=True,
                  native_binary_architecture=command_output(["file", "-b", args.native_probe]),
                  native_model_already_expanded=True,
                  python=command_output([args.python, "--version"]),
                  threads=args.threads, arithmetic="float32", quantized=False,
                  order=[r["runtime"] for r in runs], warmup_rounds=args.warmup,
                  measured_rounds_per_process=args.repeats, fresh_processes_per_runtime=2,
                  filesystem_cache="warm; process-cold results do not imply storage-cold startup",
                  workload=[dict(id=c["id"], text=c["text"],
                                 input_tokens=len(c["reference_passes"][0]["input_ids"])) for c in workload],
                  source_sha256={p: sha(ROOT / p) for p in sources},
                  fixture_sha256=sha(FIXTURES), summary=summary, runs=runs,
                  caller_thread="One persistent JSONL caller thread per process; application job dispatch is not measured; the persistent native helper owns its inference thread",
                  scope="Verified prepared native cache: model, tokenizer, current-architecture runtime and notices; network setup excluded. Actual production native Client/tokenizer/ONNX Runtime helper vs pinned Python JSONL worker; serial CPU, request round trip. No application job dispatch, GUI, source extraction or Apply timing. Native runs have no Python on PATH.")
    (args.output / "benchmark.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-probe", type=Path, required=True)
    parser.add_argument("--native-cache", type=Path, required=True)
    parser.add_argument("--python", type=Path)
    parser.add_argument("--python-cache", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=4, choices=range(1, 9))
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--benchmark", action="store_true")
    args = parser.parse_args()
    if args.warmup < 0 or args.repeats < 1:
        parser.error("Use nonnegative warmup and positive repeats")
    if not (args.verify or args.benchmark):
        parser.error("Choose --verify and/or --benchmark")
    args.output.mkdir(parents=True, exist_ok=True)
    if args.verify: verify(args)
    if args.benchmark: benchmark(args)


if __name__ == "__main__":
    main()
