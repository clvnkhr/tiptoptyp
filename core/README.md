# tiptoptyp-core

Headless document and orchestration rules. Run `cargo test -p tiptoptyp-core`
without a desktop, sidecar binaries, network, or filesystem fixtures.

- `document`: only edit transactions/history commands mutate source. Immutable
  snapshots bind source to a window/document version. A save receipt records the
  bytes actually written; stale completions cannot regress persisted metadata or
  release a pending continuation.
- `workflow` and `closing`: exclusive document-operation phases, consumed save
  continuations, and all-window close approval. Any canceled window or changed
  approved version prevents the batch from closing. The runtime also supplies
  whether protected background operations are still pending before process exit.
- `preview` and `connection`: artifact/raster provenance and generation-owned
  readiness. Old display content can survive a restart without becoming current.
- `text` and `geometry`: distinguish byte, scalar, UTF-16 and native coordinates.
- `scheduling`: callers supply time; tests advance `Duration` values directly.

Keep GUI, filesystem, thread scheduling and process operations in application
adapters. `tests/architecture_boundaries.rs` checks this rule and protected
viewport/font calls. Adding a dependency to this crate requires reviewing that
explicit dependency allowlist. The checks are architectural rules, not a sandbox.

`tests/contracts.rs` drives the public state-machine contracts through edit/history,
save ordering, workflow, multiwindow close, preview, connection, text, geometry,
and scheduling edge cases. `tests/save_close.rs` drives the real
document/workflow boundary through canceled, failed, uncertain, stale and
successful writes. Add adversarial event sequences there when changing save
policy; retain real adapter tests in the application.
The `test-support` feature exposes snapshot fixtures for application tests and
is enabled only through the application's dev dependency.
