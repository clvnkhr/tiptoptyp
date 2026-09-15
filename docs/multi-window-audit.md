# Multi-window audit (todo 159)

Audited on 2026-09-15: process/native-menu dispatch, Finder-open delivery,
document-window lifetime, focused child ownership, shared settings/history,
close coordination, background completion routing, and explicit root viewport
references.

## Fixes

- Refresh the active document from current viewport focus before dispatching
  native commands. A document's Settings and other scoped children count as
  that document's focus; a hidden root's stale focus does not.
- When the active secondary closes, select a surviving document instead of
  unconditionally selecting the hidden root.
- Dock reopen focuses and restores an existing document, including a minimized
  one. Only when no document remains does it reveal the retained root. Native
  document commands do not steal focus from an already visible Settings window.
- With no documents, New, Open, New Window, Open in New Window, Settings, and
  process Quit remain usable. Document-only menu commands are disabled and
  ignored if already queued. Clipboard commands still work in a focused child
  such as Settings, without revealing the hidden document host.
- An accepted root-window close replaces its document with a clean untitled
  buffer. Discarded source, file destination, undo history, and autosave deadline
  cannot survive the close. The document epoch changes, rejecting late results
  and save receipts for the closed document. Dirty primary quit confirmations
  explicitly reveal their owner when needed.
- Finder-open sends explicitly wake the root process dispatcher. Startup events
  remain queued until the shell exists; disconnected sends do not request a
  repaint.
- Merge every pending Settings edit against its owner's applied settings,
  inactive owners first and active last. Independent field edits survive;
  active-owner precedence applies only to conflicting fields. Existing MRU and
  explicit workspace-removal rules remain intact. No settings snapshot is
  cloned by synchronization when no window has submitted an update.
- Detached process-operation completion notices go to the active owner rather
  than always to the primary document.

## Root references that are intentional

The eframe root remains the process/event-loop host, not necessarily the active
document. Native menu/open callbacks and exclusive background-operation
completion wake that host. Process Quit is coordinated across document keys
before closing it. The root frame counter in `viewport_fonts` gates shared font
atlas maintenance once per shell frame. Screenshot batch orchestration and
profiling also belong to the process host. Document workers, dialogs, preview
parents and child IDs retain their own session/viewport ownership.

## Verification and limits

Focused deterministic tests cover surviving-window selection, current document
and child focus before owner paint, no-document command policy, independent
settings edits/conflicts, startup and live Finder-open delivery, root-only wakeup,
failed-send silence, and discarded-buffer/epoch/autosave/history reset. Existing
tests cover close transactions, owner-specific async completion, scoped child
input, settings-child scrolling, and one-shot window activation.

Required checks: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test --no-fail-fast`, and
`cargo test --manifest-path xtask/Cargo.toml`.

These are state/routing changes, not a visual-layout contract change. No native
Dock/Finder interaction or composed desktop screenshot was exercised in this
pass; headless focus/event tests do not prove AppKit focus behavior end to end.

No material steady-state performance change is expected and no CPU/FPS claim is
made. Focus selection is bounded by window count and the fixed owned-child set;
there are no new timers or continuous activation/repaint requests. Open-event
wakes and preference merges are event-driven. Closing the retained root uses
the existing untitled-document service reset once, not per frame.

One architectural follow-up remains (todo 160): the hidden root still hosts an
EditorApp and its services. Suspending document-only rendering, workspace scans,
and compilation there needs a distinct dormant-host lifecycle which must retain
Settings, process commands, pending dialogs, and completion delivery. Profile
that no-window workload before and after implementing it; do not simply skip
the root callback, which also pumps these application-level responsibilities.
