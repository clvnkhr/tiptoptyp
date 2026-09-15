# Performance workflow

Performance is a continuing review requirement; see [the working agreement](../AGENTS.md#performance-working-agreement).
Measure before optimizing, preserve comparable evidence, and protect the cause
with deterministic tests. Completing the profiling setup does not mean every
subsystem has been optimized.

## One-command native runs

Prerequisites: a desktop session, the normal Rust development dependencies,
Poppler (`pdftoppm` on `PATH`), and the pinned sidecars installed by
`cargo xtask fetch-sidecars`. The runner checks prerequisites; it does not download
tools or silently switch to a different sampler when one fails.

```sh
cargo xtask profile --scenario settings
cargo xtask profile --scenario large --sampler none
cargo xtask profile --scenario find --warmup 5 --seconds 30 --skip-build
cargo xtask profile --help
```

The runner builds `target/profiling/tiptoptyp` with release optimizations, debug
symbols, the `profiling` feature, and frame pointers. The first build can take
several minutes. `--skip-build` reuses the binary and records its hash, but cannot
prove that it matches the current source or was built with those flags. Omit it
after code changes. Normal release packaging is unchanged. The Cargo settings
follow the official [custom-profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html)
and rustc's [frame-pointer option](https://doc.rust-lang.org/rustc/codegen-options/index.html#force-frame-pointers).

Every run creates a fresh, Git-ignored `.tiptoptyp/profiles/<unique-run>/` with an
isolated document/workspace. The existing non-persistent UI QA scenes supply
fixed initial window sizes, Catppuccin Latte, and fixture state; your files and
saved Settings are not used as the workload. Profiling supplies eframe with a
fresh `app-state/` directory, isolating reads as well as writes. Do not resize,
type, or move the pointer over the app during an idle comparison. Record display
scale and any manual interactions alongside the artifacts.

A local `typst.toml` bounds app project discovery. The child process receives a
Git discovery ceiling and no inherited `GIT_*` overrides, so it cannot profile
the surrounding development checkout accidentally. These are non-Git fixtures;
repository status/indexing needs a separate, controlled repository workload.

| Scenario | Initial workload |
| --- | --- |
| `main` | Small document, split editor/preview |
| `settings` (default) | Same document with Settings open |
| `find` | Find/replace controls open |
| `fonts` | Settings font picker open, deterministic sample font |
| `hover` | Function-hover native popup above the editor |
| `large` | 5,000 Unicode comment lines and 100 definitions, over 300 KB of source; short rendered PDF |

These are **steady-state starting points**, not automated typing, scrolling,
startup, or huge-PDF benchmarks. For an active workload, choose a longer duration,
wait for warmup, perform a repeatable sequence in the disposable fixture, and
record that sequence. Do not mix its results with idle runs. The large-source
case isolates source scaling; it does not establish preview scaling.
QA helpers remain active (including deterministic font-picker fixture setup);
use the samples to distinguish fixture overhead and confirm suspected production
hot paths in an ordinary app launch before changing them.

The app finishes its initial QA capture, signals readiness, warms up (default
3 seconds), measures for 10 seconds, and closes after a 2-second grace period.
Initial capture frames are excluded from the timing summary. Warmup is time-based,
not a guarantee that all background startup work has settled; increase it when
checking idle behavior. The sampler attaches after the runner observes readiness
and waits for warmup, so its window
can be slightly offset by polling/attachment overhead. No continuous repaint
or synthetic input is added to make an idle app look busy.
A single sleeping `profiling-deadline` thread wakes the native event loop to
close the run; it does not depend on further UI frames. Ignore that timer's
waiting stack when inspecting application hot paths.

## Artifacts and interpretation

Each run retains:

- `metadata.txt`: revision/dirty status, binary/fixture/sidecar/manifest hashes,
  compiler, build flags, platform, timing parameters and sampler. Skipped builds
  are explicitly labelled; metadata describes the current tree, not a guarantee
  of the reused binary's provenance.
- `summary.json`: bounded, inclusive **wall-time** summaries for instrumented
  UI, theme, highlighting, search, compile-result handling and worker scopes.
- `cpu.sample.txt` on macOS, or `perf.data` when explicitly selecting Linux perf.
- `cpu-before.txt` / `cpu-after.txt`: supplementary `ps` readings on Unix, or an
  explicit unavailable message. These bracket sampling and its analysis, not an
  exact measurement window. `%CPU` is not a portable benchmark score.
- `app.log`, `sampler.log` when sampling, the `ready` marker, and
  `workspace/.tiptoptyp/screenshots/` containing the initial viewport capture.

Logs and source fixtures stay local. Review them before sharing: native samples
and metadata may contain machine paths. The timing recorder itself accepts only
static scope labels and never logs document contents. Captures are viewport
framebuffers, not composed desktop images; see [UI QA](ui-qa-screenshots.md).

`summary.json` has a schema version, `complete`, measured duration, dropped-scope
count, and per-scope calls, total milliseconds, mean/max microseconds, and
`p50_upper_us` / `p95_upper_us`. Percentiles are **upper bounds from power-of-two
nanosecond buckets**, not exact quantiles. Storage is bounded to 64 distinct scope
labels with 65 counters each, independent of frame count; unknown labels beyond
the cap increment `dropped_scopes`. A quiet idle app may have few or no calls for
a scope. Missing work is not a measured zero-duration operation.

Spans crossing measurement boundaries are excluded. Nested scopes overlap:
never sum them as independent costs. `highlight.total` / `highlight.rebuild` and
`search.total` / `search.rebuild` distinguish calls from cache misses.
`worker.job` aggregates different jobs, including blocking waits; it is not a
CPU-time breakdown. The recorder uses a small mutex-protected in-memory
aggregation and writes once on exit. This adds measurement overhead when enabled;
compare like builds and corroborate hot paths with a native sampler. In ordinary
builds spans/ticks compile to no-ops; a profiling build without
`TIPTOPTYP_PROFILE_DIR` does not start a session or take timing samples.

### Native CPU and memory investigation

On macOS `--sampler auto` uses `/usr/bin/sample` at a 1 ms interval. Inspect the
call tree for hot Rust/system stacks, distinguishing event waits from runnable
CPU work. The generated `.dSYM` is kept beside the executable; keep it with that
exact binary for later symbolication. Instruments can attach to the same
symbolized executable for Time Profiler or Allocations investigations.

On Linux, `--sampler perf` runs `perf record -F 99 -g --call-graph fp` against
the owned app PID. Install perf and arrange the host's profiling permissions
yourself; the runner never changes kernel security settings. Inspect results
with `perf report -i <run>/perf.data`; see the
[perf record manual](https://man7.org/linux/man-pages/man1/perf-record.1.html).
On Linux/Windows `auto` selects `none`; `--sampler none` works without a native
CPU sampler and produces wall-time summaries only. Native attachment has been
validated locally on macOS, not Linux/Windows.

App CPU samples do **not** include the separate Typst, Tinymist, Poppler or WebKit
processes. Attach a suitable profiler to those processes when investigating
compiler, server or preview costs. The runner is not a GPU, allocation, memory
leak, or end-to-end compile-latency profiler.

Failures are nonzero and artifacts are retained. Read `app.log` for missing GUI
access/startup failures, `sampler.log` for attachment failures. Readiness and exit
have watchdogs. During a handled error/timeout the runner stops only its owned
app/sampler tree (a dedicated process group on Unix). No existing user app is
terminated and previous evidence is never overwritten or pruned. Abruptly
killing the runner is not a handled shutdown; retain its printed PID to inspect
and stop that run if needed. Profiling sessions close themselves on their normal
deadline.

For a custom fixture or scene, build as above, make a fresh output directory,
and launch the profiling binary with `TIPTOPTYP_PROFILE_DIR` pointing there,
`TIPTOPTYP_PROFILE_WARMUP` (0–300 whole seconds), and
`TIPTOPTYP_PROFILE_SECONDS` (1–300). This enables automatic closing; use a copy of
your document and a `--ui-snapshot-scene` with non-persistent QA settings. Timed
runs use QA close semantics: an ordinary macOS launch hides its last window on
close instead of quitting. For ordinary interactive launches, attach a native
sampler separately and quit manually. The directory must already exist;
existing `ready` or `summary.json` files are rejected. A non-profiling binary
rejects the profiling directory instead of silently producing no data.
An existing `app-state/` directory is also rejected, so earlier preferences
cannot leak into a new profiling session.

## Headless scaling probes

```sh
cargo bench -p tiptoptyp-core --bench document
```

This dependency-free optimized benchmark prints CSV for shared snapshots, no-op
edits and edit/undo cycles at approximately 16 KiB, 256 KiB and 1 MiB. Each probe
has warmup iterations; source generation is outside the measured interval.
`edit_and_undo` reports one complete cycle, not one keystroke. Each case also
checks basic state invariants. This isolates core document operations and does
not measure native UI responsiveness, highlighting, compiling, or painting.

Save output alongside `rustc -vV`, revision, build flags and hardware details;
repeat 3–5 times on a quiet machine and compare medians and scaling trends.
Do not use shared-CI wall-time thresholds as correctness tests. The full test
suite contains cache-reuse and document identity tests; the focused child-window
regression protects idle repaint behavior:

```sh
cargo test persistent_child_settles_and_reapplies_native_theme_after_change_or_reopen
cargo test --features profiling performance::
cargo test --manifest-path xtask/Cargo.toml
```

CI checks both ordinary and profiling-feature code, including deterministic
recorder/runner tests. It does not pretend headless tests replace desktop
measurements. When fixing a performance bug, add a focused invariant test and
record before/after evidence using this workflow. Investigating individual hot
paths remains ongoing work, not an unchecked part of the profiling setup.

## Hover popup probes

```sh
cargo xtask profile --scenario hover --warmup 6 --seconds 6
RUSTFLAGS=-Cforce-frame-pointers=yes cargo test --profile profiling --features profiling --bin tiptoptyp tooltip_document_render_cost_probe -- --nocapture
cargo test tooltip
```

The headless probe compares a complete 121-byte function hover, a short preview
of synthetic 17,470-byte documentation, and that entire document with/without
offscreen-block culling. It reports the first layout and the mean of 20 warm
layouts after three warmups. Fonts and shared syntax databases are initialized
before timing; visible glyph preparation remains part of first-layout cost.
Repeat on a quiet machine. These are CPU-side UI costs, not server latency,
native buffer swaps, or an end-to-end scroll benchmark. No timing threshold is
used in CI; separate tests protect cache reuse, bounded previews, scroll
geometry, and independent native repainting.

Long hover responses are received once from Tinymist, shared without repeated
string copies, and initially measured/rendered only through a UTF-8-safe
600-character preview. Entering or keyboard-focusing the popup reveals the
entire response automatically, with no button, pagination, or viewport resize.
That first full layout still scales with document length. Subsequent paints
skip offscreen Markdown blocks before highlighting/widget creation; content,
width, style, and font changes invalidate cached geometry.

Wheel/trackpad input in the owning viewport dismisses its hover; source offset
changes cover scrollbar and keyboard scrolling too. A stationary pointer stays
disarmed until it moves, preventing immediate reappearance over newly scrolled
text. Scrolling *inside* a native tooltip neither dismisses it nor repaints the
editor. Pointer movement away from the source-to-popup route, leaving the popup,
Escape, and defocus also dismiss it. Hovers retain their configured delay but
have no fade animation or fade setting.
