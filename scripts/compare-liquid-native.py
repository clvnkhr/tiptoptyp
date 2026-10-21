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
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "assets/liquid/correction-cases.json"


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
    return [args.native_probe, "--cache-dir", args.native_cache, "--threads", str(args.threads), "--resources", args.native_resources]


def verify(args):
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
    report = dict(schema_version=1, driver_architecture=platform.machine(),
                  native_binary_architecture=command_output(["file", "-b", args.native_probe]),
                  native_python_path_disabled=True,
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
               "scripts/liquid-native-probe/Cargo.lock", "scripts/compare-liquid-native.py"]
    model_root = args.native_cache / "onnxruntime/lfm25-encoder-350m-onnxruntime-v1"
    model_hashes = {name: sha(model_root / name) for name in ["manifest.json", "liquid.onnx", "tokenizer.json"]}
    for asset in json.loads((model_root / "manifest.json").read_text())["files"]:
        if asset.get("target") == ("aarch64-apple-darwin" if platform.machine() == "arm64" else "x86_64-apple-darwin"):
            relative = str(Path("runtime") / asset["target"] / asset["name"])
            model_hashes[relative] = sha(model_root / relative)
    report = dict(schema_version=1, platform=platform.platform(), machine=platform.machine(),
                  cpu=command_output(["sysctl", "-n", "machdep.cpu.brand_string"]) if sys.platform == "darwin" else None,
                  rustc=command_output(["rustc", "--version"]),
                  native_binary_sha256=sha(args.native_probe), native_cache_sha256=model_hashes,
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
                  scope="Actual production native Client/tokenizer/ONNX Runtime helper vs pinned Python JSONL worker; serial CPU, request round trip. No application job dispatch, GUI, source extraction or Apply timing. Native runs have no Python on PATH.")
    (args.output / "benchmark.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-probe", type=Path, required=True)
    parser.add_argument("--native-cache", type=Path, required=True)
    parser.add_argument("--native-resources", type=Path, default=ROOT / "toolchain")
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
