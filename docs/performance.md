# Performance workflow

Performance is a continuing review requirement; see [the working agreement](../AGENTS.md#performance-working-agreement).
Measure before optimizing, preserve comparable evidence, and protect the cause
with deterministic tests. Completing the profiling setup does not mean every
subsystem has been optimized.

For failed tab gestures, use the bounded, opt-in [tab drag diagnostics](tab-drag-debugging.md).

## One-command native runs

Prerequisites: a desktop session, the normal Rust development dependencies,
Poppler (`pdftoppm` on `PATH`), and the pinned sidecars installed by
`cargo xtask fetch-sidecars`. The runner checks prerequisites; it does not download
tools or silently switch to a different sampler when one fails.

```sh
cargo xtask profile --scenario settings
cargo xtask profile --scenario large --sampler none
cargo xtask profile --scenario find --warmup 5 --seconds 30 --skip-build
cargo xtask profile --scenario multi-window --warmup 8 --seconds 8
cargo xtask profile --scenario hover-scroll --warmup 0 --seconds 2 --sampler none
cargo xtask profile --scenario tabs-switch --warmup 0 --seconds 2 --sampler none --skip-build
cargo xtask profile --help
```

The runner builds `target/profiling/tiptoptyp` with release optimizations, debug
symbols, the `profiling` feature, and frame pointers. The first build can take
several minutes. `--skip-build` reuses the binary and records its hash, but cannot
prove that it matches the current source or was built with those flags. Omit it
after code changes. Normal release packaging is unchanged. The Cargo settings
follow the official [custom-profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html)
and rustc's [frame-pointer option](https://doc.rust-lang.org/rustc/codegen-options/index.html#force-frame-pointers).

`--binary PATH` profiles a preserved optimized binary without rebuilding it,
while retaining its actual path and hash in metadata. This allows interleaved
baseline/after repeats without modifying the working tree or overwriting the
current executable. As with `--skip-build`, record that binary's build provenance;
the runner cannot infer it from the current source. Do not run builds or other
profiling sessions concurrently with a measured workload.

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
| `hover-scroll` | Function-hover popup with bounded anchor, settle, wheel-scroll and pointer-away phases |
| `large` | 5,000 Unicode comment lines and 100 definitions, over 300 KB of source; short rendered PDF |
| `tabs` | Three deterministic tabs with the first tab retained as the designated preview |
| `tabs-switch` | Three-tab fixture with bounded pointer clicks on the second, third and first tab |
| `pdf` | A deterministic 40-page A6 PDF opened through the asset-preview path |
| `no-window` | Capture the small document, then close it through the retained-root lifecycle before warmup; no document windows remain |
| `multi-window` | The small isolated document opened in four independent document sessions; three secondary native windows are created once before warmup |

These are **steady-state starting points**, except for the two explicitly named
active scenarios. The idle scenes do not automate typing or scrolling. For an
active workload, choose a longer duration, wait for warmup, perform a repeatable
sequence in the disposable fixture, and record that sequence. Do not mix its
results with idle runs. The large-source case isolates source scaling; it does
not establish preview scaling.
QA helpers remain active (including deterministic font-picker fixture setup);
use the samples to distinguish fixture overhead and confirm suspected production
hot paths in an ordinary app launch before changing them.

For active interaction runs, use `hover-scroll` for hover-then-scroll dismissal
and `tabs-switch` for repeatable tab selection. These scenarios inject only
bounded egui pointer/wheel events after measurement begins; `summary.json`
records phase names, event counts and repaint locations without source contents.
Use `tabs` for manual tab reordering and `pdf` for manual scrolling through the
multi-page asset preview. The active scripts remain separate from idle runs, so
an unattended idle profile still protects idle repaint behavior.

The `no-window` scenario sets `TIPTOPTYP_PROFILE_NO_WINDOW=1` for the profiling
build. It performs one document-close transition after capture, not repeated
synthetic frames. The normal profiling deadline still quits the process.
Compare worker waiting stacks and retained service threads as well as UI spans:
eframe already suppresses editor painting when all native windows are hidden,
so an empty span report alone does not establish that document services stopped.
See [the dormant-host results](multi-window-audit.md#dormant-host-lifecycle-todo-160).

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
  UI, theme, highlighting, search, compile-result handling and worker scopes,
  plus bounded root repaint-request call-site counts.
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

`root_repaint_requests` counts the source file/line locations reported by egui
on measured root passes. It includes delayed requests, not just requests for an
immediate frame; it does not identify every OS-triggered paint. At most 64 call
sites are retained, with excess occurrences counted in
`dropped_repaint_locations`. Free-form repaint reasons are not recorded because
they can contain application text. This helped distinguish an indefinite
preview spinner from normal workspace/file polling while Settings was open.

`secondary_repaint_requests` records the same information across secondary
document passes. Root and secondary counters share the same 64-location bound;
there are no per-window unbounded maps or per-frame log writes. These include
delayed requests, not just immediate redraws. Deferred documents repaint on
their own input, timers and worker notifications; registration in a root pass
does not execute their editor layout. The native event loop and native webviews
remain on the UI thread. See [multi-window results](multi-window-audit.md#independent-document-repaints-todo-174).

Settings uses an independently repainted native viewport. Scroll, search, hints,
and font-preview completion stay local; preference edits, app commands, native
close and focus ownership notify the editor. Parent snapshots invalidate Settings
only on actual presentation changes, and font catalogs are copied only when
their revision changes or the window reopens. Rapid preference edits coalesce
and merge only changed fields, preserving newer history and sibling preferences.
The native preview's wait for document-window activation is static: it must not
animate forever behind Settings. Real loading retains its progress indicator.

```sh
cargo test settings_window
cargo test preview_focus_wait
```

These tests exercise independent child/parent passes and wheel input, semantic
search/edit controls, preference delivery, and the non-animating wait state.
Native idle samples measure eliminated coupled work; they are not end-to-end
scroll-FPS benchmarks.

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

## Architecture regression probes

### Integrated endpoint evidence — 2026-09-20

The baseline rows below were collected on Apple arm64 macOS with the same
optimized profiling binary (`target/profiling/tiptoptyp`, SHA-256
`260b0ea36dd5763df5a5e4f774ab259027998c014473bb44327be5f4b24c7a0b`), the
Catppuccin Latte theme, isolated fixtures, and sampler `none` unless noted. The
repaired hover and tabs rows use the current optimized binary, SHA-256
`828d3d52a7a28fce63f1f32c3aac7f337d1d73445ac7bdbbd7f7aa650541f78c`; they are
endpoint validation, not a before/after performance comparison.
The active input rows use the rebuilt optimized binary, SHA-256
`ed4af77288658063bdf11ed4369b0bd016cb28ab3321505d93b4e3943f328be0`.
The retained directories are Git-ignored and contain the exact metadata,
summary, logs, CPU readings, and initial viewport framebuffer.

| Workload | Artifact | Warmup / measure | Result |
| --- | --- | ---: | --- |
| one-window idle | `.tiptoptyp/profiles/1789891323533-63744-main-0` | 5s / 5s | Complete; no instrumented spans or repaint requests |
| four-owner idle | `.tiptoptyp/profiles/1789891342485-64050-multi-window-0` | 5s / 5s | Complete; no secondary repaint requests or instrumented spans |
| Settings startup/interaction window | `.tiptoptyp/profiles/1789891263205-61209-settings-0` | 0s / 1s | Complete; 8 Settings/UI calls and 101 spinner repaint requests during active startup |
| Settings settled | `.tiptoptyp/profiles/1789891583728-65502-settings-0` | 5s / 5s | Complete; settled idle had no instrumented spans or repaint requests |
| Settings CPU sample | `.tiptoptyp/profiles/1789891627456-66109-settings-0` | 1s / 2s | Complete; `/usr/bin/sample` artifact retained, no wall-time scope calls |
| 40-page PDF residency | `.tiptoptyp/profiles/1789891570117-65164-pdf-0` | 0s / 1s | Complete; startup/raster admission captured `ui.editor.pass` and a bounded repaint |
| deterministic function tooltip | `.tiptoptyp/profiles/1789894830846-81006-hover-0` | 0s / 1s | Complete after item 259; four diagnostic child paints and bounded root repaint requests |
| deterministic three-tab workload | `.tiptoptyp/profiles/1789894840618-81492-tabs-0` | 0s / 1s | Complete after item 259; one bounded root repaint request and clean deadline shutdown |
| active hover then scroll | `.tiptoptyp/profiles/1789895984635-91521-hover-scroll-0` | 0s / 2s | Complete after item 260; four phases and 68 injected events |
| active tab selection | `.tiptoptyp/profiles/1789895996450-91995-tabs-switch-0` | 0s / 2s | Complete after item 260; seven phases and 10 injected events |

Earlier attempts remain retained as invalid history: hover
(`1789891376923-64483-hover-0`) never reached readiness, while tabs
(`1789891511578-64829-tabs-0`) reached measurement but exceeded shutdown. Item
259 fixed both causes: deterministic tooltip scenes now survive child-view
lifecycle cleanup, and the profiler hands its deadline close back to the event
loop and outer screenshot wrapper. The current endpoint rows above are valid
captures. The active rows are deliberately not comparable to the idle rows:
they request frames during a bounded scripted interaction and should only be
compared with repeated runs using the same phase schedule.
Native sampling can perturb that schedule: a 0s-warmup/2s `hover-scroll` run
with `/usr/bin/sample` completed, but recorded only its initial anchor phase.
Use sampler-free runs when validating that every active phase executes; retain
sampler-backed runs for stack evidence and process-resource snapshots.

The `cpu-before.txt`/`cpu-after.txt` values are process snapshots around the
measurement window, not a portable CPU score; the successful runs used different
startup/cache phases and must not be ranked by those values. Unix runs also keep
`resources-before.txt`/`resources-after.txt`, listing the profiler-owned process
group's PID, RSS, VSZ, CPU and elapsed time; these are residency clues, not
allocator or GPU measurements. No GPU memory or texture-residency accounting is
claimed. Future before/after work must preserve the same binary provenance,
fixture, warmup, interaction sequence and sampler. The remaining architecture
work is therefore documented as evidence collection with known gates, not a
claim that the refactor improved runtime performance.

The [background-save review](background-saves.md) records matched optimized
synchronous/background dispatch measurements and the opt-in slow-storage probe.
It separates foreground submission latency from total persistence time.

See [the architecture performance review](architecture-performance.md) for
matched baseline/fixed measurements, retained evidence, limitations, and the
opt-in `architecture_cost_probe` command. Normal test runs skip its timing loop;
deterministic tests protect bounded normalization work and borrowed wire payloads.

The [next architecture batch](architecture-followup.md) records the completion
popup payload allocation comparison and its opt-in reproduction command. It
isolates item-list copying, not whole-frame or GPU costs.

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
