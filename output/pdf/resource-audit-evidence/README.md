# Resource audit evidence - 17 September 2026

The report is `../resource-audit.typ`; its rendered companion is
`../resource-audit.pdf`. This is audit evidence, not a change to app behavior.

## Reproduce

From the repository root:

```sh
cargo build --locked --profile profiling --features profiling --bin tiptoptyp --config 'build.rustflags=["-C", "force-frame-pointers=yes"]'
python3 output/pdf/resource-audit-evidence/collect.py main
python3 output/pdf/resource-audit-evidence/summarize.py
```

Other scenarios: `settings`, `multi-window`, `no-window`, `large`, `pdf-1`,
`pdf-20`. Each run creates a new timestamped directory and isolated workspace.
The page-count scenarios open generated Typst source, not a PDF asset tab.
Two earlier direct-PDF attempts timed out before capture-ready; they are
retained but excluded by the summarizer. There was one successful trial per
scenario. These are current-state observations, not before/after benchmarks.

## Contents

- `metadata.json`: command, fixture/binary/source/toolchain hashes, Git status,
  environment and completion status.
- `footprint.json`, `vmmap.json`: raw macOS memory snapshots.
- `family-start.json`, `cpu-start.json`, `cpu-end.json`: app/descendant process
  snapshots. CPU deltas use only processes present at both boundaries.
- `cpu-interval.json`, `sampler.json`, `cpu.sample.txt`: native sample and its
  CPU-delta bracket. This sample perturbs execution.
- `unattributed-new-webkit.json`: candidates only, **not app-attributed memory**.
- `summary.json`: app profiling scopes and repaint locations over ten seconds.
  Inclusive nested wall-time scopes must not be summed as CPU time.
- `workspace/`: generated fixture and fresh viewport captures. Native WebKit
  child surfaces are not composited into these captures.
- `results.json`: derived table of completed scenarios, including raw footprint
  process entries and binary hashes.

The capture warmup exercises raster preview; this is not a clean measurement
of interactive-preview-only production use. Whole-family WebKit attribution,
GPU execution/allocations, energy, active interaction and peak compilation
memory remain unmeasured. Other user applications were left running.

## Repository checks at handoff

All completed successfully on the audited checkout:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast --quiet
cargo test --manifest-path xtask/Cargo.toml
```

The normal suite reported ten ignored tests. Its opt-in native window drag
check was skipped; that interaction is unrelated to this report-only change.
No production profiling instrumentation was modified.
