import json
from pathlib import Path
import re

BASE = Path(__file__).resolve().parent


def cpu(value):
    parts = value.split(":")
    return sum(float(part) * 60 ** i for i, part in enumerate(reversed(parts)))


results = []
for directory in sorted(BASE.glob("20*-*")):
    metadata = json.loads((directory / "metadata.json").read_text())
    if metadata.get("exit_code") != 0 or not (directory / "summary.json").exists():
        continue
    summary = json.loads((directory / "summary.json").read_text())
    footprint = json.loads((directory / "footprint.json").read_text())["stdout"]
    memory = [(name, int(pid), int(size)) for name, pid, size in
              re.findall(r"([^\n]+) \[(\d+)\]: 64-bit\s+Footprint: (\d+) B", footprint)]
    start = {row["pid"]: row for row in json.loads((directory / "cpu-start.json").read_text())}
    end = {row["pid"]: row for row in json.loads((directory / "cpu-end.json").read_text())}
    interval = json.loads((directory / "cpu-interval.json").read_text())["seconds"]
    deltas = {pid: cpu(end[pid]["cpu_time"]) - cpu(row["cpu_time"]) for pid, row in start.items() if pid in end}
    scopes = {scope["name"]: scope for scope in summary["scopes"]}
    app_memory = next(size for name, pid, size in memory if pid == metadata["pid"])
    row = {"scenario": metadata["scenario"], "directory": directory.name,
           "complete": summary["complete"], "exit": metadata.get("exit_code"),
           "app_mib": round(app_memory / 2**20, 1),
           "direct_family_mib": round(sum(size for _, _, size in memory) / 2**20, 1),
           "direct_processes": len(memory),
           "cpu_interval_seconds": round(interval, 3),
           "app_cpu_seconds": round(deltas.get(metadata["pid"], 0), 3),
           "direct_family_cpu_seconds": round(sum(deltas.values()), 3),
           "app_cpu_one_core_percent": round(100 * deltas.get(metadata["pid"], 0) / interval, 2),
           "editor_calls": scopes.get("ui.editor.pass", {}).get("calls", 0),
           "editor_mean_us": round(scopes.get("ui.editor.pass", {}).get("mean_us", 0), 1),
           "editor_total_ms": round(scopes.get("ui.editor.pass", {}).get("total_ms", 0), 2),
           "highlight_rebuilds": scopes.get("highlight.rebuild", {}).get("calls", 0),
           "binary_sha256": metadata["binary_sha256"], "memory_processes": memory}
    results.append(row)
(BASE / "results.json").write_text(json.dumps(results, indent=2) + "\n")
for row in results:
    print(json.dumps({k: v for k, v in row.items() if k not in {"memory_processes", "binary_sha256"}}))
