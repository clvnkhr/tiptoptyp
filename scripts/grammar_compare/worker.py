"""One model per process; runtime allocations never carry between models."""
from __future__ import annotations

import argparse
import importlib.metadata
import json
from pathlib import Path
import platform
import resource
import statistics
import sys
import time
import traceback


def rss_mib() -> float:
    # Darwin reports bytes; other supported Unix runtimes report KiB.
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return round(peak / (1024 * 1024 if sys.platform == "darwin" else 1024), 1)


def run(request: dict, destination: Path) -> dict:
    result = {"name": request["model"], "results": []}
    started = time.perf_counter()

    def checkpoint():
        result["peak_rss_mib"] = rss_mib()
        destination.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    try:
        from .models import MODEL_SPECS, load_model
        import torch
        torch.manual_seed(0)
        torch.set_num_threads(request["threads"])
        torch.set_num_interop_threads(1)
        result["import_ms"] = (time.perf_counter() - started) * 1000
        spec = MODEL_SPECS[request["model"]]
        result["label"] = spec.label
        loaded = time.perf_counter()
        adapter = load_model(request["model"], Path(request["cache_dir"]), threads=request["threads"])
        result["load_ms"] = (time.perf_counter() - loaded) * 1000
        result["metadata"] = {
            **adapter.metadata,
            "python": platform.python_version(), "torch": torch.__version__,
            "transformers": importlib.metadata.version("transformers"),
            "warmup_requests": request["warmup"], "repeats": request["repeats"],
            "seed": 0, "cache_dir": request["cache_dir"],
        }
        result["warmup_ms"] = []
        for _ in range(request["warmup"]):
            before = time.perf_counter()
            adapter.correct("These estimates holds for every bounded operator.")
            result["warmup_ms"].append((time.perf_counter() - before) * 1000)
        checkpoint()
        for index, case in enumerate(request["cases"], 1):
            row = {"id": case["id"]}
            try:
                outputs, samples = [], []
                for _ in range(request["repeats"]):
                    before = time.perf_counter()
                    output = adapter.correct(case["text"])
                    samples.append((time.perf_counter() - before) * 1000)
                    if not isinstance(output, str) or not output.strip():
                        raise ValueError("Adapter returned an empty or non-text correction.")
                    outputs.append(output)
                row.update(output=outputs[0], samples_ms=samples, latency_ms=statistics.median(samples), distinct_outputs=list(dict.fromkeys(outputs)))
            except Exception as error:
                row["error"] = f"{type(error).__name__}: {error}"
            result["results"].append(row)
            checkpoint()
            print(f"{request['model']}: {index}/{len(request['cases'])} {case['id']}", flush=True)
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        traceback.print_exc()
    checkpoint()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("request", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    request = json.loads(args.request.read_text(encoding="utf-8"))
    result = run(request, args.destination)
    return 1 if result.get("error") or any(row.get("error") for row in result["results"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
