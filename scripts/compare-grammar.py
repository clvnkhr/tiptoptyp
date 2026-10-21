#!/usr/bin/env python3
"""Compare local grammar models on identical mathematical prose, without the app."""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

from grammar_compare.comparison import load_corpus, save_report

HERE = Path(__file__).resolve().parent
MODEL_NAMES = ("liquid", "gector", "coedit")


def positive(value: str) -> int:
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("Must be at least 1.")
    return number


def nonnegative(value: str) -> int:
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("Must be zero or greater.")
    return number


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=HERE / "grammar_compare/corpus.json")
    parser.add_argument("--models", choices=MODEL_NAMES, nargs="+", default=list(MODEL_NAMES))
    parser.add_argument("--output", type=Path, default=HERE.parent / ".tiptoptyp/grammar-comparison" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    parser.add_argument("--cache-dir", type=Path, default=HERE.parent / ".tiptoptyp/grammar-comparison/models")
    parser.add_argument("--threads", type=positive, default=4)
    parser.add_argument("--repeats", type=positive, default=3)
    parser.add_argument("--warmup", type=nonnegative, default=2)
    parser.add_argument("--timeout", type=positive, default=1800, help="Maximum seconds per model, including loading.")
    parser.add_argument("--limit", type=positive)
    parser.add_argument("--case", action="append", default=[], help="Case ID, repeatable.")
    parser.add_argument("--category", action="append", default=[], help="Category, repeatable.")
    parser.add_argument("--validate", action="store_true", help="Validate selected fixtures without loading ML dependencies.")
    parser.add_argument("--report-only", type=Path, help="Recreate HTML from a completed results.json without ML dependencies.")
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = arguments(argv)
    if args.report_only:
        data = json.loads(args.report_only.read_text(encoding="utf-8"))
        if data.get("version") != 1:
            raise ValueError("Unsupported results version; expected version 1.")
        save_report(data, args.output)
        print(f"Report: {args.output.resolve() / 'report.html'}")
        return 0
    corpus, digest = load_corpus(args.corpus)
    cases = corpus["cases"]
    for key, requested in (("id", args.case), ("category", args.category)):
        unknown = set(requested) - {case[key] for case in cases}
        if unknown:
            raise ValueError(f"Unknown {key}: {', '.join(sorted(unknown))}")
        if requested:
            cases = [case for case in cases if case[key] in requested]
    if args.limit:
        cases = cases[:args.limit]
    if not cases:
        raise ValueError("Selection contains no cases.")
    corpus = {**corpus, "cases": cases}
    print(f"Validated {len(cases)} cases: {dict(Counter(case['format'] for case in cases))}", flush=True)
    if args.validate:
        return 0
    args.output.mkdir(parents=True, exist_ok=True)
    worker_dir = args.output / "per-worker"
    worker_dir.mkdir(exist_ok=True)
    processor = platform.processor()
    if sys.platform == "darwin":
        try:
            processor = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True, stderr=subprocess.DEVNULL, timeout=2).strip()
        except (OSError, subprocess.SubprocessError):
            pass
    metadata = {
        "version": 1, "created_utc": datetime.now(timezone.utc).isoformat(),
        "corpus": corpus, "corpus_sha256": digest,
        "selection": {"ids": [case["id"] for case in cases]},
        "runtime": {"platform": platform.platform(), "machine": platform.machine(),
                    "python": platform.python_version(), "processor": processor, "logical_cpus": os.cpu_count(),
                    "device": "cpu", "threads": args.threads,
                    "repeats": args.repeats, "warmup": args.warmup},
        "scoring": "Exact target with pre-reviewed alternatives; Unicode NFC and whitespace normalisation only. Research passages without references are unscored and excluded from quality denominators. Not ERRANT precision/recall.",
        "harness_sha256": {
            str(path.relative_to(HERE)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in [Path(__file__).resolve(), *sorted((HERE / "grammar_compare").glob("*.py")), HERE / "grammar_compare/requirements.txt"]
        },
        "models": [],
    }
    try:
        metadata["git_revision"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE.parent, text=True, stderr=subprocess.DEVNULL).strip()
        metadata["git_dirty"] = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=HERE.parent, text=True))
    except (OSError, subprocess.CalledProcessError):
        metadata["git_revision"] = None
    environment = {**os.environ, "PYTHONHASHSEED": "0", "TOKENIZERS_PARALLELISM": "false", "OMP_NUM_THREADS": str(args.threads), "MKL_NUM_THREADS": str(args.threads), "HF_HUB_DISABLE_TELEMETRY": "1"}
    failed = False
    for name in dict.fromkeys(args.models):
        request = {"model": name, "cache_dir": str(args.cache_dir.resolve()), "threads": args.threads, "repeats": args.repeats, "warmup": args.warmup, "cases": cases}
        request_path, result_path = worker_dir / f"{name}-request.json", worker_dir / f"{name}.json"
        request_path.write_text(json.dumps(request, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        # Never reuse a previous worker result after a failed launch.
        result_path.unlink(missing_ok=True)
        command = [sys.executable, "-m", "grammar_compare.worker", str(request_path.resolve()), str(result_path.resolve())]
        started = time.perf_counter()
        print(f"Loading {name}; inference uses CPU with {args.threads} threads.", flush=True)
        error = None
        try:
            completed = subprocess.run(command, cwd=HERE, env=environment, timeout=args.timeout, check=False)
            if completed.returncode:
                error = f"Worker exited with status {completed.returncode}."
        except subprocess.TimeoutExpired:
            error = f"Worker exceeded {args.timeout} seconds; partial results retained."
        except OSError as exc:
            error = str(exc)
        result = {"name": name, "results": []}
        if result_path.exists():
            try:
                candidate = json.loads(result_path.read_text(encoding="utf-8"))
                rows = candidate.get("results") if isinstance(candidate, dict) else None
                if not isinstance(rows, list) or any(not isinstance(row, dict) or row.get("id") not in {case["id"] for case in cases} for row in rows):
                    raise ValueError("Invalid worker result structure.")
                if len({row["id"] for row in rows}) != len(rows):
                    raise ValueError("Worker returned duplicate case IDs.")
                result = candidate
                result["name"] = name
            except (ValueError, OSError) as exc:
                error = f"Cannot read worker result: {exc}"
        if len(result["results"]) != len(cases):
            error = error or "Worker did not evaluate every selected case."
        result["worker_wall_ms"] = (time.perf_counter() - started) * 1000
        if error:
            result.setdefault("error", error)
        failed = failed or bool(result.get("error")) or any(row.get("error") for row in result["results"])
        metadata["models"].append(result)
        save_report(metadata, args.output)
        score = result["summary"]
        failed = failed or score["errors"] > 0
        print(f"{name}: exact fixes {score['exact_corrections']}/{score['error_cases']}; correct sentences changed {score['changed_controls']}/{score['correct_controls']}; protected changes {score['unsafe']}; unscored {score['unscored_evaluated']}; unavailable {score['errors']}", flush=True)
    print(f"Report: {args.output.resolve() / 'report.html'}")
    print(f"Raw results: {args.output.resolve() / 'results.json'}")
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, OSError) as error:
        print(f"grammar comparison: {error}", file=sys.stderr)
        raise SystemExit(2)
