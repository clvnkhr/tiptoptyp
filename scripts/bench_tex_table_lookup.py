#!/usr/bin/env python3
"""Compare historical and current production TeX table lookup components.

Run on a quiet host, without another build or profiler:
  python3 scripts/bench_tex_table_lookup.py
  python3 scripts/bench_tex_table_lookup.py --before-ref 585b42d --after-ref HEAD

Requires the repository's pinned Rust toolchain and local Git history. It does
not invoke Cargo, download tools, launch the app, or read application settings.
The default output is a new, ignored .tiptoptyp/profiles/tex-table-lookup-* folder.
An explicit --output must not exist, including symlinks. Failed runs retain their
artifacts and are never reported as complete. See report.json for exact sources,
compiler flags, fixtures, caret coordinates, binaries and interleaved samples.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = Path(__file__).with_name("benchmarks") / "tex_table_lookup.rs"
DEFAULT_BEFORE = "585b42d"
SOURCES = (
    "src/editor_features/tex_table.rs",
    "src/editor_features.rs",
    "src/editor_math_guides.rs",
    "src/editor_data.rs",
)
CASES = (
    "no-table-1MiB",
    "inside-one-table-1MiB",
    "many-sibling-tables",
    "unsupported-inner-table",
    "stable-caret-control",
)
SCOPE = (
    "Actual production TeX table scanner/index/cache and lossless parser, with a "
    "small last-caret adapter matching EditorDerivedData's existing fast path. "
    "The scalar byte map is prepared outside timing, as the editor already retains it. "
    "Warm caret lookup and cold index plus first-target costs are reported separately."
)
LIMITATIONS = (
    "Local optimized component measurements, not whole-editor, keystroke, compiler, "
    "provider, layout, painting, startup, allocation or native-input latency. "
    "No wall-time CI threshold or cross-platform guarantee. Other host activity is "
    "uncontrolled; do not run concurrently with a build or another profiler. "
    "Stable-caret timings near clock/observer overhead are a control, not a speed claim."
)


class BenchmarkError(RuntimeError):
    pass


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def command(args, *, cwd=None) -> str:
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, check=False)
    if result.returncode:
        raise BenchmarkError(f"Command failed ({result.returncode}): {args[0]}\n{result.stderr.strip()}")
    return result.stdout.strip()


def git(root: Path, *args: str) -> str:
    return command(["git", "-C", str(root), *args])


def revision(root: Path, reference: str) -> str:
    return git(root, "rev-parse", "--verify", "--end-of-options", reference + "^{commit}")


def output_directory(root: Path, requested: str | None) -> Path:
    if requested is not None:
        path = Path(requested).expanduser().absolute()
        if os.path.lexists(path):
            raise BenchmarkError(f"Output already exists; refusing to overwrite: {path}")
        path = path.resolve()
        path.mkdir(mode=0o700, parents=True, exist_ok=False)
        return path
    parent = root / ".tiptoptyp" / "profiles"
    parent.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="tex-table-lookup-", dir=parent)).resolve()


def write_new(path: Path, data: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(data)


def write_json(path: Path, value) -> None:
    write_new(path, (json.dumps(value, indent=2, allow_nan=False) + "\n").encode())


def extract(source: str, start: str, end: str, label: str) -> str:
    try:
        left = source.index(start)
        right = source.index(end, left)
    except ValueError as error:
        raise BenchmarkError(f"{label}: source contract changed; review the benchmark adapter") from error
    return source[left:right]


def helpers(features: str, guides: str) -> tuple[str, str]:
    try:
        at = features.index("pub(crate) struct SourceEdit {")
        start = features.rindex("#[derive", 0, at)
        end = features.index("\n}", at) + 2
        edit = features[start:end]
        conversions = extract(features, "fn char_to_byte(", "\nstruct LineMap", "UTF-8 conversions")
        at = guides.index("pub(crate) struct TexInlineLiteral {")
        start = guides.rindex("#[derive", 0, at)
        end = guides.index("\nfn tex_blocks(", at)
    except ValueError as error:
        raise BenchmarkError("Source helpers changed; review the benchmark adapter") from error
    return edit + "\n" + conversions + "\n", guides[start:end] + "\n"


def capture(root: Path, output: Path, label: str, reference: str | None) -> dict:
    destination = output / label
    destination.mkdir()
    head = revision(root, reference or "HEAD")
    files = {}
    for name in SOURCES:
        if reference is None:
            data = (root / name).read_bytes()
        else:
            result = subprocess.run(["git", "-C", str(root), "show", f"{head}:{name}"],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            if result.returncode:
                raise BenchmarkError(f"Cannot capture {label} {name} at {head}: {result.stderr.decode(errors='replace').strip()}")
            data = result.stdout
        files[name] = data
    if reference is None:
        if head != revision(root, "HEAD") or any((root / name).read_bytes() != data for name, data in files.items()):
            raise BenchmarkError("Working-tree sources changed during capture; retry on a stable tree")
    for name, data in files.items():
        path = destination / "sources" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        write_new(path, data)
    feature_helpers, inline = helpers(files["src/editor_features.rs"].decode(), files["src/editor_math_guides.rs"].decode())
    write_new(destination / "helpers.rs", feature_helpers.encode())
    write_new(destination / "inline.rs", inline.encode())
    info = dict(label=label, selector=reference or "working-tree", revision=head,
                source_sha256={name: digest(data) for name, data in files.items()},
                helper_sha256=digest(feature_helpers.encode()), inline_sha256=digest(inline.encode()))
    if reference is None:
        info["source_worktree_status"] = git(root, "status", "--porcelain", "--untracked-files=no", "--", *SOURCES)
    write_json(destination / "source.json", info)
    return info


def rust_string(value: str) -> str:
    escapes = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
    return '"' + "".join(escapes.get(char, f"\\u{{{ord(char):x}}}" if ord(char) < 32 or ord(char) == 127 else char) for char in value) + '"'


def adapter(output: Path) -> Path:
    template = TEMPLATE.read_text()
    substitutions = {
        "__BEFORE_HELPERS__": output / "before/helpers.rs",
        "__BEFORE_TABLE__": output / "before/sources/src/editor_features/tex_table.rs",
        "__INLINE__": output / "before/inline.rs",
        "__AFTER_HELPERS__": output / "after/helpers.rs",
        "__AFTER_TABLE__": output / "after/sources/src/editor_features/tex_table.rs",
    }
    for marker, path in substitutions.items():
        if template.count(marker) != 1:
            raise BenchmarkError(f"Invalid adapter placeholder: {marker}")
        template = template.replace(marker, rust_string(str(path)))
    path = output / "lookup.rs"
    write_new(path, template.encode())
    return path


def compiler(root: Path, explicit: str | None) -> tuple[str, dict]:
    channel = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    chosen = explicit or os.environ.get("RUSTC")
    if chosen is None:
        rustup = shutil.which("rustup")
        if rustup is None:
            raise BenchmarkError("rustup is unavailable; use --rustc with the repository's pinned compiler")
        chosen = command([rustup, "which", "--toolchain", channel, "rustc"])
    chosen = shutil.which(chosen) or str(Path(chosen).resolve())
    details = command([chosen, "-vV"])
    release = next((line.removeprefix("release: ") for line in details.splitlines() if line.startswith("release: ")), "")
    if release != channel:
        raise BenchmarkError(f"Compiler release {release!r} does not match pinned channel {channel!r}")
    return chosen, dict(path=chosen, pinned_channel=channel, verbose_version=details,
                        executable_sha256=digest(Path(chosen).read_bytes()))


def build(output: Path, label: str, rustc: str, source: Path) -> dict:
    executable = output / (label + ("-probe.exe" if os.name == "nt" else "-probe"))
    args = [rustc, "--edition=2024", "-C", "opt-level=3", "--check-cfg=cfg(indexed_probe)",
            "--check-cfg=cfg(test)", str(source), "-o", str(executable)]
    if label == "after":
        args += ["--cfg", "indexed_probe"]
    result = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    write_new(output / (label + "-compile.log"), result.stdout.encode())
    if result.returncode:
        raise BenchmarkError(f"{label} source compilation failed; see {label}-compile.log")
    return dict(command=args, executable=str(executable), executable_sha256=digest(executable.read_bytes()))


def validate_rows(rows: list, label: str) -> None:
    expected = "uncached" if label == "before" else "indexed"
    if len(rows) != len(CASES) or not all(isinstance(row, dict) for row in rows) or {row.get("case") for row in rows} != set(CASES):
        raise BenchmarkError(f"{label}: incomplete or duplicate benchmark cases")
    for row in rows:
        if row.get("strategy") != expected or row.get("golden_checks") != row.get("carets_per_batch") or row.get("golden_checks", 0) <= 0:
            raise BenchmarkError(f"{label}: strategy/golden-validation contract failed")
        for name in ("warm_samples_ns_per_query", "cold_samples_ns"):
            values = row.get(name)
            if not isinstance(values, list) or len(values) != 9 or not all(isinstance(value, (int, float)) and math.isfinite(value) and value >= 0 for value in values):
                raise BenchmarkError(f"{label}: invalid {name}")
        for name in ("warm_median_ns", "warm_p90_ns", "cold_index_and_first_target_median_ns", "cold_p90_ns"):
            value = row.get(name)
            if not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                raise BenchmarkError(f"{label}: invalid {name}")
        warm = sorted(row["warm_samples_ns_per_query"])
        cold = sorted(row["cold_samples_ns"])
        if (row["warm_median_ns"], row["warm_p90_ns"], row["cold_index_and_first_target_median_ns"], row["cold_p90_ns"]) != (warm[4], warm[8], cold[4], cold[8]):
            raise BenchmarkError(f"{label}: summary disagrees with raw batch samples")


def run_probe(output: Path, round_number: int, label: str, executable: str) -> dict:
    prefix = output / "runs" / f"round-{round_number:02d}-{label}"
    artifacts = prefix.with_suffix(".artifacts")
    result = subprocess.run([executable, str(artifacts)], text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, check=False)
    write_new(prefix.with_suffix(".jsonl"), result.stdout.encode())
    write_new(prefix.with_suffix(".stderr.log"), result.stderr.encode())
    if result.returncode:
        raise BenchmarkError(f"{label} round {round_number} failed, including its untimed golden checks; artifacts retained")
    try:
        rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    except json.JSONDecodeError as error:
        raise BenchmarkError(f"{label} round {round_number}: malformed measurement output") from error
    validate_rows(rows, label)
    manifests = {}
    for case in CASES:
        source = artifacts / (case + ".tex")
        carets = artifacts / (case + ".carets.json")
        positions = json.loads(carets.read_text())
        row = next(row for row in rows if row["case"] == case)
        source_bytes = source.read_bytes()
        text = source_bytes.decode()
        if len(source_bytes) != row["source_bytes"] or len(text) != row["source_chars"] or not isinstance(positions, list) or len(positions) != row["carets_per_batch"]:
            raise BenchmarkError(f"{case}: fixture/caret metadata disagrees with measurement")
        if not all(isinstance(position, list) and len(position) == 2 and all(type(value) is int for value in position)
                   and 0 <= position[0] <= len(text) and 0 <= position[1] <= len(source_bytes) for position in positions):
            raise BenchmarkError(f"{case}: invalid caret coordinates")
        manifests[case] = dict(source_sha256=digest(source_bytes),
                               carets_sha256=digest(carets.read_bytes()),
                               source_path=str(source), carets_path=str(carets))
    return dict(round=round_number, label=label, rows=rows, fixtures=manifests)


def summarize(runs: list) -> dict:
    summary = {}
    for case in CASES:
        item = {}
        for label in ("before", "after"):
            rows = [row for run in runs if run["label"] == label for row in run["rows"] if row["case"] == case]
            item[label] = dict(warm_median_ns=statistics.median(row["warm_median_ns"] for row in rows),
                               cold_index_and_first_target_median_ns=statistics.median(row["cold_index_and_first_target_median_ns"] for row in rows))
        summary[case] = item
    return summary


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", type=Path, default=ROOT, help="source repository (default: this checkout)")
    parser.add_argument("--before-ref", default=DEFAULT_BEFORE, help="historical commit/ref, available locally")
    parser.add_argument("--after-ref", help="compare a commit/ref instead of the working tree")
    parser.add_argument("--rustc", help="explicit compiler matching rust-toolchain.toml; also honors RUSTC")
    parser.add_argument("--output", help="new output directory; existing paths and symlinks are rejected")
    parser.add_argument("--rounds", type=int, choices=range(1, 10), default=3, metavar="1..9", help="counterbalanced before/after pairs (default: 3)")
    args = parser.parse_args(argv)
    output = None
    report = dict(schema_version=1, status="running", complete=False,
                  started_utc=datetime.now(timezone.utc).isoformat(), scope=SCOPE, limitations=LIMITATIONS,
                  viewport_theme="Not applicable: pure component operation", warmup_batches=3,
                  measured_batches=9, aggregation="Median of per-process batch medians; every raw batch is retained",
                  golden_validation="Complete TexTable Debug representation versus historical production parser, outside timing",
                  rounds=args.rounds, runs=[])
    try:
        root = args.repo.resolve()
        output = output_directory(root, args.output)
        report["output"] = str(output)
        report["host"] = dict(platform=platform.platform(), machine=platform.machine(),
                              processor=platform.processor(), logical_cpus=os.cpu_count(), python=sys.version)
        report["compiler_path"], report["compiler"] = compiler(root, args.rustc)
        report["before"] = capture(root, output, "before", args.before_ref)
        report["after"] = capture(root, output, "after", args.after_ref)
        if report["before"]["inline_sha256"] != report["after"]["inline_sha256"]:
            raise BenchmarkError("Shared literal-reader helper changed; review the adapter before comparing these revisions")
        source = adapter(output)
        report["adapter_sha256"] = digest(source.read_bytes())
        report["template_sha256"] = digest(TEMPLATE.read_bytes())
        report["script_sha256"] = digest(Path(__file__).read_bytes())
        report["binaries"] = {label: build(output, label, report["compiler_path"], source) for label in ("before", "after")}
        (output / "runs").mkdir()
        reference_fixtures = None
        for round_number in range(1, args.rounds + 1):
            labels = ("before", "after") if round_number % 2 else ("after", "before")
            for label in labels:
                run = run_probe(output, round_number, label, report["binaries"][label]["executable"])
                fingerprints = {case: (info["source_sha256"], info["carets_sha256"]) for case, info in run["fixtures"].items()}
                if reference_fixtures is not None and fingerprints != reference_fixtures:
                    raise BenchmarkError("Fixture/caret hashes differ between runs; comparison is invalid")
                reference_fixtures = fingerprints
                report["runs"].append(run)
        report["summary"] = summarize(report["runs"])
        report.update(status="complete", complete=True)
        print(output)
        print(json.dumps(report["summary"], indent=2))
        return 0
    except (BenchmarkError, OSError, UnicodeError, ValueError, KeyError) as error:
        report.update(status="failed", error=str(error))
        print(f"TeX table benchmark failed: {error}", file=sys.stderr)
        if output is not None:
            print(f"Retained artifacts: {output}", file=sys.stderr)
        return 1
    finally:
        if output is not None:
            report["finished_utc"] = datetime.now(timezone.utc).isoformat()
            write_json(output / "report.json", report)


if __name__ == "__main__":
    raise SystemExit(main())
