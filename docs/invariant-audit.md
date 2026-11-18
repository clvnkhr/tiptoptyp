# Invariant audit — 2026-11-08

This audit follows state and data across their mutation boundaries: document
ownership, workspace file operations, saves, background work, Overleaf sessions,
language-server coordinates, persisted settings, and recovery. It distinguishes
rules already enforced by types/checks from assumptions that needed enforcement.
It covers the application and core crate; it is not an audit of every vendored
dependency or a proof about every possible operating-system interleaving.
The settings and recovery rows include the 2026-11-13 upgrade-enforcement follow-up.

## Gaps found and enforced

| Invariant | Previous gap | Enforcement and regression coverage |
| --- | --- | --- |
| Every Overleaf storage root is absolute and normalized before filesystem effects. | Metadata/offline callers could bypass the mirror's absolute-path check; the shared Unix opener interpreted relative paths from `/`. | Validate every component in the common root opener before opening or creating anything. Tests cover relative-root aliasing and rejected traversal without intermediate directory creation. |
| Accepted sync state can be checkpointed. | A browser conflict message could exceed the journal's 8 KiB limit after consuming its pending save. | Share the message bound and reject oversized conflict events before state mutation. Regression retains source and the pending save. |
| A tree's entries agree with its derived ID/path indexes. | Callers outside the Overleaf module could mutate the entries vector directly. | Make entries private; expose an immutable slice. Index construction remains coupled to validated tree construction. |
| Cursor coordinate conversion does not panic on a stale index. | Conversion sliced supplied UTF-8 text using unchecked cached line offsets. | Check the slice before conversion. Regressions cover shortened text and a changed UTF-8 boundary. |
| Every local workspace mutation respects managed-project ownership and reserved names. | Import/Delete lacked the protections used by Create/Rename/Duplicate. | Reuse validated no-follow directory operations for import/delete; reject managed ancestors and reserved `.tiptoptyp` names. Tests retain original and outside files. |
| A validated workspace target cannot adopt a replacement symlink as new authority. | Import/Delete recanonicalized paths at execution, potentially accepting a newly substituted root. | Preserve the validated canonical root/parent and reopen through the existing no-follow machinery. A root-replacement regression verifies neither original nor outside files change. |
| Local Rename/Delete cannot race an app-owned save or block the UI waiting for it. | Separate mutation paths did not share the save's source lease. | Acquire the same canonical resource lease without waiting; report a busy file for retry. Deterministic held-lease tests cover both mutations. |
| Exhausted recovery stays exhausted until an explicit reset. | An extra `started` event reopened a spent retry budget. | Ignore starts in the exhausted phase. Regression attempts hundreds of restarts, then verifies an explicit reset restores the normal budget. |
| Source insertion requires an open editable document. | Common edit gating omitted empty workspaces and PDF/image pages, allowing invisible source mutations. | Check tab presence and document editability in the shared source-edit predicate. Test text/symbol insertion against empty, PDF, image, and editable pages. Bibliography-only import remains available independently; inserting its citation checks the exact source key and typesetting kind. |
| Rename/Delete cannot begin or commit during a document/close transaction. | Explorer bypassed the guards already used by other controls. | Guard both admission and commit; disable conflicting popup mutations. Regression holds an admitted save and verifies paths/source remain intact. |
| Deleting an open inactive file retires its page and deferred work. | Successful deletion left a parked tab, preview ownership and pending autosave alive. | Retire the exact matching parked owner after successful deletion, rebase preview ownership and cancel its deferred work. Regression retains the active document and checks the deleted page cannot be autosaved back. |
| Persisted preferences obey the same schema and ranges as Settings edits. | Startup deserialization bypassed editor validation, retaining old Tinymist Boolean modes and accepting deadline values as large as `u64::MAX`. | Validate saved input through the shared JSON boundary before constructing any session. Reject invalid values instead of normalizing them, applying defaults or rewriting the original profile. Regressions cover the old Boolean mode, other provider settings, numeric extremes, valid roundtrips and writes refused without changing storage. |
| Saved settings obey the settings editor's input budget. | Startup deserialization had no equivalent to the JSON editor's 2 MB limit. | Share a single read/write limit and bound the outer preferences envelope. Read-only preflight rejects oversized input before construction; invalid or oversized saves preserve existing storage. |
| Incompatible profiles fail before app or install side effects. | A failed load could continue with defaults, and installation could replace a working app before discovering incompatible preferences. | Startup returns a failing process status with the profile path and field, without windows, sidecars or persistence writes. `--check-settings [app.ron]` exposes the same admission independently. The installer invokes the new packaged executable before removal/copy; failed validation or execution preserves the installed app. |
| Built-in Tinymist defaults agree with the reviewed server schema. | Defaults, validation and server upgrades could drift, letting an app-authored value become invalid after an upgrade. | Share typed defaults and validation between build script and runtime. An unreviewed bundled Tinymist pin or invalid defaults fails the build. Contract tests retain the rejection of old Boolean modes and app-owned transport/entry overrides. |
| Permanent configuration failures do not consume transient retries. | Invalid Tinymist configuration could fail before spawning, spend all five restart attempts and obscure the exact cause behind a retry-limit message. | Validate before spawn, publish a distinct permanent configuration failure and stop recovery immediately with its field detail. Tests exercise no process spawn, terminal recovery, ignored stale failures and a corrected explicit restart. |
| All language-server positions use the editor's UTF-16 coordinates. | Tinymist accepted a server-selected UTF-8/UTF-32 encoding; other providers accepted malformed explicit choices. | Share encoding validation across Tinymist, TeX services, and LTeX+. Omission uses the protocol default; explicit choices must be UTF-16. Initialization regression verifies rejection precedes readiness/document opening. |
| Outbound JSON-RPC payload construction is bounded and rejection emits no partial frame. | The writer checked 32 MiB only after serializing the entire value into memory. | Bound serialization itself before writing the frame. A lazy oversized serializer verifies early termination and an untouched connection. |
| Git setup modifies only the reviewed repository and its local configuration. | The original initialize action had no author/remote review; inherited Git routing variables could override the workspace. | Review the actual root, bind the request to its configuration fingerprint, remove repository/index routing overrides and use atomic local configuration writes. Regression fixtures retain unrelated and global configuration. |
| Git commits cannot include app metadata through a backend bypass. | Private-data checks at the presentation layer did not protect direct commit admission. | Refuse staged private data under the repository lease, including case aliases. Setup also adds a repository-private exclude rule without changing Overleaf source files or rewriting history. |
| Git backup takes a locally saved, settled copy. | Initial Overleaf hydration and queued mirror writes could still be pending when staging/commit controls were available. | Gate backup actions with existing constant-time local-copy state, including actions queued behind a status refresh. Saved offline edits remain eligible even when not published to Overleaf. |
| Git source restoration respects Overleaf's document ownership. | Pull/Revert could change files behind the active model and sync journals. | Refuse Pull for a selected linked workspace and Revert for managed targets under the repository lease; keep setup, staging, commits, Fetch and Push available. Tests preserve the source while exercising a real local backup remote. |
| Timed-out Git operations do not leave task-owned helpers running. | Killing only the Git parent could leave a hook or SSH child holding output files/pipes. | Use an owned Unix process group with termination on completion, cancellation and timeout. A pipe-closure regression verifies descendants stop as well as the direct process. |
| GitHub creation uses the account, workspace and visibility reviewed by the user. | A new hosted-setup path needs to handle account or local configuration changes before an external effect. | Verify the account ID/login and local configuration again, require an empty origin, and validate the returned owner, repository ID, visibility and exact GitHub addresses before connecting. Creating the remote never commits or pushes files. |
| A retry cannot accidentally create a second hosted repository. | Losing a creation acknowledgement leaves the outcome uncertain. | Persist a bounded, atomic creation intent before POST. Recover through GET and the original account; retain the confirmed public receipt before attachment. Repeated connection attempts never repeat POST. Clearing a missing request requires an explicit check. |
| GitHub credentials stay outside project files and app logs. | The official helper can fall back to a plaintext profile when the secure store is unavailable. | Isolate the helper profile outside workspaces, reject and remove its token fields, validate secure-store status, and configure only a repository-local helper pinned to the reviewed account and address. Helper output is bounded and never copied into notices. |
| GitHub preparation and sign-in remain bounded and cancellable. | A helper download, authentication subprocess, or second window could otherwise run indefinitely or queue behind browser approval. | Verify pinned archive and executable hashes, extract only the expected regular file, bound process output/time, terminate owned process groups, and refuse a busy profile lease without waiting. |

## Existing enforcement checked

| Area | Existing enforcement retained |
| --- | --- |
| Document ownership and save completion | Save requests/receipts carry owner, epoch, revision and path identity. Stale completion cannot mark a replacement buffer clean or release an unrelated close continuation. |
| Workspace departure | Dirty-page approvals bind exact document state; departure drains admitted saves and Overleaf checkpoints before retiring old pages. |
| Active and pinned preview pages | Stable tab identities and explicit rebasing prevent a parked page or stale completion from taking over another preview. |
| Background jobs | Replaceable reads discard obsolete receivers; mutating jobs use exclusive operation ownership. Disconnects/panics become terminal failures rather than perpetual pending work. |
| UTF-16 edits | Core edit resolution rejects invalid surrogate boundaries, reversed/out-of-source ranges and overlapping edits before applying a batch. |
| Overleaf project/tree identity | URLs bind project and origin; trees reject duplicate IDs, path/case collisions, non-folder parents and invalid main documents. Source/listing budgets are checked. |
| Overleaf publication and review | Epoch/project/path/kind checks, fresh browser verification, request-matched acknowledgements and exact conflict-review versions prevent stale publication or acceptance. |
| Offline state | Exact journal checkpoint hashes reject stale writers. No-follow operations and disk comparisons preserve externally changed files. |
| Generated fixes, Git and table edits | Source-bound request identities and checked replacement transactions reject stale results. Git hunks verify baseline text and repository ownership before mutation. |
| Native preview geometry | Existing finite/positive-size checks, bounds validation and pure geometry tests remain the enforcement for native-view and snapshot requests. No geometry or rendering contract changed in this audit. |

## Performance and validation

The new state guards and recovery checks are constant-time and add no idle
repaints. Tree privacy adds no runtime work. Root checks run during existing
filesystem operations; journal validation runs at event admission. Settings
validation runs at startup, publication and save, and oversized input is rejected
before parsing. Build-time configuration checks run only while building; permanent
configuration failures add no restart worker or idle repaint loop.
Invalid form drafts cache validation until an edit or shared-settings change;
these checks have no material expected runtime performance impact.
Outbound serialization now stops at its existing payload budget. File mutation
leases never wait on the UI thread. Git setup/configuration and repository
discovery run only on explicit background jobs. Local-copy gating uses existing
state flags and queue checks without new per-frame filesystem reads. Git setup
responses bind a non-reused request ID and workspace, and repeated notice polling
does not duplicate status entries.

For the serializer change, matching optimized component builds processed the
same lazy oversized JSON sequence in fresh test processes. Observed maximum
resident memory was about 122 MiB before and 34 MiB after. The original version
failed the early-termination regression; the updated version passed. This is a
single local memory-boundary check, not a UI speed measurement. Fixture, compiler,
host, source hashes and commands are retained in
[`performance-results/lsp-payload-bound-2026-11-08.json`](performance-results/lsp-payload-bound-2026-11-08.json).

Focused regressions established failing reproductions before the boundary fixes.
Validation completed on macOS 14.6.1, arm64, on 2026-11-08:

- Required formatting, strict Clippy, full Rust and xtask checks passed. The main
  app suite passed 3,367 tests, with 83 optional tests ignored. Strict Clippy also
  passed with `desktop-ui-tests` enabled; the xtask formatting check passed.
- The initial audit's native Explorer, empty-workspace, workspace-switch,
  Overleaf-session, citation-import and TeX-intelligence journeys passed.
- The final Git build passed `git_setup`, `git_github` and `git_hunks` through
  native input. These verified an offline Overleaf backup, first commit/push,
  configuration review and cancellation, stale-account refusal, workspace-bound
  outcomes, and interrupted creation recovered without another POST.
- All 431 desktop script contracts and six isolated GitHub provider/transport
  contracts passed. Official helper archives for all six supported targets were
  checked against the pinned archive/executable hashes and extracted with the
  app's extraction implementation.
- Fresh local-remote and GitHub setup viewport framebuffers were inspected for
  alignment, clipping, focus and control visibility. Native hit rectangles were
  checked separately. These captures do not establish composed native preview
  geometry; this work changes no preview bounds or maintained gallery contract.

GitHub protocol tests use an owned synthetic CLI and loopback provider, with
actual disposable local Git remotes. They do not establish real browser OAuth
consent or system credential-store behavior, and they do not read or change a
real GitHub account or repository. See [desktop UI journeys](desktop-ui-journeys.md)
for reproduction commands and the distinction between each kind of evidence.

External writers can still change a filesystem after an OS operation's final
precondition check; portable APIs do not provide a universal compare-and-unlink
transaction. The app uses no-follow handles, identity comparisons and cooperative
resource leases rather than treating a prior path check as sufficient authority.

## Upgrade enforcement verification (2026-11-13)

On macOS 14.6.1 arm64 with Rust 1.98.1, formatting, all-target Clippy with warnings
denied, the full Rust suite and the xtask suite passed. The 484 Python contracts
and 11 isolated installer tests also passed. Negative build probes verified a
Boolean typed default fails compilation, invalid serialized defaults fail the
shared validator, and the actual compiled build script rejects an unreviewed
Tinymist version. Temporary probes left repository contracts unchanged.

The real bundled Tinymist accepted the built-in configuration, initialized,
opened its preview and compiled successfully. Read-only preflight also accepted
the current production preferences. Regression tests reject the old Boolean
mode, malformed/oversized envelopes and named pipes without writing preferences;
they verify internal publication guards, retained form drafts, nested preference
merges, permanent failures without retries and installer preservation.

Real native input passed `tool_defaults` and `comment_controls_settings` with
the preserved desktop-test build. Receipts are in
`.tiptoptyp/desktop-ui-tests/run-r4t_prc_/result.json` and
`.tiptoptyp/desktop-ui-tests/run-ju7mccr0/result.json`. These checks verify
interactions and observed state; no visual composition verification is claimed.
