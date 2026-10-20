# Protected background saves — todos 189–190

## Ownership and behavior

`src/app/saves.rs` is the shared owner-local admission/completion adapter for
manual saves, Save As, post-format writes, active autosave and parked-tab autosave.
`src/save_io.rs` performs hashing, disk-baseline verification, staging, replacement
and durability confirmation. It uses the existing `ExclusiveJob`, not another
executor. A window admits at most one save and owns no pending payload queue.
Duplicate submission is rejected while it is busy; later dirty edits retain an
autosave deadline. Parked tabs are submitted one at a time instead of writing all
of them synchronously in one frame. Idle windows neither start workers nor poll
on a repaint timer for saves: completion wakes the owner.

The existing canonical SaveInput/SaveReceipt handoff is consumed once. The UI
retains routing metadata (stable tab ID, source key, destination, continuation and
format policy), not another source copy. Immutable document snapshots remain
alive until completion. Two windows may have saves in flight, but checks and
replacement for the same canonical destination share the app-owned resource
lease. A second request with an obsolete baseline becomes a conflict, not a
silent overwrite. A confirmed overwrite carries the disk observation the user
approved; a later change requires a new confirmation.

`file_versions::write_checked` checks and commits under that same canonical
resource lease. On macOS it coordinates the transaction with `NSFileCoordinator`,
uses metadata-preserving atomic replacement, and creates native file revisions.
Other platforms retain `AtomicFileWriter::write_checked`; existing unrelated
writer clients share the same resource lease via `AtomicFileWriter::write`.
This is not a universal OS lock: external programs that ignore macOS file
coordination (or the in-process destination lease) can still race the write. Save As/unknown explicit baselines remain explicitly unchecked;
automatic saves require a known baseline. There is no automatic conflict merge.

Save completion is now asynchronous; submission success does **not** mean bytes
are durable. The status reads “Saving…” until a result arrives. Workflow dispatch
cannot discard a pending save-before-close continuation. Only the matching,
durable receipt with no newer dirty edits releases it. Failed/uncertain writes
do not close a document. A replacement document never accepts an old receipt.
Formatting starts only after a successful, unchanged manual save; Save As waits
for the replacement Tinymist session. Autosave and the post-format persistence
pass do not request formatting, avoiding a save/format loop.

Completion follows the existing stable tab ID through parking and reordering.
An inactive saved tab cannot format or close the current tab. Core owner/epoch/
revision receipt checks remain authoritative. The current tab implementation
rekeys a document when reactivating it: if that happens before its save returns,
the stale receipt is deliberately not applied. The disk result is reported, but
the reactivated buffer is not falsely marked clean. This conservative limitation
remains for the document-store work in 184–186; it is not a background-write
cancellation or an excuse to bypass receipt validation.

Closing a window does not cancel an admitted mutation. ExclusiveJob retains its
outcome for the process shell, and its active-operation lease prevents premature
process exit. Closing the retained root detaches the save before resetting that
document. Deferred outcomes contain the path and success/error/durability status,
not document contents. The save path reuses this established lifecycle.

## Deterministic evidence

- A writer blocked by a channel under the destination lease leaves UI dispatch
  and an egui frame runnable. A duplicate owner request cannot start a second job.
- Dropping that owner while blocked still persists the bytes and delivers the
  completion summary to the shell.
- Two same-path/alias requests with one baseline serialize: the first succeeds,
  the second observes the first's bytes and refuses the stale write. Missing-file
  checks distinguish a permitted creation from a later unexpected replacement.
- Editing during a delayed save preserves newer dirty text and cannot release
  close. A durable unchanged save releases exactly one close action.
- Conflict and repeated-confirmation tests preserve externally changed bytes.
- A parked/reordered tab receives its own receipt without changing or formatting
  the active tab. Projected active and ordinary parked autosave tests pass.
- Save As does not update the path or schedule formatting before completion;
  autosave never formats. Late results cannot mutate a replacement or consume its
  newly installed close continuation.
- Existing core receipt, failed/uncertain durability, private atomic-write and
  protected-operation tests remain. This change does not alter UI geometry or
  paint; no fresh visual verification is claimed.

## Optimized foreground-dispatch measurement

Environment: Apple M2 Max arm64, macOS 14.6.1 (23G93), Rust 1.96.0. Worktree based
on `53d315acc28fabf9a82e56dec3dbcfcba826916e` plus this implementation. Both paths
run in one optimized profiling binary with identical 128-KiB text, destination,
baseline check, atomic writer and a 10-ms injected storage delay under the lease.
Each path has three warmups and five measured samples; fixtures are prepared
outside timing. Synchronous samples precede background samples. No concurrent
build ran during measurement. No viewport/theme applies to this headless probe.

```sh
RUSTFLAGS=-Cforce-frame-pointers=yes cargo test --profile profiling --features profiling --bin tiptoptyp save_io::tests::save_dispatch_cost_probe -- --ignored --nocapture
```

| Sample | Synchronous dispatch / total (ns) | Background dispatch / total (ns) |
| --- | ---: | ---: |
| 0 | 20672000 / 20672041 | 16916 / 20933125 |
| 1 | 20716750 / 20716792 | 21333 / 24753125 |
| 2 | 21791667 / 21791917 | 26667 / 21644333 |
| 3 | 22198834 / 22199292 | 12542 / 22878583 |
| 4 | 22431833 / 22432208 | 14417 / 24774209 |

Median dispatch: **21.792 ms synchronous, 0.0169 ms background**. Median total:
**21.792 ms synchronous, 22.879 ms background**. Offloading makes the caller
responsive; it does not make storage faster. Thread admission and completion
costs still exist. This isolates the disk/dispatch boundary, not GUI FPS, input
latency across platforms, CPU sampling, GPU memory or storage throughput.
Source snapshot/projection preparation and path normalization still occur before
dispatch and are outside the probe; no claim of zero foreground work or IO is
made. In particular, path canonicalization can touch filesystem metadata.

Measured SaveIO/probe source SHA-256:
`838a79ddafdbf800a9a0a6b3e17d66f82b060963646562d4f08f06ba8f9c4944`.
Executable SHA-256:
`01311770bbb517612bfabfb6af132d61f49e3d74b6e31794d466f06a91ea3d5d`.
The probe is ignored in ordinary test runs; channel-controlled regression tests
assert ordering and bounded admission instead of flaky performance thresholds.

## Cross-owner follow-up (2026-09-20)

The bounded item-256 shell regression now holds the existing canonical resource
lease around an admitted save while switching document owners and tabs. The
save is completed only after the lease is released, so the scenario checks the
real `ExclusiveJob`/receipt path without adding a second executor or queue.
Late language-service data is checked separately by document identity; it cannot
release, overwrite or otherwise mutate another owner's save state. This is
ownership evidence, not a new save timing claim; the integrated measurements
and their CPU/GPU limitations are recorded in [`performance.md`](performance.md).

## Native local versions (20 October 2026)

The existing save worker now owns native macOS version creation too; no extra
executor, per-frame filesystem polling, source queue, or repaint timer was added.
Original disk contents are preserved before their first overwrite. Automatic
snapshots coalesce to one per minute after that initial preservation, explicit
saves preserve changed content immediately, and identical latest revisions are
not copied. Ownership recognition is bounded to 128 metadata-only path records.
A changed or unrecognized disk baseline still receives a safety snapshot.

Both original and committed revisions remain associated with the source across
`NSFileManager` replacement. The same lease covers checks, staging, replacement,
and checkpoints. A callback URL redirected by file coordination is rejected
before invoking the checked operation. Backup-store errors are separate from
save receipts and appear in ordinary and detached completion notices. They
cannot turn a successful source write into a false save failure.

Restoring a historical version first checkpoints the current canonical buffer,
including dirty text, without changing the disk file. Once the owner/key/path
still match, the UI applies an undoable replacement and submits an ordinary
explicit save. Normal conflict, durability, and receipt rules continue to apply.
The comparison and its historical PDF never expose a private source mirror as
an editable document. Historical PDF source is frozen; imported files and
assets resolve from the current project. See
[local-file-versions.md](local-file-versions.md) for the complete contract.

### Save-worker cost

An isolated optimized Rust harness includes the repository's actual
`file_versions`, `private_workspace`, and `resource_lock` modules. It compares
the prior atomic writer, native automatic saves, and native explicit saves with
an identical 128-KiB file and one changed tail byte per write. Modes are
interleaved within one process. Each mode has five warmups and ten measured
samples; the first write is reported separately. There is no viewport/theme
because the workload consists of background disk operations, not painting or
foreground dispatch.

A clean final run on an arm64 Mac, macOS 14.6.1, Rust 1.98.1 measured warm
medians of 9.40 ms for the prior writer, 11.05 ms for native automatic saves
(+1.65 ms of worker I/O), and 15.21 ms for explicit saves. Fifteen automatic
writes created two revisions (original plus first captured save), while explicit
writes created sixteen. First-write observations were 10.75 ms, 22.55 ms and
14.69 ms respectively; these are separate from the warmed comparison. No build,
gallery or native journey ran concurrently with this final measurement.

The [checked-in receipt](performance-results/local-file-versions-2026-10-20.json)
contains every measured sample, exact source/probe hashes, the isolated harness,
run metadata and verification results. It supersedes the preliminary concurrent-
build timing. Raw local receipts and the rebuilt probe are also retained under
`.tiptoptyp/file-versions-audit/`. The harness includes production source modules;
its path imports point at this checkout and can be adjusted when reproducing it
elsewhere. It has no app viewport or theme.

This measures source-save worker I/O on one local filesystem. It does not
measure main-thread input latency, GUI FPS, cold application startup, revision
storage on network volumes, or cross-platform behavior. Native version creation
adds disk work; offloading keeps that work outside the editor's frame loop.
