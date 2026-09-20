# Architecture follow-up — 2026-09-17

## Bounded PDF page residency (198–200, completed 2026-09-18)

PDF inspection now produces a lightweight all-page catalog of 144-DPI layout
dimensions and normalized links. Decoded RGBA buffers and uploaded egui textures
are separate resident resources keyed by artifact generation, page, requested
DPI and appearance revision. On artifact replacement, the current catalog and
pixels remain visible until the first valid page for the replacement arrives.
Results from an old artifact or appearance are rejected; canonical PDF bytes
remain the unchanged export source.

The fallback view computes its intersecting page range from scroll geometry,
prefetches one page on either side and caps each request at 12 pages. Requests
are latest-wins, and Poppler is polled for cancellation during rendering and
between page decodes. Typst and opened-asset previews have independent workers
because both panes can be visible together. Offscreen pages retain lightweight
geometry but do not construct image widgets or require resident pixels.

Decoded pixels and estimated GPU texture bytes are accounted process-wide at
96 MiB and 192 MiB respectively. Eviction coordinates visibility across window
owners and prefers the least-recently-visible nonvisible page. The evicted owner
is repainted so its UI thread drops the actual texture handle. A page larger
than either budget evicts all other pages and is admitted alone; this preserves
usability while preventing multiple oversized pages from accumulating.

Deterministic tests cover metadata and rotation parsing, visible-range and
prefetch bounds, supersession, artifact/theme rejection, replacement retention,
cross-window visible-page preference, oversized pages and 1/20/100-page bounded
residency. An architecture guard prevents decoded-page ownership from returning
to the compiler. The inspected fresh Catppuccin Latte `main` framebuffer showed
the fallback page populated, aligned and unclipped; it does not prove native
webview composition.

The matched opt-in accounting probe ran on Apple M2 Max, arm64 macOS 14.6.1,
Rust 1.96.0:

```sh
cargo test --release optimized_pdf_residency_probe -- --ignored --nocapture
```

With synthetic pages costing 8 MiB decoded plus 8 MiB texture each, the former
all-page model retained 16/320/1600 MiB for 1/20/100 pages. The bounded model's
cold/warm, idle/scroll/zoom sequence retained 8/24/24 MiB (1/3/3 pages). Its
local operation timings were 311834/42/0/10083/49625 ns for one page,
13125/84/0/2583/17375 ns for 20 pages, and
7875/41/42/7500/12667 ns for 100 pages. These measure the matched optimized
accounting workload, not Poppler execution, GPU allocation or end-to-end frame
time. The deterministic byte bounds—not nanosecond thresholds—are the maintained
performance contract.

## Child/native resource lifecycle (215–217, completed 2026-09-18)

The child-view host now records explicit `Visible`, `TemporarilyHidden`,
`DurablyClosed` and `DormantHosted` states per owner viewport. Hiding retains a
surface and its generation; durable close increments the generation so an old
deferred callback is inert. A dormant document owner closes transient children
but keeps only surfaces explicitly marked as native hosts (currently Settings),
and normal document painting resumes the owner. Existing scroll dismissal and
keyboard-focus state tests remain; new tests cover hide/reopen, durable
recreation, dormant hosting and stale callbacks.

Durable close removes every font-sample slot owned by that viewport, dropping
its private atlas textures and canceled/latest job state. The existing bounded
four-sample slot cache remains for temporarily hidden/reopened pickers. The
font test now measures one live slot before close and zero afterward, so repeated
open/preview/close cycles return to baseline rather than accumulating viewport
keys.

Native-preview tracing showed the same clipped geometry on two consecutive
frames:

```text
egui=(1038.7,30.0)-(1672.0,958.2)
native=(934.8,27.0)-(1504.8,862.4)
viewport=(0.0,0.0)-(1680.0,982.2)
```

The prior adapter issued background, bounds and visibility setters every frame.
The deterministic trace model therefore counts 300 native calls across 100
unchanged frames. Applied-state diffing now emits all three only at creation,
zero across those 100 stable frames, bounds alone after a move, and all three
after native recreation. The cache is invalidated when the Wry view is dropped,
its native parent changes, preview fallback occurs or the workspace empties;
changed scale/geometry naturally changes `NativeRect`. The fresh `main` viewport
capture was inspected for clipping/alignment, but—as documented—it is not proof
of native child composition; the retained bounds trace is the native geometry
evidence.

## Shared workspace observation (208–209, completed 2026-09-18)

`src/workspace_service.rs` owns one recursive native filesystem observer and one
immutable snapshot stream per canonical workspace root. Windows sharing a root
receive the same `Arc<WorkspaceSnapshot>`; each `WorkspaceTree` still owns its
generation, and Explorer selection, expansion, search and section state remain
window-local. Removing one subscriber leaves the shared root and other windows
alive; the observer shuts down when its final owner closes.

Events are accumulated for a 120 ms quiet period. Data/metadata changes notify
interested windows without rescanning the tree; create, remove and rename events
coalesce into one structural scan. Access events and `.git`, `target` and
`.tiptoptyp` paths are ignored so observing the app's own reads/private outputs
cannot create a feedback loop. A 30-second verification scan is the conservative
fallback for a missed native event. Active-file verification checks length and
modification metadata first and reads/hash-compares content only after metadata
changes or a concrete event. Atomic-save/create, delete and rename classification
and missed-event manual verification have deterministic tests.

The two-window test records one initial scan serial for both subscribers and
pointer-identical snapshot storage: one tree scan, zero workspace file-content
reads and one shared snapshot allocation. Closing the first window and refreshing
after a new file produces one later serial for the remaining window. The old
Explorer two-second scan, active-file one-second full read and their repaint
timers are removed. Language indexing and Git retain their separate models and
refresh triggers; no generic filesystem model was introduced.

## Bounded project-index reads (204–206, completed 2026-09-18)

`src/index_jobs.rs` is a deliberately specialized process-wide runner for
project indexing. Two workers serve all windows. The FIFO queue keeps at most
one pending request per window/source key, replaces superseded payloads in
place, caps each request at 32 MiB and total pending source payload at 64 MiB,
and records pending bytes before admission. It has no generic task API,
priority system, mutation path or protocol ownership.

Active work receives an atomic cancellation token. The literal-only indexer
checks it between files and every 256 syntax nodes, so a superseded large file
cannot monopolize a worker indefinitely. Completions carry owner, source key
and request identity; stale/closed-owner results are discarded, and a
successful send wakes only the requesting viewport. The previous 50 ms UI
polling repaint was removed. Queue tests show 100 same-key submissions retain
one pending payload, FIFO fairness across owners, bounded byte admission, and
owner closure canceling active/pending work. The existing completeness and
literal dependency tests remain unchanged.

The opt-in optimized comparison used a 2,000-section in-memory override, four
windows and 100 requests on Apple M2 Max, arm64 macOS 14.6.1, Rust 1.96.0:

```sh
cargo test --release index_jobs::tests::profile_repeated_multi_window_indexing -- --ignored --exact --nocapture
```

The former per-request spawning model started 100 threads and completed in
193,145,208 ns; the bounded path used two shared workers, coalesced replacements
and delivered all four latest results in 11,337,750 ns. This is a local matched
synthetic indexing workload, not a cross-platform GUI latency guarantee. It
does establish the intended concurrency and supersession behavior without a
timing threshold in normal tests.

## Preview transition ownership (194–196, completed 2026-09-18)

`PreviewController` now applies typed transition events to the existing
connection, recovery and content models and returns adapter effects for service
restart/stop, refresh mode, raster scheduling and bounded repaint deadlines.
There is no second retry counter or parallel connection state. `EditorApp`
performs those effects, while native views, texture handles, sidecar calls and
egui repaint APIs remain in their platform/UI adapters.

Service failures, recovery ticks, explicit restarts, preview-entry changes,
pause changes, stop and render requests use this path. A failure emits each
cleanup/repaint/render effect once; a duplicate terminal event emits nothing,
the fifth consecutive failure schedules one raster fallback, and exhausted or
inactive recovery emits no idle repaint. Tests also cover retained late
readiness, pause/restart, export rendering during recovery and entry changes.

Preview and settings rendering now consume one read-only status snapshot that
separates the requested and effective backend, native readiness/transition,
whether native creation should be attempted, and canonical artifact
availability. Fallback text is allocated only when a caller asks for it. An
architecture guard prevents the view from invoking the lower-level policy
queries and prevents the transition owner from acquiring sidecar or egui
effect APIs. The transition and snapshot paths are constant-time, add no
worker, queue, texture copy or periodic wake, and therefore have no material
disabled-mode or idle performance cost.

## Versioned Tinymist synchronization (191–193, completed 2026-09-18)

`src/tinymist_sync.rs` is now the window-owned synchronization policy. It owns
the active generation, current and designated-preview URIs, open URI set, and
private backing guards for active and tabbed unsaved documents. Inputs pair a
full `DocumentKey`, LSP version, URI and canonical source. Transitions emit typed
Open, Change, Close and UpdateBacking effects; `EditorApp` remains the adapter
that writes backing files and calls the existing `TinymistSidecar`.

Startup, editing, tab switches, post-initialize current-document admission,
parked-document opening, rename/close and shutdown use the same transition/effect
boundary. Canonical snapshot collection and compatible preview-root selection
are policy helpers. This also fixes parked opens using the active document's
revision: each parked input now carries its own document key/version.

Formatting, hover and completion results pass a shared generation + current URI
+ revision admission check before mutation. Pure command-log tests cover start,
edit, switch, preview-entry replacement, close, backing update and miTeX source
changes. Additional tests reject stale generation/URI/version independently and
show ordinary and projected documents retain separate display/canonical source
domains. The existing `mitex_document` tests continue to assert that ordinary
documents perform zero projection encodes.

An architecture boundary prevents UI, process, thread, sidecar and filesystem
effect APIs from entering the coordinator. When Tinymist and private backings
have no source consumer, the edit path still returns before collecting or
copying canonical source, so this refactor adds no disabled-mode typing cost,
worker, queue or repaint loop. The existing command-line limitation is retained:
imported subfiles absent from editor overrides are compiled from disk.

## Stable document records (185–186, completed 2026-09-18)

The tab model now keeps every open document in a `BTreeMap<u64, TabRecord>`
keyed by the existing stable tab identity, with a separate `Vec<u64>` defining
display order and optional active/preview IDs. The previous active hole,
parallel identity vector and positional remapping are gone. Reordering changes
only the order list. An empty workspace has no open record and uses a private
empty presentation record until New/Open creates a real tab.

Each record owns `DocumentSession`, folding, saved egui editor state, workspace
and autosave deadline for its full lifetime. A switch leaves both records in the
store, saves/restores only widget state and the active workspace, and rekeys the
incoming document/folding state against the outgoing document key. This retains
the existing late-result rejection contract without moving Tinymist, compiler,
indexing, Git, native views or texture resources out of the window owner.
Production code reaches the store through stable accessors; an architecture
test rejects direct map access outside `tabs.rs`.

Focused tests preserve the existing identity, preview, undo/selection, dirty
close, empty-workspace, save-completion and late-asset-result contracts. Added
tests verify store/order membership, per-record folding and autosave retention,
and that repeated activation starts no Tinymist, project-index or workspace-scan
job.

The opt-in optimized probe was run on Apple M2 Max, arm64 macOS 14.6.1, Rust
1.96.0:

```sh
cargo test --release app::tabs::tests::tab_store_cost_probe -- --ignored --nocapture
```

It measured 2,000 alternating switches at 894,042 ns and 732,072 requested
allocation bytes (about 366 bytes/switch), and 100 egui tab-strip frames with
100 tabs at 126,145,666 ns and 237,217,300 requested bytes. The allocator is a
test-only thread-local observer over the system allocator. These figures isolate
tab activation and egui tab-strip layout; they do not measure GPU memory,
process RSS or composed desktop frames. No timing threshold runs in normal CI.
The new ownership model introduces no worker, queue, filesystem work, service
start or repaint loop.

This batch addresses todos 187, 197, 203, 210 and 211. It preserves the previous
working-tree changes and does not change save execution timing, process/thread
ownership, PDF quality, or the visual popup layout.

## Boundaries and decisions

- **187 — save-order safety tests.** The real document/workflow adapter now has
  adversarial tests for edits while a receipt is pending, overlapping same-path
  receipts arriving out of order, failed writes (no receipt), uncertain durability,
  cancellation of close, document replacement, and one-time close continuation.
  Tests distinguish returning a continuation from granting a native close permit.
  These exercise controlled completion ordering, not concurrent disk writes;
  background save serialization remains items 188–190.
- **197 — shared PDF service.** `src/pdf.rs` owns raster modes, page/link data,
  Poppler command construction, decoding, link extraction and numeric page order.
  Compiler, asset loader, thumbnails and preview presentation import it directly;
  no old compiler aliases remain. The compiler still snapshots canonical export
  bytes once and publishes the artifact before optional rasterization. The shared
  bounded pipe-reader join belongs in `src/process.rs`. Existing tests moved with
  their owner; new fixture coverage checks staged bytes, DPI, decoded pixels and
  cancellation. Link normalization, optional links, first-page bounds and reader
  shutdown tests remain. This extraction adds no copies, queues or workers.
- **203 — reassess session/process coupling.** No further production split is
  justified yet. `Session` owns the pipe, request IDs, pending calls, document
  versions and handshake phase together; transitions send ordered messages and
  change that same state. Moving these into a parallel coordinator would require
  forwarding effects or sharing ownership without adding an independent policy.
  `ProcessSupervisor` must remain separately reachable for forced termination
  when the protocol writer blocks. Existing generation/document admission checks
  stay authoritative. New tests cover canceled and duplicate completion replies,
  replacement request preservation, graceful `shutdown` then `exit`, stopping
  before initialization, reaping before replacement, and stale supervisor cleanup.
  Existing blocked-pipe, inherited-pipe, timeout and real-server tests remain.
- **210 — completion transaction.** `src/completion_edit.rs` owns snippet
  expansion and preparation. A consumed `CompletionTransaction` carries a full
  `DocumentKey`, explicit canonical/display coordinate space, the core-validated
  replacement and resulting selection. Commit rechecks the key, runs existing
  miTeX preflight for canonical edits, and calls document edit once. The type is
  the single undo intent; it cannot be reused. It does not retain duplicate source
  snapshots or validated edit arrays after application. Pending completion state
  now retains the full key rather than relying only on a truncated LSP revision;
  local rebasing updates it. Tests cover Unicode, CRLF, split-surrogate/reversed/
  out-of-bounds ranges, revision/epoch/owner changes, and one-step undo. Existing
  miTeX completion tests exercise canonical projection. Typing/IME remains in
  TextEdit, and ordinary documents bypass translation through the existing core.
- **211 — read-only completion view.** `src/app/completion_popup.rs` takes borrowed
  items/source, geometry and selection, and returns Select/Accept/Dismiss actions.
  Font-sample painting is supplied by the adapter; the renderer cannot query a
  catalog, launch services or access the application. Selection actions are not
  repeated for an unchanged hovered row. Semantic tests click/hover real controls
  and dismiss outside. Existing placement tests still cover edge clamping.
  Per-frame clones of the item list and selected font family were removed.

The architectural guards reject application/service access from the popup,
compiler ownership of PDF decoding, and payload cloning in the popup handoff.
No screenshot was required: layout/paint code was moved unchanged, while state
and interaction changes have deterministic and semantic coverage. This is not a
claim of fresh visual or native-composition verification.

## Completion payload allocation measurement

Same machine as [the earlier performance review](architecture-performance.md):
Apple M2 Max, arm64, macOS 14.6.1, Rust 1.96.0. Both paths run in the same optimized
binary against the same prebuilt 256-item fixture (each has 4,200 documentation
bytes). Five warmups precede seven batches of 1,000 handoffs per path. No viewport
or theme applies to this isolated headless handoff; no build ran during timing.

```sh
RUSTFLAGS=-Cforce-frame-pointers=yes cargo test --profile profiling --features profiling --test completion_payload_cost completion_payload_cost_probe -- --ignored --nocapture
```

The baseline operation is the previous `items.clone()`; the new operation passes
the existing slice. A test-only thread-local allocator counts requested bytes
(not peak/live heap or process RSS). Each baseline handoff allocated **1,135,250
bytes**; the borrowed handoff allocated **0**. Median baseline time was **57.82
microseconds**. The borrowed-loop timing is below a meaningful per-call timing
resolution; its zero allocation count is the useful result, not a speedup ratio.
This excludes egui layout/labels/tooltips, font rasterization, GPU work and the
separate source snapshot taken when requesting completion. Those costs are not
claimed to disappear. Actual renderer/adapter source guards ensure the measured
borrowed ownership contract stays in use. Normal tests assert allocation
behavior; timings remain opt-in without CI thresholds.

Raw totals (nanoseconds / allocated bytes per 1,000 handoffs):

| Sample | Previous cloned handoff | Borrowed handoff |
| --- | ---: | ---: |
| 0 | 60513167 / 1135250000 | 291 / 0 |
| 1 | 57589917 / 1135250000 | 375 / 0 |
| 2 | 57660375 / 1135250000 | 333 / 0 |
| 3 | 56513000 / 1135250000 | 333 / 0 |
| 4 | 57819333 / 1135250000 | 333 / 0 |
| 5 | 58048000 / 1135250000 | 333 / 0 |
| 6 | 57827625 / 1135250000 | 417 / 0 |

Probe source SHA-256:
`d964be79b59a109b7fa07c0a3d92caae33c92f21893f09c82f1d3015ef4af958`.
Measured executable SHA-256:
`1133f23adb34bc89436fcb77898e1b05fc9b18f4b0b16d0d20556211cd77f62d`.
The earlier native profiles precede this batch; they are not presented as new
measurements of these changes. Pure moves and test additions have no material
runtime impact. The completion transaction adds constant-size identity checks;
edit validation/projection and source replacement reuse the prior algorithms.

## Next work

Immutable save worker inputs (188) build on the new adversarial tests; see the
subsequent implementation below.
PDF metadata/residency (198–200) and bounded shared indexing (204–206) remain
separate, performance-sensitive changes needing their own workload measurements.
No background executor, PDF residency cache or extra compatibility layer was
introduced as part of this batch.

## Next step: immutable save handoff (188)

Subsequent update: [189–190 now route these inputs through protected background
saves](background-saves.md). The description below records the intermediate
synchronous stage and its tests, not the current execution model.

`src/save_transaction.rs` now wraps the existing projection-aware SaveRequest
and SaveReceipt instead of defining another document identity. SaveInput owns
the original request, intent, expected disk state, and an optional continuation
token. The canonical bytes remain borrowed from that request while the injected
writer runs. Successful persistence returns the original receipt plus either
synchronized or uncertain durability. Failed persistence returns an error with
no receipt. Metadata travels with either outcome; both input and completion are
Send, ready for a later worker boundary.

Active saves and parked-tab autosaves use the handoff synchronously. Existing
disk-admission checks, manual-only formatting, atomic-file writer, resource lease,
and error/notice handling remain where they were. Expected state distinguishes
a known fingerprint, known missing destination after confirmation, and an
unchecked Save As/unknown baseline. It is recorded here, **not rechecked under
the lease yet**. That check and background execution remain item 189; this step
does not close the existing check-before-write race or claim protection against
external writers. Unified scheduling and save policy remain item 190.

The existing core Workflow allocates non-reused continuation tokens. A completion
from an earlier save cannot release or cancel a newly requested close, even when
the document receipt itself remains valid. Tokens identify actions, not documents;
the document model remains solely responsible for receipt identity/revision checks.
Matching uncertain/dirty saves still cannot release a close. Tests cover token
replacement, duplicate completion, exhausted counters, canonical Unicode/miTeX
bytes, source-pointer reuse, failed/uncertain persistence and wrong-owner/epoch
receipt rejection. Existing save-close and projected save/autosave tests remain.

No UI rendering, idle repaint or background-job behavior changed. The wrapper
adds constant-size metadata and token comparisons, with no additional source
allocation, file read or hash pass. This is preparatory refactoring, **not a
measured save-latency improvement**. The actual responsiveness work is deferred
to the controlled slow-writer and same-path concurrency tests in item 189.

## Repository execution boundary (212)

`src/git/repository.rs` owns subprocess discovery/execution, bounded output,
status decoding, repository snapshots and whole-file/index commands. Its
`diff` module owns unified hunk models/parsing and buffer comparisons; `hunks`
owns checked revert construction and index patch transactions. These modules
have no UI imports. Panel/editor adapters retain worker admission, projection
coordinate mapping, receipt routing, layout caches and rendering. The borrowed
`Repository` handle is constructed inside the existing worker closures and
does not discover Git, allocate a path or create a service thread.

Invocation still uses argument arrays and literal pathspecs, no terminal prompts,
the existing 60-second process limit and 4-MiB output limits. Panel and hunk
mutations retain the shared repository-root lease and under-lease baseline
checks from 213. Revert remains a checked string result applied through the
existing single editor undo transaction. No compatibility forwarding API remains
at the old editor mutation paths.

The extraction exposed a result-routing bug: a mutation submitted from a nested
workspace returned its resolved repository root as its workspace. The panel
treated that as a stale completion, losing commit-message clearing and requesting
another refresh. Repository results now retain the requesting workspace while
their snapshots retain the resolved root. Direct service and async panel tests
cover this distinction.

Disposable-repository coverage exercises quoted/Unicode names (quotes on Unix),
new files, CRLF and missing final newlines through scan, stage, commit, diff,
unstage and checked revert, asserting exact index bytes and unchanged working
files. Existing tests cover literal pathspecs, non-UTF-8 decoding, concurrent
panel/hunk transactions, partial staging and conflict rejection. A dependency
test guards against UI imports in the repository layer and command/codec
implementations returning to the views.

This extraction has no expected material performance impact: command sequences,
payload ownership, worker counts, output bounds and repaint scheduling are
unchanged. The nested-workspace fix adds a conditional path copy on completion
and avoids a spurious refresh. No speedup or fresh native profile is claimed.
Item 214 was completed subsequently; the next section records its read-only
views and typed returned actions.

## Read-only Git views (214)

`src/git/view.rs` renders panel snapshots through immutable Input and returns a
typed Output containing at most one repository operation, a commit-text edit and
a consumed reveal request. `GitPanel::show` performs no repository admission;
the app applies the output after Explorer section rendering. The view has no
repository handle, worker, process or filesystem access. A source-boundary test
protects that rule and a semantic test invokes the renderer directly to show a
Fetch click can only produce an action.

`src/git/editor/view.rs` similarly owns the gutter and selected-hunk popup. It
returns OpenChunk or RunHunk, while editor/controller code retains selection
validation, one-undo revert application and protected index mutations. The hunk
buttons are small controls on the same row as the popup title; shortcuts remain
in hover hints and accessibility keeps each full action label. Deterministic UI
geometry tests cover the common baseline and compact height, so this behavioral
layout change did not require screenshot evidence.

Presentation caching remains bounded and demand-driven. The commit edit mirror
resynchronizes only when model text differs, and clones text only for an actual
edit or Commit action. Diff contents use shared Arc strings; the existing galley
cache now lives in view state and keys content identity, style and font-cache
identity. Ordinary frames add no source copy, worker, IO, queue or repaint.
Therefore no material performance change is expected or claimed.

## Cached capability snapshot (219)

`src/capabilities.rs` derives one cached, read-only snapshot from the resolved
Typst/Tinymist tools and the existing live service states. It reports editing,
LSP, interactive preview, PDF generation, rasterization and PDF-link extraction
independently. A missing Typst compiler therefore does not hide a working LSP,
and missing `pdftoppm` does not misreport PDF generation or `pdftohtml` link
extraction as unavailable.

Optional Poppler executable lookup reuses the PATH policy in `toolchain.rs`.
It runs once when a window session is created and again only for the existing
explicit Refresh tools action. Tool-preference changes invalidate the derived
snapshot without repeating unrelated Poppler discovery. Settings-frame reads
compare owned state and return the cached snapshot; they perform no filesystem,
environment or process work and add no worker, queue or repaint source.

Pure tests cover partial tool/platform availability and distinguish cheap
preference invalidation from explicit re-probing. A source boundary prevents
effect APIs from entering capability derivation and prevents Settings renderers
from invoking tool discovery. Semantic Settings coverage verifies all six
capabilities remain individually visible. This is clearer status reporting and
bounded discovery, not a claimed runtime speedup.

## Stable tab identity accessors (184; user item 180)

The existing numeric tab IDs are now the window-facing identity boundary.
Cross-tab document lookup and selection, close, rename, preview, save and
navigation actions carry IDs; only `tabs.rs` translates them to the current
positional representation. Active and preview identity accessors return `None`
for the empty workspace, and positional fields plus parked storage are private.
A dependency test rejects production window code that reaches those slots or
the removed index-based document helper directly.

Characterization coverage retains reorder identity, empty-workspace behavior,
dirty-close admission, saved selection/undo history and stale save routing. A
new active-lifecycle regression specifically exercises repeated New commands:
the designated preview ID and source remain pinned while the new editor tab
becomes active. This closes user item 180 and protects its rule before item 185
changes storage.

The change replaces integer-index handoffs with constant-time ID lookup over
the same small order vector. Rendering already visited that vector and now
carries the ID it read instead of a position; there is no additional frame
scan, source clone, service start or repaint. No speedup or new native profile
is claimed.

## Follow-up state after items 249–258 (2026-09-20)

The presentation-boundary work moved shortcut and Find/Replace state into
explicit leaf modules, while the shared document-transition reset remains in
the window owner. Item 256 now has a bounded two-owner regression combining
multiple tabs, a designated preview, an in-flight locked save, a late
language-service reply and root-owned Settings. The test calls the real reply
identity adapter and does not introduce another event bus or worker policy.

Current physical counts and profiling evidence are maintained in
[`docs/app-ownership.md`](app-ownership.md) and
[`docs/performance.md`](performance.md). The current optimized profiling
endpoint has valid one-window, multi-window, Settings, 40-page PDF, deterministic
tooltip and three-tab runs. The earlier hover-readiness and tabs-shutdown
failures are retained as invalid history; item 259 fixed their runner/scene
causes without adding an idle repaint loop. Item 260 now adds separate,
bounded `hover-scroll` and `tabs-switch` input scenarios with phase/event
records; they must not be compared with idle endpoint rows. Items 229, 234 and
237 remain native acceptance gates. Proposed transaction/resource-wrapper
extractions 251 and 253 remain deferred because review found no justified
duplicate policy to remove without weakening the existing owners.
