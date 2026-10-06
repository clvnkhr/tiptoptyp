# Performance workflow

## Historical PDF thumbnail probe (23 September 2026)

Hayro was subsequently removed; see [the PDFium thumbnail consolidation](hayro-removal.md)
for the current implementation and its validation.

On macOS 14.6.1 arm64, the same three-page `manual-tests/typst-preview.typ`
PDF was rendered at a 720-pixel long edge, white background, four consecutive
runs with warm filesystem caches. The old external Poppler executable plus PNG
output took 79.75, 55.69, 55.62 and 56.16 ms. The in-process Hayro path in an
optimized Rust release test took 3.96, 2.24, 2.13 and 2.04 ms. Both produce
508 × 720 pixels; the new path returns RGBA and avoids process/PNG transport.
Thus these are timings of the respective utility operations, not isolated
renderer benchmarks or whole-app latency measurements. The first Hayro run
is cold within the process; later runs warm code/data caches but create a new
PDF and render cache. No desktop viewport/theme applies to this worker probe.

The fixture PDF/hash, baseline metadata, complete after log and bundled-library
audit are retained in `.tiptoptyp/profiles/pdf-utilities-followup/`; the rendered
PNG was inspected. Repeat with `TIPTOPTYP_PDF_PROBE=/absolute/file.pdf cargo test
--release --bin tiptoptyp pdf::probes::thumbnail_probe -- --ignored --nocapture`.
This small document establishes neither large-document throughput nor a
cross-platform performance guarantee. Cancellation remains between pages, not
within an active page render. No extra worker or idle repaint loop was added.

Performance is a continuing review requirement; see [the working agreement](../AGENTS.md#performance-working-agreement).
Measure before optimizing, preserve comparable evidence, and protect the cause
with deterministic tests. Completing the profiling setup does not mean every
subsystem has been optimized.

For failed tab gestures, use the bounded, opt-in [tab drag diagnostics](tab-drag-debugging.md).

## One-command native runs

Prerequisites: a desktop session, the normal Rust development dependencies,
the pinned sidecars installed by
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
The phase clock advances in fixed 64ms logical ticks per delivered root frame,
so a stall cannot create a burst of catch-up events; an incomplete phase remains
visible in the phase ledger instead of being mistaken for a completed workload.
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

`counters` records bounded event counts such as highlight and tooltip cache hits
and misses. Counter labels share the same 64-label bound as spans; excess labels
are reported in `dropped_counters`. These counts describe recorder-admitted
events, not allocations or GPU work.

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

App CPU samples do **not** include the separate Typst, Tinymist or WebKit
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

This dependency-free optimized benchmark prints CSV for clean/dirty status
queries, shared snapshots, no-op edits and edit/undo cycles at approximately
16 KiB, 256 KiB and 1 MiB. The dirty case changes the final character without
changing byte length, so an uncached comparison must scan the whole buffer. Each probe
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

For the editor's diagnostic refresh path:

```sh
cargo test --release --bin tiptoptyp diagnostic_refresh_cost_probe -- --ignored --nocapture
```

This uses 1, 100 and 1,000 synthetic warnings for one existing temporary file.
Each case has ten warmups and five batches of 100 changed-revision refreshes;
fixture generation is outside timing. It measures file targeting and inline
diagnostic aggregation, including filesystem lookups, not provider execution or
painting. Path matching is cached only within a refresh, so later refreshes
still resolve symlinks and missing files afresh. Tests enforce one check per
distinct diagnostic file across both provider lists, including negative matches.

### Editor state and diagnostic refresh — 30 September 2026

On Apple M2 Max / macOS 14.6.1 / Rust 1.98.1, matched release probes compared
`fa80b0d` plus the new probes against the changes recorded in
[the measurement data](performance-results/editor-work-2026-09-30.json).
Preserved old/new executables were run in alternating order for three pairs,
without concurrent builds. The data retains all samples and binary hashes;
raw logs and binaries are in `.tiptoptyp/profiles/editor-work-2026-09-30/`.

| Warm operation | Before | After |
| --- | ---: | ---: |
| Diagnostic refresh, 1 warning | 32.38 µs | 33.13 µs |
| Diagnostic refresh, 100 warnings for one file | 1.575 ms | 0.075 ms |
| Diagnostic refresh, 1,000 warnings for one file | 15.513 ms | 0.430 ms |
| Repeated clean-state query, 1 MiB source | 25.13 µs | <0.001 µs |
| Repeated dirty-state query, same-length edit near EOF | 26.05 µs | <0.001 µs |

Diagnostic medians use the fifteen batch means; document medians use the three
process means. The dirty-state cache is lazy and invalidated on edits, history,
saved-copy changes and representation replacement. Its first read after a
mutation can still compare the full buffer; repeated reads are constant-time.
Read-only kinds still suppress the dirty indicator. There is no new polling,
worker, persistent filesystem cache, dependency, or per-frame log output.

These improvements do not remove the editor's no-op buffer comparison or speed
up every operation. At 1 MiB the control no-op edit was 24.59 → 25.39 µs and
edit/undo was 87.49 → 88.84 µs. The 256 KiB edit/undo control was slower,
10.82 → 12.50 µs, while the 16 KiB control was 0.728 → 0.768 µs. These short
allocation-heavy controls are retained rather than claiming a general typing
speedup. The substantial measured reduction is file targeting during diagnostic
refresh; provider execution, cold startup, painting and end-to-end keystroke
latency are outside these headless probes. Results are local, not cross-platform
guarantees.

Matched native `large` runs (5-second warmup, 5-second measurement, Catppuccin
Latte, 2800 × 1770 framebuffer, macOS CPU sampling) recorded no measured UI
spans or repaint requests in either build. The main-thread samples were event
waits; process CPU advanced 0.00 / 0.01 seconds. Both fresh viewport captures
were inspected. The subsequent native input preflight reported a locked desktop,
so these samples do not establish unlocked-desktop idle behavior or interaction
coverage. Framebuffer captures also do not verify composed native child views.

## Architecture regression probes

### Integrated endpoint evidence — 2026-09-20

The baseline rows below were collected on Apple arm64 macOS with the same
optimized profiling binary (`target/profiling/tiptoptyp`, SHA-256
`260b0ea36dd5763df5a5e4f774ab259027998c014473bb44327be5f4b24c7a0b`), the
Catppuccin Latte theme, isolated fixtures, and sampler `none` unless noted. The
repaired hover and tabs rows use the current optimized binary, SHA-256
`6c65927befcadfea2bc4aa557356babbeec40b8a655a7d2f867dc2bf104d8d7d`; they are
endpoint validation, not a before/after performance comparison.
The active input rows use the rebuilt optimized binary, SHA-256
`9b4ae2ba1c500c5c1c80a8cfb22e0c37858cc118ad167dbdbd7ddc77aecdd742`.
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
| deterministic function tooltip | `.tiptoptyp/profiles/1789897889628-3518-hover-0` | 0s / 1s | Complete after items 229/259; four diagnostic child paints and bounded root repaint requests |
| deterministic three-tab workload | `.tiptoptyp/profiles/1789897920800-3976-tabs-0` | 0s / 1s | Complete after item 259; one bounded root repaint request and clean deadline shutdown |
| active hover then scroll | `.tiptoptyp/profiles/1789897269887-97329-hover-scroll-0` | 0s / 2s | Complete after items 257/260; four phases and 10 injected events; cache counters retained |
| active tab selection | `.tiptoptyp/profiles/1789897355143-97793-tabs-switch-0` | 0s / 2s | Complete after items 257/260; seven phases and 10 injected events; one bounded highlight miss |

Earlier attempts remain retained as invalid history: hover
(`1789891376923-64483-hover-0`) never reached readiness, while tabs
(`1789891511578-64829-tabs-0`) reached measurement but exceeded shutdown. Item
229/259 fixed both causes: deterministic tooltip scenes now survive child-view
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

## TeX editing syntax work

The opt-in `tex_edit_cost_probe` measures 25 single-character edits in the
middle of the same 423,890-byte, 5,000-line TeX document, after one initial
highlight. On this macOS arm64 Apple M2 Max, with the release test profile and
the same target directory, the full-document Syntect pass took 14,502 ms total
(580.090 ms/edit) at `5fcd572`. The incremental line-state cache took 150 ms
total (6.013 ms/edit) on this branch. Run it with
`cargo test --release --bin tiptoptyp tex_edit_cost_probe -- --ignored --nocapture`.
The test has no wall-time assertion; deterministic tests cover reparsing only
until the syntax state rejoins unchanged lines. This isolates highlighting, not
the whole editor frame, font layout, PDF compilation, or cross-machine typing
latency.

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

## Offline handwriting expansion (27 September 2026)

On macOS 14.6.1, Apple M2 Max, rustc 1.98.1, the same release test executable
compared Detypify alone with Detypify plus full Detexify and its calligraphic
shortlist. The fixed input was the two-stroke dagger from the drawing fixture;
no viewport or theme is involved in this worker-only measurement. The isolated
probe ran after builds finished, with one test thread. Initial cold calls took
44.73 ms for Detypify and another 12.52 ms to load/run Detexify. After those warmups,
20 calls each averaged 5.99 ms (baseline) and 7.96 ms (combined), about 1.97 ms extra.
These numbers do not measure end-to-end GUI latency or guarantee other hardware.

Run `cargo test --release --bin tiptoptyp worker_cost_probe -- --ignored --nocapture
--test-threads=1` to reproduce. Logs, fixture metadata and asset hashes are retained
in `.tiptoptyp/profiles/handwriting-full`. All inference stays on the existing
background worker; only one job per window runs, no idle work is added, and
inputs are capped at 4,096 points. The coarse pass uses every accepted sample;
the general fine pass is bounded to 64 classes × 3 templates. Calligraphic
ranking uses the fixed calligraphic subset separately.

The release executable grew from 52,946,272 to 58,296,272 bytes (+5.35 MB), mostly
from the embedded quantized sample set. No new runtime dependency was added.

## Mathematical dictionary (30 September 2026)

The 467-spelling Harper supplement is built once on first use by the background
writing worker, then shared by document parsing and linting. Its word data takes
5,260 bytes. It adds no per-frame work, file reads, worker jobs or idle repaints.

An optimized macOS 14.6.1 arm64 test executable compared the curated dictionary
with the merged dictionary on the same 1,800-byte Typst fixture, parser and
British dialect. After three warmups, five samples per dictionary had medians
of 13.073 ms and 13.080 ms, respectively, measured after other builds/tests
finished. That difference is within local timing noise. Cold dictionary
initialization and end-to-end GUI latency were not measured. The
[run metadata](performance-results/math-dictionary-2026-09-30.json) records the
command, workload, optimized profile and executable/dictionary hashes.

### Editor decorations (2026-09-30)

Decorations now run inside the existing syntax layout cache. The optimized
600-row fixture measured 6,806 us without decorations and 6,939 us with the
default rules over 100 cached calls (about 68–69 us/call); a repeat was
6,831 / 6,925 us. This is no material local cache-hit difference. Matching and
definition extraction happen only on source/configuration changes; guide
painting visits visible rows and visible indentation columns only. This
measurement excludes painting and cold/cache-miss work. Metadata is in
`performance-results/editor-decorations-2026-09-30.json`.


## Live document diff (2026-09-30)

The combined editor compares source in memory rather than starting Git and
writing comparison files for each edit. A same-release-profile macOS 14.6.1
ARM64 probe used 4,000 lines and 24 warmed comparison requests: the existing
Git-process comparison took 354.8 ms total, versus 9.16 ms for the live model
(14.8 vs 0.38 ms per request). The first live request reuses its warmed source;
subsequent requests each change the document. One thousand unchanged model
requests took 87.3 ms and triggered no rebuilds.

This measures the comparison utilities, not rendered typing latency, cold
startup or whole-app CPU use; viewport/theme do not apply. Run metadata,
fixture hash and raw output are retained in
`.tiptoptyp/profiles/full-document-diff/measurement.log`. Repeat with
`cargo test --release --bin tiptoptyp document_diff_measurement -- --ignored --nocapture`.

Regression tests protect bounded comparison work, preservation of every line,
layout reuse on unchanged source and unrelated settings, wrapped alignment,
Unicode source/cursor identity, and invalidation on resized columns. Both
laid-out columns and their logical row groups are cached; geometric gaps do not
insert synthetic text into the document. No extra idle repaint loop was added.
This local result does not establish a cross-platform performance guarantee.

## Resize presentation correction (2026-10-02)

An optimized deterministic UI probe compares forced single-pass presentation
(which models the old unaligned resize paint) with two-pass presentation of the
same anchoring algorithm. Both use the same 100-line wrapped fixture, default
test style/fonts, one-point scale, 700/360-point widths, 420-point height and
20-frame warmup on macOS ARM64. No startup or native compositor latency is
included. Run with:

```sh
cargo test --release measure_resize_presentation -- --ignored --nocapture
```

| Warm workload | Forced single pass | Corrected presentation |
| --- | ---: | ---: |
| 40 alternating resizes | 55.559 ms / 40 passes | 56.998 ms / 80 passes |
| Maximum presented caret jump | 1,440 points | 0 points |
| 100 settled frames | 3.574 ms / 100 passes | 3.427 ms / 100 passes |

The correction adds a bounded second pass only when reflow moves the anchor by
more than half a point; idle frames do not request discard. This local probe
compares presentation policies, not two historical binaries, CPU samples or
cross-platform input latency. Its metadata and complete log are retained in
`.tiptoptyp/profiles/panel-resize/measurement.log`.

## Sparse hunk formatting audit (2026-10-03)

The formatter used to count the full prefix twice for each separated hunk. It
now advances line offsets only through the gap since the previous hunk, retaining
identical unified-diff output. This removes the quadratic prefix-counting step.

An isolated probe compiles the actual production `make_hunks` and unified-diff
codec bodies with `rustc --edition=2024 -O`, comparing `c32e076` with the fixed
implementation on macOS 14.6.1 ARM64, Rust 1.98.1. Both use 10,000 lines and 33
separated replacements, five warmup calls, then 100 calls per sample. Three
alternating before/after process pairs provide 15 samples per implementation,
with other builds and native journeys finished before the matched run.

| Active component workload | Before | After |
| --- | ---: | ---: |
| Median of 100 hunk-generation calls | 53.460 ms | 37.782 ms |

This is a 29.3% reduction for the measured component, not whole-editor latency.
It excludes alignment, startup, GUI rendering and idle/cache-hit behavior; none
of those workloads is compared to these timings. The algorithm adds no idle work.
The probe generator, compiler metadata, implementation hashes and complete logs
are retained under `.tiptoptyp/profiles/recent-commit-audit/`; the generator reads
production sources from its current checkout and builds the isolated probe.

## Editor typing frames (2026-10-06)

A new opt-in `editor_typing_frame_cost_probe` commits one character per
headless editor frame through the real Typst editor (highlighting, folding,
completion, diagnostics plumbing included) on a 2,000-line fixture, plus
subsystem micro samples with one edit per sample so revision caches rebuild.
Run with
`cargo test --release --bin tiptoptyp editor_typing_frame_cost_probe -- --ignored --nocapture --test-threads=1`.
`TIPTOPTYP_PROBE_LINES` rescales the fixture, `TIPTOPTYP_PROBE_MICRO`
adds the subsystem samples, and `TIPTOPTYP_PROBE_REPEAT` stretches the run
for native sampling. `TIPTOPTYP_PROBE_NO_RAINBOW` and
`TIPTOPTYP_PROBE_NO_GUIDES` toggle those passes for differential runs.

Native sampling of the probe attributed the frame to several full-document
passes per keystroke rather than one hotspot: our syntax tree was walked
separately for highlighting, rainbow brackets, indentation guides, scope
regions and the completion index, on top of egui layout. Three fixes roughly
halved the frame, measured back-to-back on a quiet Apple M2 Max machine
(medians; full data in
`performance-results/editor-typing-2026-10-06.json`):

| Typing frame, 174 KB Typst | Before | After |
| --- | ---: | ---: |
| Frame median | 32.572 ms | 17.749 ms |
| Frame p95 | 39.991 ms | 18.880 ms |
| Highlight rebuild micro | 14.047 ms | 7.293 ms |
| Completion micro | 2.594 ms | 0.573 ms |

A 17 KB fixture types at 1.749 ms, so the cost scales with the document and
fixed per-frame overhead is small. The fixes: `TextFormat` is cached per
syntax role instead of built (including a formatted font-family `Arc`) per
leaf, which also restores `LayoutJob`'s merge fast path; the guide block
walk is skipped when no line is indented and no math marker exists; the TeX
completion index walk is skipped when the source has no backslash, `mi`, or
`tex` marker. Deterministic tests pin the skip conditions and the direct
Typst convert behavior is unchanged.

This is CPU-side editor cost only, not preview compilation, LSP servers,
painting, or end-to-end latency, and not a cross-platform guarantee.
Remaining per-keystroke work is still linear in the document (highlight walk
plus egui shaping); incremental region highlighting would be a larger
follow-up.

## Native startup audit (2026-10-03)

Startup now defers the generic Syntect grammar and default-theme databases until
first use. Typst retains its own parser, and a supplied syntax theme no longer
causes default themes to be decoded. Fixed platform CJK fallback bytes are held
in one bounded process cache and registered as borrowed font data. This removes
egui's full collection copy when creating a font engine and repeated reads for
later windows/font changes. The font, face index, fallback order and glyph bytes
are unchanged; user-selected fonts retain their existing loading behavior.

`scripts/measure-startup.py` measures **process spawn to the first fresh editor
state snapshot**, including native window initialization and document loading.
It does not measure compositor presentation or preview readiness. It uses a
fresh private settings store/workspace and the ordinary interactive app, without
QA scenes or screenshots. CPU sampling is optional and belongs in separate runs.
The runner saves metadata and completed observations after every launch, builds
its native driver directory on fresh checkouts, and cleans up the app even if
sampling or native quit input fails. Comparison launch order reverses each round
to counterbalance systematic order effects. Failure-path regressions run with
`python3 -m unittest discover -s scripts -p test_measure_startup.py`.
Build both binaries with `cargo build --release --features desktop-ui-tests`,
preserve them, then run:

```sh
python3 scripts/measure-startup.py --binary /absolute/before --compare /absolute/after \
  --fixture empty --runs 7 --output .tiptoptyp/profiles/startup-comparison
```

The two binaries were built with the same release profile (thin LTO, one codegen
unit, stripped), Rust 1.98.1 on macOS 14.6.1 ARM64. Each run used a fresh process;
filesystem caches were not flushed. Before/after launches alternated, with one
initial launch per binary retained separately and excluded from the medians.
No builds or other profiling workloads ran during the matched measurements.
Both used the same 1400×900 window, dark default interface, 100% UI scale and
1512×982-point display at 2× backing scale. All observed source sizes matched.

| Launch workload | Measured pairs | Before median | After median |
| --- | ---: | ---: | ---: |
| No document argument, fresh history/default initial buffer | 7 | 646.447 ms | 598.117 ms |
| Small Typst document | 5 | 627.674 ms | 620.179 ms |
| Small TeX document | 5 | 556.521 ms | 557.358 ms |

The no-argument median improved 7.5%. Typst/TeX differences are small relative to
run-to-run variation; these samples do not establish a gain or regression for
those workloads, packaged applications, large documents, cold filesystem caches
or other platforms. Native OS/window setup still dominates much of startup.
Preview/compiler readiness is a separate future measurement.

Binary hashes, versions, compiler/display metadata and every sample are retained
in `.tiptoptyp/profiles/startup-matched-{empty,typ,tex}/measurement.json`, with
per-launch logs. The preserved binaries are in `startup-binaries/`. A separate
bounded native CPU sample is in `startup-cpu-before/`; it mainly caught idle
run-loop stacks after initialization, so it is not evidence ranking startup CPU
hotspots. The optimized component probe (`startup_components_probe`, ignored by
default) recorded the baseline font setup and first text-frame costs separately;
its wall times are not CPU times or end-to-end window latency.

Deterministic regressions verify that construction does not force databases,
custom themes do not decode default themes, grammar data is shared after first
use, and two actual font engines retain borrowed pointers to the same CJK bytes.
Existing highlighting and glyph-coverage tests still pass. No worker, idle
repaint loop, production profiling hook or per-frame I/O was added.

### Startup runner audit (2026-10-03)

Reproduced three cleanup failures deterministically before fixing them: sampler
launch failure, sampler timeout, and native quit-driver failure could leave the
measured app alive. Cleanup now surrounds sampler launch and has a separate app
finally block; bounded waits escalate to termination and kill. A fresh-checkout
output-directory omission and loss of earlier observations after a later failed
launch are also fixed. Eight Python regressions cover those paths and comparison
order. Lazy syntax loading and borrowed platform font storage were reviewed;
no further production change was required.

A native sampled smoke run (one warmup plus one measured launch) completed in
`.tiptoptyp/profiles/startup-audit-smoke/`, including automatic shutdown and CPU
sample files. Its timings are diagnostic only: sampling and concurrent checks
make it unsuitable for comparison with the earlier optimized measurements.
There is no production performance change in this audit.
