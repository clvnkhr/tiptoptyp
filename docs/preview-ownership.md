# Preview ownership after extraction step 5

Audit baseline: `74cf297` (steps 1–4 committed). This is an ownership inventory,
not a proposal for another executor or state machine.

| Policy/state | Decision owner | Adapter work that remains |
| --- | --- | --- |
| Process generation | Tinymist service; `TinymistSync::begin/finish_stop` owns the document-sync association | Start/stop the process, associate its returned generation with recovery, connection and sync. These are distinct consumers of one generation, not extra generation counters. |
| Five-failure budget and retry deadline | Core `Recovery`, via `PreviewController::transition` | Apply stop/restart/raster effects and request the specified repaint deadline. No second counter or timer. |
| LSP initialization and preview endpoint acceptance | `PreviewController::initialized/preview_ready`, using core `Connection` | Synchronize documents after accepted initialization; request native navigation after an accepted endpoint. |
| Visibility transitions | `PreviewController::visibility_changed` | Supply document/view/backend facts, hide the native child when needed, apply returned effects. Unchanged visibility returns an empty, non-allocating effect vector. |
| Retaining a surface across restart | Existing `retain_preview_surface_for_restart` predicate | Suspend connection readiness; retain a display only when requested and backed by both an endpoint and native child. The retained URL never establishes readiness. |
| Native view and cached identity teardown | `native_views::discard_webview` | Call from document/window closure, non-retained restart, local failure and changed native parent. Construction, bounds and visibility remain on the owning UI thread. |
| Raster content/identity | `PreviewController` and core `PreviewContent`, keyed by artifact/document identity | Deliver compiler/PDF results, invalidate on edits/Save As, clear on document replacement. Views read content; these existing paths were not rewritten. |
| Open URIs, versions and backing files | `TinymistSync` | Collect source only for a request/change, execute its returned open/change/close/backing-write batch, confirm successful opens. Stop keeps backing files alive through didClose. |

## What changed

- Removed app-side readiness/status/recovery sequencing in favor of controller
  admission methods. The old code called `recovered` before parsing or admitting
  the endpoint, which could incorrectly reset the consecutive-failure budget.
  Invalid endpoints now use the existing preview-failure/retry path. A stale,
  uninitialized, suspended or LSP-only notification cannot request embedding.
- Removed the duplicate Starting-event status/completion reset: the launch adapter
  already performs it before starting the process.
- Moved visibility-transition memory into the controller (now private), preserving
  restart-before-render effect order and the existing no-work idle behavior.
  Suspension also clears enabled/visible flags: a failing-before/passing-after
  regression caught stale enabled state repeatedly requesting restart on later
  hidden visibility checks. The fixed path emits no effects across 100 checks.
- Removed repeated native teardown blocks and repeated no-document connection/status
  cleanup. Retained-surface restarts still clear their pending reload flag without
  destroying the native view. Equal URLs from replacement servers still cause a
  navigation once an endpoint is accepted.
- Removed the redundant `tinymist_session_requested` alias. Tool resolution,
  startup errors and actual native operations remain adapter responsibilities.

## Audit of steps 1–4

The reviewed boundaries preserve Explorer IDs, clipping, section persistence and
effect ordering; search focus is distinct from source-navigation focus; saved
receipts still target stable tab identity rather than the active slot. Disk leases,
epoch checks, durability and conflict confirmation were not relaxed. The parked
save regression now checks both cancellation of the original close continuation
and preservation of a newer active-tab continuation. No introduced regression was
found in this audit; this does not establish that every native interaction is sound.

## Evidence and limits

Validation passed: 1,069 full-suite test executions (16 opt-in tests ignored),
13 xtask tests, formatting and strict all-target Clippy. The focused preview suite
has 18 tests. The audit baseline also passed 14 navigation, 30 Explorer and
27 save tests (2 opt-in save tests ignored).

Focused coverage includes malformed endpoints through five failures, pre-init and
stale readiness, a retained endpoint rebound to a replacement generation at the
same URL, LSP-only readiness, suspension, and 100 unchanged visible/hidden updates
without effects. A dependency test keeps readiness admission and native teardown
out of their old duplicate locations. Existing recovery, sync, artifact, workspace,
save and multi-window lifecycle tests remain in the full suite.

No native bounds, visual styling or profiling instrumentation changed. No new
worker, retry counter, source copy, disk polling or idle repaint was introduced.
No material performance impact is expected; no matched native timing comparison
or visual verification is claimed. Step 2's macOS WebView-to-editor focus acceptance
remains explicitly unverified.

Accounting for this step: app.rs 12,400 → 12,341 (−59). Other production deltas:
preview controller +75, core connection +3, native adapter +13, workspace adapter
−10: **+22 production lines overall**, including the readiness and suspension fixes.
Tests grow by 179 lines. This is policy consolidation and a correctness fix, not
a total-source reduction or measured speedup.
