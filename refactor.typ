#set document(title: "Refactoring for correctness and testability", author: "tiptoptyp architecture review")
#set page(paper: "a4", margin: (x: 1.9cm, y: 1.8cm), numbering: "1")
#set text(size: 10.5pt)
#set par(leading: 0.65em)
#set heading(numbering: "1.")
#show heading.where(level: 1): set text(size: 16pt)
#show heading.where(level: 2): set text(size: 12pt)
#let source(path, symbol) = block(above: 4pt, below: 6pt)[
  #text(size: 9pt, fill: rgb("46546a"))[Evidence: #raw(path) · #raw(symbol)]
]

#text(size: 23pt, weight: "bold")[Refactoring for correctness\ and testability]

tiptoptyp · Architecture review · 13 September 2026

= Implementation record — 13 September 2026

The R1–R8 boundary refactors are implemented. Product features proposed alongside
this review are recorded separately as todo items 114–118. Items 119–120 cover
the module extraction and this implementation.

#table(
  columns: (0.5fr, 4.2fr, 3.3fr),
  inset: 6pt,
  align: left,
  table.header([ID], [Implemented boundary], [Evidence]),
  [R1], [Private document source, identity, saved state and history. Edits finalize history and version advancement together, including unwind. Consumers receive one immutable snapshot. Save receipts retain the exact written snapshot and window owner.], [`core/src/document.rs`; generated Unicode edit/history sequences; stale and cross-window save tests; compile-fail field-access test.],
  [R2], [Exclusive document workflow phases, consumed save continuations, and version-specific close permits. App-wide close obtains each window’s answer and commits only when all approved document versions still match; cancellation revokes the batch.], [`core/src/workflow.rs`; `core/src/closing.rs`; `core/tests/save_close.rs`; `src/windowing.rs`.],
  [R3], [Workers receive explicit owner viewport targets. Document keys include window identity; asset and thumbnail tokens are distinct types. Asset and compiler request mailboxes retain one pending request. Mutations cannot be superseded; completion survives window closure and is reported by the shell. Process exit waits for active operations. Dead services report a terminal outcome.], [`src/worker.rs`; `src/worker/latest_queue.rs`; `src/worker/exclusive.rs`; owner-repaint, queue saturation, disconnection and closed-owner completion tests.],
  [R4], [Canonical PDF bytes and build identity are stored together. Raster acceptance checks provenance internally and retains stale display content deliberately. Interactive endpoints use typed URLs with process generations; a retained URL does not imply readiness after restart.], [`core/src/preview.rs`; `core/src/connection.rs`; out-of-order raster and same-URL restart tests.],
  [R5], [New headless `tiptoptyp-core` workspace crate. Document, workflow, close, preview, text, geometry and scheduling rules have no GUI or process dependencies. Time is supplied to debounce decisions. One-shot process waiting centralizes timeout, monitoring and reaping.], [`core/Cargo.toml`; `core/src/scheduling.rs`; `src/process.rs`; headless save/close event sequences; real temporary-repository and child-process tests.],
  [R6], [Byte offsets, scalar offsets, line indices and UTF-16/scalar columns are distinct types. Native preview placement requires a validated native rectangle produced by the viewport transform.], [`core/src/text.rs`; `core/src/geometry.rs`; Unicode boundary round trips; compile-fail unit mismatches; native bounds trace.],
  [R7], [Imported palettes belong to the egui context, with explicit palette inputs for non-context consumers. Child-window specifications have private fields. New native children wait until an appearance-change frame finishes. Boundary checks restrict immediate viewport creation and font installation to rendering adapters.], [`src/theme.rs`; `src/child_view.rs`; `tests/architecture_boundaries.rs`; interleaved-context theme and existing atlas regression tests.],
  [R8], [Workspace import/delete targets are constructed through their owning root and revalidated on execution. Atomic writes distinguish pre-commit failure from committed-but-uncertain durability. Same-resource writes and Git mutations serialize across app windows. A durability warning keeps a pending close open.], [`src/workspace.rs`; `src/private_workspace.rs`; `src/resource_lock.rs`; injected persist/sync failures and changed-symlink target tests.],
)

== Bugs found while enforcing the boundaries

- The new Unicode round-trip test reproduced a panic when LSP edits addressed
  the empty final line after a newline. The line reader stripped the preceding
  line’s terminator and produced a reversed byte range. It now trims terminators
  only within the requested line.
- A disconnected long-lived worker could look like an empty result queue,
  leaving a loading state indefinitely. Disconnection now produces one terminal
  service result, after already queued events have been drained.
- PDF link extraction had no time limit. It now uses the shared monitored
  process wait with a 60-second bound and guaranteed child reaping on failure.
- A native capture reproduced a Glow texture-creation abort when light/dark
  appearance changed in the same frame that created a child window. The
  protected rendering boundary now defers only new children for that frame;
  existing windows retain their state. Same-theme transitions and direct
  Settings capture served as controls.
- Capture scenes that replaced the document could lose the original batch
  fixture. The batch now owns a separate restore point; returning from a font
  scene restores source, path, kind and fingerprint, invalidates incompatible
  preview content, and advances document identity.
- Post-rename directory-sync failure previously reported an ordinary failed
  write even though destination bytes had changed. The explicit durability
  outcome now records the written snapshot and prevents an automatic close.

== Module boundaries and remaining limits

Settings controls, source-editor rendering, and native child views now live in
`src/app/settings_view.rs`, `src/app/editor_view.rs`, and
`src/app/native_views.rs`. The app tests live in `src/app/tests.rs` and window
orchestration remains in `src/windowing.rs`. The main app file is approximately
14,800 lines, down from approximately 22,800 including tests; it still contains
substantial runtime and shared UI helpers. This extraction does not claim that
all application logic belongs in the core.

The core owns decisions and provenance; rendering adapters still own egui
texture payloads and platform views. Streaming Tinymist/watch processes retain
their protocol-specific supervision. The one-shot process helper is not used
as a replacement for a streaming protocol.

Superseding a Git decoration read discards its acceptance channel; an already
running Git command can finish under its timeout. Asset/PDF decoding checks
cooperative cancellation, and a decoder may still finish its current indivisible
step. Bounded compiler/asset mailboxes limit queued requests, not the size of a
single document. Shared repository status and latency measurement remain item
117 rather than being hidden in this refactor.

Per-resource locks serialize this app’s writers. They do not provide atomic
compare-and-swap against another application, nor do validated paths eliminate
an external symlink race between validation and the OS call. The filesystem
adapter keeps destination-local staging and permission preservation.

The architectural source checks are deliberately conservative repository rules,
not a security sandbox or a proof about every possible Rust program. The core
crate’s dependency boundary and compile-fail tests provide stronger guarantees
for the particular APIs they cover.

== Verification

Formatting and strict Clippy pass. All 643 workspace tests and all 8 xtask
tests pass, with 2 environment-dependent workspace tests ignored. The full
68-image gallery was regenerated in one app session and validated. The core suite includes
headless workflow sequences, a manual-time debounce test, generated Unicode
cases, and compile-fail restrictions. Adapter tests retain real Git repositories,
filesystem writes, process fixtures, semantic UI interactions, and font-atlas
ordering checks.

Fresh viewport captures and native bounds traces are retained under
`.tiptoptyp/screenshots/refactor-review`. A viewport framebuffer cannot prove
composed native child-window placement; that desktop composition is not claimed
as visually verified. Capture results and final test totals are recorded with
todo items 119–120 below the task list.

#pagebreak()

= Original review and recommendation

Make invalid operations difficult to express, and put the remaining runtime
decisions behind a small number of deterministic transitions. The highest-value
work is to seal document mutation, model save/close workflows explicitly, and
give asynchronous work an owner and a typed identity. Splitting the large app
file helps only when it also changes these dependency and ownership boundaries.

The original review below is retained as the design rationale. Its Observed
paragraphs and source references describe the pre-refactor baseline, not the
current APIs. The implementation record above identifies the replacement
boundaries and their verification. The review did not claim that every risky
pattern was already causing a bug.

The review covered document and save workflows, editor mutations and derived
data, multiwindow orchestration, Git and other workers, compiler/preview state,
text coordinates, presentation state, filesystem writes, and existing tests.
References identify symbols in the current working tree; line numbers are
deliberately omitted because these refactors will move them.

#table(
  columns: (0.6fr, 2.6fr, 0.8fr, 3.5fr),
  inset: 6pt,
  align: left,
  table.header([ID], [Boundary to strengthen], [Priority], [Class of failure addressed]),
  [R1], [Document mutation and snapshots], [First], [Stale caches, missed undo, edits without revision changes],
  [R2], [Save, dialog, and close workflows], [First], [Invalid continuations, duplicate actions, stale approval],
  [R3], [Owned asynchronous operations], [First], [Wrong-window results, stale failures, abandoned work],
  [R4], [Preview artifacts and backend state], [Next], [Mismatched PDF/raster state and invalid readiness],
  [R5], [Headless core and effect adapters], [Enabler], [Untestable orchestration and timing-dependent tests],
  [R6], [Text and geometry units], [Next], [Byte/scalar/UTF-16 confusion and scale conversion errors],
  [R7], [Presentation and viewport ownership], [Next], [Ambient theme state and bypassed rendering safeguards],
  [R8], [File operations and commit outcomes], [Next], [Unscoped writes and ambiguous save failures],
)

Start with a narrow part of R5 while implementing R1. These are incremental
changes, not a request for a framework rewrite. “First” reflects potential
impact on document correctness; “Next” remains worthwhile but depends on the
earlier boundaries. No schedule estimate is implied.

= R1 — Seal document mutation

#source("src/document.rs", "DocumentSession, DocumentKey, complete_save")
#source("src/app.rs", "undo_editor, toggle_comments, save_to_with_intent, rename path, TextEdit construction")
#source("src/editor_data.rs", "EditorRevision, EditorDerivedData::prepare_source")

*Observed.* `DocumentSession` owns history and identity, but source, saved
source, path, epoch, revision, kind, and disk fingerprint are all writable
through `pub(crate)` fields. The app directly changes the buffer for undo,
comments, formatting, completion, search replacement, and widget editing.
Rename also changes identity fields directly. Correctness depends on following
each change with the right history, revision, cache, and service updates.
`prepare_source` trusts its revision key and skips rebuilding when that key
has not changed. `DocumentKey` and `EditorRevision` separately represent the
same epoch/revision pair.

*Design.* Make document fields private. Expose an immutable, versioned
`DocumentSnapshot` containing source and identity together, plus deliberate
commands such as edit, undo, redo, rename, load, and record-save. Use one
document version type for source-derived consumers. Separate window/session
identity from the document version; two windows can have different buffers
for the same disk path.

The egui text adapter must not expose unrestricted access to the stored String.
It can implement the widget buffer interface through an edit transaction,
then commit once per interaction. The transaction owns history grouping,
revision advancement, and cursor mapping. It emits a `DocumentChanged` event
for compilation and indexing. Avoid copying the entire source each frame;
retain the current immutable snapshot caching strategy.

`complete_save` should consume a receipt describing the bytes and identity
actually written, rather than a caller-supplied `path_changed` boolean. If a
future asynchronous save finishes after another edit, record the saved snapshot
without marking the newer buffer clean.

*Enforced invariant.* Outside the document module, source cannot change without
the mutation boundary. A cache cannot be passed an independently selected key
and unrelated source. Correct transaction behavior still needs tests; private
fields do not prove the algorithm correct.

*Tests and first slice.* Route comments, completion, and undo through the new
API first, then widget edits and rename. Test edit → undo → redo, no-op edits,
save-as, rename with unsaved content, and save completion after a newer edit.
Generate Unicode edit sequences and compare cached metadata with a full rebuild.
Use compile-fail coverage for forbidden field mutation and mismatched snapshot
construction. Done means every production mutation uses this boundary,
including snapshot-scene setup through a dedicated fixture constructor.

= R2 — Model workflows as transitions, not loosely related fields

#source("src/workflow.rs", "DocumentWorkflow, PendingDocumentAction, DeferredDocumentAction, PendingExport")
#source("src/app.rs", "save_to_with_intent, poll_document_dialog, poll_export_dialog, request_document_replacement")

*Observed.* The workflow has separate optional modal, pending action, post-save
action, native dialog, export, and export dialog fields, plus `allow_close` and
focus flags. `is_busy` receives additional state from outside the workflow.
The existing keyed actions are valuable, but transitions and cleanup remain
distributed through the app. States that should be mutually exclusive are
structurally representable at the same time.

*Design.* Use a small document-operation state machine, for example Idle,
ChoosingDestination, AwaitingDiscardDecision, AwaitingOverwriteDecision,
Writing, and Continuing. Each variant carries precisely the data it needs.
Keep export as a separate machine because export and ordinary editing can
legitimately overlap. Keep focus/blur presentation state outside both machines;
do not build one enormous enum for every independent UI condition.

Move decision-making into a deterministic transition function:

```rust
fn transition(
    state: WorkflowState,
    event: WorkflowEvent,
    document: &DocumentSnapshot,
) -> (WorkflowState, Vec<WorkflowEffect>);
```

This signature is illustrative. Events include dialog cancellation, save
completion, external modification, document replacement, and close request.
An overwrite decision carries the target, observed disk version, and document
version. Consuming the decision produces one write request. A write completion
must match its operation ID before it can release a close/open continuation.
An app-wide close coordinator aggregates per-window outcomes and abandons
the close if any window cancels.

*Enforced invariant.* A workflow cannot simultaneously be in two exclusive
phases. Continuations are owned by the active operation and cannot run twice
through that machine. Version checks still occur at runtime; enums alone do
not prevent stale filesystem observations.

*Tests and first slice.* Extract save-before-close first. Drive event sequences
without a native dialog: cancel, duplicate completion, edit while a chooser is
open, a second external modification after confirmation, and one cancelled
window among several closing windows. Assert that cancel never emits a write,
failed saves never emit close, and stale completion never affects a replacement
document. Retain semantic tests for mapping buttons to the correct events.

= R3 — Give every asynchronous operation an explicit owner

#source("src/worker.rs", "LatestJob::start_with_repaint, poll, cancel")
#source("src/git/editor.rs", "RequestKey, GitEditorState::prepare_request, accept")
#source("src/asset.rs", "AssetLoader, AssetThumbnailLoader, worker_loop")
#source("src/compiler.rs", "Compiler, CompileRequest, send_result")
#source("src/tinymist.rs", "Generation, emit")
#source("src/windowing.rs", "open_pending_windows")

*Observed.* Git has a workspace/path/document key; assets use numeric tokens;
compilation uses revisions and artifact generations; Tinymist has process
generations and request tokens. `LatestJob` correctly captures its repaint
viewport and reports disconnection, but `cancel` only drops its receiver.
Long-lived asset/compiler/Tinymist workers still contain unscoped
`context.request_repaint()` calls. This is an ownership inconsistency, not a
newly reproduced failure in this review. Secondary editors are constructed
before their secondary viewport is rendered, so capturing the current viewport
inside every constructor would not be a reliable fix.

*Design.* Have the shell allocate a `WindowSessionId` and explicit repaint
target before constructing services. Pass a narrow event sink to workers,
rather than an egui Context. Results use an envelope containing owner,
operation ID, and the relevant scope: document version, repository identity,
or process generation. Use distinct newtypes for these domains. Centralize
acceptance for both success and failure results.

Give replaceable reads a bounded latest-request queue and cooperative
cancellation. Distinguish superseding a result, requesting cancellation, and
observing worker termination. These are different operations. Git mutations,
saves, and package removal must not use a “drop the old result and forget it”
policy: serialize mutations per resource and retain their completion record
even if the initiating window closes. Shared repository status may be cached;
unsaved-buffer hunks and open chunk views remain owned by each window.

*Enforced invariant.* Callers cannot accidentally interchange token kinds or
publish an unowned completion through the new API. Central dispatch rejects
results for closed owners or superseded operations. Cancellation cannot
guarantee interruption of every decoder or child process; adapters must state
their cancellation and shutdown bounds explicitly.

*Tests and first slice.* Migrate Git decoration reads and file dialogs, then
assets and compilation. A fake executor should deliver completions in arbitrary
order, including errors after replacement and completion after window close.
Test two buffers of the same path, a different repository, spawn failure,
worker disconnection, and a full queue. Verify that cancelling a read cannot
silently cancel or duplicate a queued mutation. Require no desktop or sleeps
for these ownership tests.

= R4 — Couple preview data to its provenance

#source("src/preview.rs", "PreviewController, accept_artifact, replace_raster, raster_content_freshness")
#source("src/compiler.rs", "ArtifactKey, CompileArtifact, CompileEvent")
#source("src/app.rs", "preview fields, reset_document_services, pending export handling")

*Observed.* Generation-aware `ArtifactKey` checks already solve an important
problem: dependencies can rebuild while the editor revision stays unchanged.
However, PDF bytes, artifact key, raster key, pages, errors, interactive URL,
and readiness flags remain separately writable fields. `replace_raster` accepts
a key and pages without requiring the caller to prove that acceptance already
happened. Backend lifecycle also spans the controller and app WebView fields.

*Design.* Store the canonical artifact as one value containing build identity
and PDF bytes. Represent raster state separately as Missing, Rendering,
Ready with artifact identity and pages, or Failed with identity and error.
Explicitly retain a previously displayed raster when a new build starts; stale
preview is valid behavior and must not be removed by an oversimplified enum.
Derive freshness by comparing artifact identities.

Model the interactive connection independently as Disabled, Starting,
Connected, or Failed. Connected carries a typed URL and process generation.
Backend selection and fallback explanation are derived from preference and
these states instead of being maintained as additional readiness booleans.
Use a build identity that includes the relevant session/entry identity as well
as artifact generation. A designated preview entry can differ from the file
currently being edited.

Move egui texture handles into a rendering cache keyed by raster identity.
The controller then holds CPU data and can be tested without a GPU. Export
consumes a canonical artifact; it never obtains bytes through the raster cache.

*Enforced invariant.* PDF bytes cannot be paired with a separately assigned
artifact key; a connected service must have its endpoint and generation.
Acceptance methods enforce freshness at one boundary. Runtime events can still
arrive out of order and must be validated there.

*Tests and first slice.* Make the canonical artifact atomic first. Then test
same-revision dependency rebuilds, old raster success after a new PDF,
raster failure with a still-exportable PDF, stale display during compilation,
pause/resume, designated-entry switches, and server restart reusing a URL.
Keep the existing atlas and native viewport regression tests intact.

= R5 — Separate the headless core from effect execution

#source("src/lib.rs", "public theme modules")
#source("src/main.rs", "application module declarations")
#source("src/app.rs", "EditorApp, mark_edited, save_to_with_intent, reset_document_services")
#source("src/git.rs", "run_command")
#source("src/compiler.rs", "worker_loop and process lifecycle")

*Observed.* The library currently exposes themes, while document, workflow,
search, and service orchestration belong to the executable. `app.rs` is about
22,800 lines including tests. It mixes rendering with filesystem operations,
deadlines, process setup, and service synchronization. Many existing pure
helpers already have good deterministic coverage; the gap is testing complete
orchestration without assembling real services.

*Design.* Extract a small internal core crate with no eframe, Wry, native dialog,
or process-launch dependency. Start with document state, typed identifiers,
workflow decisions, and events/effects; do not move everything at once. Keep
widgets and platform adapters in the application. Put LSP protocol values in a
UI-independent module so `lsp_text` does not depend on the sidecar implementation.

Let core transitions emit typed effects. An application runtime executes them
and feeds typed outcomes back. Inject time at transition boundaries and expose
only the effect seams actually needed: document writes, process operations,
dialog selection, and task scheduling. Avoid a universal “mock operating
system” trait or a generic event bus that erases useful types.

Centralize process safety mechanics—bounded output, timeout, cancellation,
and child reaping—while preserving separate one-shot Git and streaming LSP/
watcher adapters. A single output-collecting command function is not sufficient
for a persistent protocol. Test adapters with small fixture processes and core
logic with recorded events.

*Enforced invariant.* The core cannot call GUI APIs because those dependencies
are absent. Process and filesystem isolation also need an enforced API/import
boundary check: Rust's standard library still exposes those operations. Moving
code into a crate alone does not prohibit I/O at compile time. A fake runtime
can enumerate failure orderings without relying on timing. This does not prove
platform adapters correct; integration checks remain necessary.

*Tests and first slice.* Build a headless save-before-close harness alongside
R1/R2. It should run with no DISPLAY, toolchain binaries, network, or sleep.
Advance a manual clock through debounce/retry boundaries. Preserve a few real
temporary-repository and sidecar tests as adapter contracts. Done means one
complete user workflow is tested end to end through core events and effects,
not merely that functions moved into smaller files.

= R6 — Make coordinate units explicit

#source("src/lsp_text.rs", "SourceIndex, range_to_char_range, lsp_position_at_char, scalar_position_at_char")
#source("src/editor_data.rs", "FontArgumentTarget, char_starts")
#source("src/app.rs", "pending_editor_selection, EditorGutterGeometry, preview bounds conversion")

*Observed.* The code already centralizes atomic LSP edits and documents the
difference between UTF-16 and scalar preview columns. Nevertheless, byte
ranges and scalar selections are commonly `Range<usize>`, and several positions
are plain integer pairs. Geometry similarly passes ordinary Rect/Vec2 values
across egui, native-window, and raster coordinate spaces. Comments and helper
names carry distinctions the type checker cannot enforce.

*Design.* Introduce private-constructor `ByteOffset`, `ScalarOffset`,
`Utf16Column`, and `LineIndex` types with matching range/position types. Tie
validated edit ranges to a document version and resolve them against its source
index. Do not alias byte and character ranges to the same Rust type.

Give native preview boundary conversion named input/output types, such as
EguiRect, NativeLogicalRect, and RasterPixelSize, with one conversion object
carrying validated scale information. Focus these wrappers at subsystem
boundaries; avoid wrapping every local arithmetic value merely for consistency.
Navigation can remain forgiving about out-of-range positions, while edit
application remains strict. Express those as different conversion methods.

*Enforced invariant.* A byte range cannot be passed to a scalar edit API without
an explicit conversion. A native placement function cannot accept an unconverted
egui rectangle. Correct conversion formulas and validity against a changing
document remain runtime obligations.

*Tests and first slice.* Start with completion, formatting, and search edits.
Test round trips only at valid Unicode boundaries; a split surrogate is not a
valid round-trip input. Cover emoji, combining marks, CJK, CRLF, empty lines,
and EOF. Generate non-overlapping edit batches and compare with a simple
reference implementation. Geometry properties should cover clipping, wrapping,
multiple display densities, and scale round trips within an explicit tolerance.
Retain native composition tracing and fresh visual evidence where pixels matter.

= R7 — Own presentation state at the rendering boundary

#source("src/theme.rs", "IMPORTED_PALETTE, set_imported_palette")
#source("src/presentation.rs", "AppliedPresentation, PresentationChanges")
#source("src/viewport_fonts.rs", "show_immediate, after_immediate_viewport")
#source("src/child_view.rs", "ChildViewSpec")
#source("src/windowing.rs", "shared_settings, take_focused_settings_update")

*Observed.* AppliedPresentation is a useful consolidated snapshot. Imported
palette selection still lives in thread-local mutable state, which distinguishes
threads but not multiple egui contexts or nested viewport renders on one
thread. The recent font-atlas repair depends on callers using the immediate
viewport wrapper. The application also keeps settings copies per editor and
merges updates through the shell. These mechanisms work through conventions
that future call sites could bypass.

*Design.* Make the shell/context own the resolved presentation and issue
immutable render snapshots to widgets. Pass palettes explicitly, or obtain them
from context-owned data through a narrow adapter. Treat fonts according to
their actual shared egui-context lifetime; do not invent per-window font
independence where the renderer has a shared atlas.

Give the rendering adapter exclusive responsibility for immediate viewport
creation, font installation, and atlas synchronization. Child specifications
include their window owner and derive unique viewport/capture identities there.
The renderer can validate role/focus combinations instead of allowing arbitrary
public field construction. Keep the four existing unsafe platform modules
small and preserve the crate-level unsafe lint boundary.

*Enforced invariant.* Pure widget/core code cannot change the global palette or
fonts if it receives only a resolved presentation and narrow view interface.
Within code that still receives a full egui Context, wrapper use is an
architectural rule, not a Rust type guarantee. Add a targeted boundary check
for direct viewport/font calls as an interim safeguard, with explicit exceptions
for renderer tests. Keep production font previews on their isolated worker atlas.

*Tests and first slice.* Remove thread-local palette selection first. Interleave
two contexts with different themes on one thread, and render nested child
windows. Test that each reads its own resolved palette. Preserve the existing
incremental-atlas and repeated-parent-pass tests. Atlas upload ordering and
native composition still require renderer tests and proportionate fresh captures;
this design does not eliminate GPU or platform bugs.

= R8 — Make file operation scope and commit outcome explicit

#source("src/workspace.rs", "import_file, delete_file")
#source("src/private_workspace.rs", "AtomicFileWriter, atomic_write_with_staging_root")
#source("src/app.rs", "save_to_with_intent, confirm_disk_unchanged")

*Observed.* Import/delete already reject several unsafe targets, and atomic
writes use destination-local staging. Keep those protections. Operations still
take broadly interchangeable PathBuf values and return mostly string or I/O
errors. In the writer, persistence happens before parent-directory sync; both
pre-persist failure and post-persist sync failure return Err. Therefore the
result type does not tell a caller whether the destination has already changed.
This is visible in control flow; no disk-failure incident was reproduced here.

*Design.* Separate validated workspace-relative targets from explicit user
export/save destinations and app-owned temporary paths. Construct workspace
targets through a repository/workspace object, and revalidate at execution.
Use distinct Import, Delete, SaveExisting, and SaveAs commands with relevant
preconditions. A confirmation token identifies the operation, document version,
target, and observed disk version; it is consumed by the write request.

Return a structured write outcome: not committed, committed with uncertain
durability, or committed and synchronized to the guarantees available on the
platform. Include a receipt for the written snapshot. The workflow can then
show an accurate message and avoid treating an already committed write as an
unperformed operation. Decide and test whether a durability warning permits a
pending close; do not bury that product policy inside the filesystem adapter.

*Enforced invariant.* APIs distinguish workspace operations from arbitrary
destinations, and callers must handle whether the commit point was crossed.
Validated path wrappers alone do not stop symlink replacement or external writes
between validation and use. Serialize the app's own writes per file; stronger
protection against external races requires platform handle-relative operations
or an explicitly documented limitation. Do not claim portable atomic
compare-and-swap semantics from a fingerprint check followed by rename.

*Tests and first slice.* Introduce write receipts before general path wrappers.
Inject failures before writing, during staging, at persist, and after persist
during directory sync. Verify both reported outcome and actual destination
bytes. Preserve permission and collision tests, add two-window writes to the
same file, and keep real filesystem tests for symlinks and destination-local
staging. A fake filesystem alone cannot verify OS rename/durability semantics.

= Delivery sequence and acceptance criteria

*Increment 1: mutation boundary.* Implement R1 with only the minimal R5 core
extraction required. Keep behavior stable and remove the old writable access
paths in the same increment. Establish typed document/window identities before
sharing any background services between windows.

*Increment 2: save-before-close.* Implement the narrow R2 machine and R8 write
receipt together, using a deterministic fake effect runner. This exercises a
complete workflow and exposes whether the proposed abstraction is useful.

*Increment 3: asynchronous ownership.* Migrate one replaceable read through R3,
then long-lived services. Add cancellation and shutdown contracts before
consolidating process adapters. Preserve separate mutation semantics.

*Increment 4: preview and boundary types.* Apply R4, then migrate text-coordinate
boundaries under R6. Convert one consumer at a time with strict constructors;
do not keep compatibility aliases that make the old and new types interchangeable.

*Increment 5: rendering ownership.* Apply R7 and the native geometry part of R6.
Split remaining app code by responsibilities now protected by these boundaries:
editor view, settings view, native views, and runtime effect adapters. File-size
reduction is a consequence, not an acceptance criterion.

For each increment, demonstrate three things:

- *A forbidden operation:* name an invalid state or call that used to be
  expressible, and show how private fields, a type distinction, a dependency
  boundary, or one compulsory runtime gate now blocks it.
- *A deterministic behavioral test:* exercise the real transition with an
  adversarial sequence, not a mock that merely repeats the implementation.
  Keep failing generated cases as reproducible regression fixtures.
- *A retained adapter check:* where behavior crosses Git, filesystems, sidecars,
  or native rendering, retain at least one real contract test for that boundary.

Use compile-fail tests for API restrictions; table-driven and generated event
sequences for workflow invariants; fake time for debounce and retry; semantic
egui tests for controls; and framebuffer/native tracing only when visually
material. Property-based tooling would be a new dev dependency if adopted;
bounded exhaustive sequences can start without it. Test counts alone are not
an architectural success metric.

Run the repository's required checks for implementation increments:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
```

Do not replace the existing regression suite with mocks, rewrite the renderer,
or centralize every service into one global mutable object. Existing strengths
to preserve include keyed document actions, artifact generations, atomic LSP
edits, the menu command registry, isolated font-preview rasterization, pure
geometry tests, temporary Git repositories, and restricted native unsafe code.
The proposed redesign extends those successful boundaries and makes bypassing
them harder.
