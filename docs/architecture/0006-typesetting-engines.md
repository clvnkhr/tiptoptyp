# Typesetting engine foundation

Status: implemented, 2026-09-22. Todo 222.

The subsequent native TeX implementation is documented in
[Native TeX services](0007-tex-services.md); the scope below records the foundation.

## Scope and existing constraints

Prepare for native TeX compilation while preserving today's Typst editor. This
change does not install a TeX distribution, select a TeX engine, or claim TeX
builds work. A `.tex` document contains native TeX; miTeX dollar notation remains
an optional editing representation of a **Typst** document. TeX source must not
be projected into Typst, sent to Tinymist, or compiled by the Typst executable.

Inspection before this refactor found four concerns conflated at the boundary:

- `DocumentKind::Text` includes TeX, so language identity is lost on opening.
- `CompileRequest` exposes Typst executable/font options beside general source
  and PDF requirements. The service loop owns Typst watcher log semantics.
- The application parses every build failure and warning as Typst short output.
- Build availability is often inferred from Typst editing or Tinymist preview
  support. Those are different capabilities for a PDF-producing TeX engine.

Keep stable tab/document keys, protected save receipts, window-owned services,
the existing preview recovery controller, and canonical immutable PDF artifacts.
The retiring raster viewer gains no new responsibilities. No per-tab services,
plugin registry, generic LSP framework, or second background executor is needed.

## Boundaries

| Owner | Contract |
| --- | --- |
| Core document model | Recognizes native TeX as editable source. A typesetting language is independent of which executable builds it. Ordinary text/PDF/images have no typesetting language. |
| Language support policy | Pure, explicit mapping from document kind to the currently implemented build engine and language service. TeX is editable but has no build or language service yet. No tool probes or process creation. |
| Application build adapter | Chooses the preview entry and canonical source, snapshots options, schedules work, admits results, maps projected coordinates, and applies diagnostics/artifacts. It does not understand compiler log syntax. |
| Compiler service | One latest-request queue and worker per window; pause/shutdown, backend dispatch, artifact generations and optional PDF inspection. The queue, cancellation and artifact lifetime contracts remain shared. |
| Typst build adapter | Typst configuration, root/font validation, private mirror, watcher lifetime/reuse, log classification, diagnostic parsing and completed-output snapshot. No GUI or preview policy. |
| Diagnostic model | Engine-independent severity, source, one-based Unicode-scalar location, headline/details, and raw output. Adapters convert their native formats before crossing into the application. |
| Preview/PDF services | Consume immutable canonical PDF bytes and artifact keys. PDF.js handles future TeX output without knowing its compiler. Tinymist's interactive transport remains Typst-specific. |

Use a closed configuration enum for implemented engines and concrete adapters.
Adding an engine requires an explicit dispatch arm and support-policy change;
there is no stringly typed command substitution. Reject language/engine mismatch
before filesystem or process work. Engine-specific options stay inside that
engine's configuration, rather than accumulating optional fields in every build.

Backend events describe started, failed, or completed PDF output. They do not
expose watch log lines to the scheduler. A backend snapshots its output before
its temporary workspace can be updated or removed; publication and inspection
share those exact bytes. Dependency rebuilds retain distinct artifact generations
even at the same editor revision. Delayed callbacks from an old watcher remain
tagged and ignored. A rejected request must retire the previous build session.

Build diagnostics are parsed once on the worker. Service failures are explicit
global diagnostics, not strings fed through a language parser. Existing diagnostic
normalization and projected-coordinate mapping remain at their current owners.

## First refactor and validation

1. Introduce native TeX document identity and the pure support matrix. Preserve
   editing, undo/save, generic syntax highlighting, and no-miTeX behavior.
2. Split the compiler scheduler/publication path from the Typst session adapter
   and typed configuration. Keep watch reuse, latest-only requests, pause/drop,
   private output cleanup, export-first publication, and optional inspection.
3. Move Typst short-output parsing behind the adapter. Pass structured reports
   to the application, including compile failures.
4. Test language routing, untouched TeX saves, service gating, mismatch rejection,
   queue replacement, stale watcher events, failures/recovery, exact bytes and
   artifact generations. Run real Typst success/error/recovery and required
   repository checks. Use deterministic tests rather than screenshots for this
   nonvisual refactor. Record matched optimized build-loop observations.

## Subsequent TeX implementation

Tectonic is the likely first TeX adapter; the engine choice remains open. An
installed TeX toolchain and a self-contained engine have different package,
network and reproducibility tradeoffs; replacing the Typst executable path does
not provide either one.

The next implementation must define and test:

- engine/tool discovery and settings, main-entry selection (including included
  files), package resolution, and explicit shell-escape/network policy;
- an engine-owned isolated build workspace and auxiliary-file lifetime. Do not
  reuse the Typst mirror blindly: relative inputs, bibliography files, output
  names and multi-pass tools have different contracts;
- build cancellation and child-process-tree cleanup, partial/failed builds,
  dependency invalidation and bibliography/index passes;
- a documented unsaved-source policy. Today only the Typst main buffer is
  overlaid for CLI builds; imported files are read from disk. Do not silently
  imply a full multi-buffer overlay;
- log-to-diagnostic conversion with real Unicode/file-location fixtures;
- PDF.js output delivery and optional SyncTeX source navigation. PDF production
  alone does not imply forward/inverse search or interactive Tinymist support;
- language intelligence as a separate adapter, retaining existing document,
  version and generation checks. Share transport only where protocols agree.

A future engine may use finite multi-pass builds instead of a persistent watcher.
Its lifecycle must fit the shared service events without fabricating Typst watch
messages. Engine switching must cancel the old session and invalidate pending
artifacts/exports; it is not just a different executable in a live session.

## Implementation evidence

The shared service is `src/compiler.rs`; the concrete adapter and short-output
decoder are `src/compiler/typst.rs` and `src/compiler/typst/diagnostics.rs`.
`src/app/build.rs` consumes structured `DiagnosticReport` values and retains the
existing projected-coordinate mapping. `src/language_support.rs` owns the pure
support matrix and preview-root selection. Save As carries `DocumentKind` rather
than a Typst boolean, preserving native TeX filenames and default extensions.

Deterministic regressions cover a native TeX open/edit/undo/redo/save round trip,
miTeX rejection, Tinymist admission, a TeX editor beside a pinned Typst preview,
capability-cache invalidation, language/engine mismatch before effects, latest
request/pause replacement, late watcher logs, private-session cleanup, immutable
output after replacement/deletion, and separate generations for dependency
rebuilds at one revision. A PDF-only capability selects PDF.js without reporting
a nonexistent interactive-service failure. The established command, native-view,
multi-window and protected-save suites remain in place.

Validation on macOS arm64: formatting, strict Clippy, all 1,187 non-ignored tests
(21 opt-in tests remain ignored in the normal suite), and all 14 xtask tests pass.
The first sandboxed app-only run could not open the PDF.js test's local socket;
the full suite with local socket/PTY permissions passes. No layout, framebuffer
or native-view composition changed, so no screenshots were captured. This does
not claim a new native visual acceptance or Linux/Windows runtime validation.

Performance evidence uses the existing real-tool
`compiler::tests::persistent_watcher_compiles_errors_and_recovers` test in the
same release profile before and after. Each run builds a one-page document,
builds a syntax error, then recovers to a two-page document, inspecting PDF page
metadata after each success. One complete run warms the tools, followed by five
measurements. This is a headless worker workload, with no viewport or theme.
Run metadata, binary hashes, exact tool versions, the measurement script and
logs are retained locally in `.tiptoptyp/tex-foundation-evidence/`.

With Rust 1.98.1 and the pinned Typst 0.15.1 on macOS 14.6.1 arm64, the median
complete cycle was 1.057 seconds before and 1.061 seconds after (about +0.4%).
The five samples ranged from 0.996–1.067 seconds before and 1.036–1.149 seconds
after. This does not establish a meaningful performance change or a
cross-platform guarantee; startup, tool execution and test polling dominate this
small end-to-end workload. No GUI interaction or cold-system benchmark was run.

The refactor adds no worker, no idle repaint source, and no tool discovery in
paint. The request slot remains latest-only, ordinary source is copied once per
submitted build as before, and PDF publication shares its `Arc` with inspection.
Diagnostic decoding now happens on the compiler worker rather than in the UI
result handler. Empty backend polls allocate no event storage. These invariants
and the focused tests matter more than a small local wall-time difference.
