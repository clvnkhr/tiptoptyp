# Architecture follow-up — 2026-09-17

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
