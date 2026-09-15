# Agent working agreement

This repository uses proportionate UI evidence. Prefer fast deterministic
tests for behavior and capture screenshots only when pixels are material to
the change. The rules below apply to every agent working in the tree.

## Compatibility policy

tiptoptyp is greenfield software. Do not retain deprecated names, settings
keys, environment variables, temporary-file conventions, or compatibility
aliases unless the user explicitly asks for a migration path. Prefer a clean
break and remove the obsolete behavior completely.

## Risk-based UI workflow

For changes that affect UI behavior, layout, preview rendering, native child
views, themes, controls, menus, dialogs, or screenshot infrastructure:

1. Run the focused Rust tests while iterating.
2. Prefer pure geometry/state tests and semantic `egui_kittest` coverage.
   Routine behavior changes, refactors, and fixes with adequate deterministic
   coverage do not require a screenshot.
3. Capture a fresh deterministic viewport framebuffer only when correctness is
   genuinely visual—for example alignment, clipping, rounded edges,
   transparency, color/theme appearance, or native-view composition:

   ```sh
   cargo run --release -- \
     --ui-theme catppuccin-latte \
     --ui-snapshot-scene main \
     --ui-screenshot-subdir screenshots/agent-review \
     --ui-screenshot-exit \
     test.typ
   ```

   Replace `main` with the affected scene. The accepted scene and framebuffer
   target contract is documented in `docs/ui-qa-screenshots.md`.
   These PNGs are not a true desktop screenshot: a native child viewport is
   captured separately and is not composited with the root app window.
4. When a PNG is captured, inspect it rather than relying on command output.
   Check alignment, clipping, edges, transparency, focus state, and the changed
   interaction.
   The PNG must be a new capture under `.tiptoptyp/screenshots`; a stale file
   or an unrelated desktop screenshot is not evidence. When available, use
   the local image inspection tool to view it.
5. For changes to native preview bounds, clipping, or child-window placement,
   use geometry tracing and retain the relevant `ui.preview.bounds` lines:

   ```sh
   TIPTOPTYP_UI_TRACE=1 cargo run --release -- test.typ
   ```

   The trace reports the available, clipped egui, native, viewport, and scale
   rectangles. Native child views are outside egui's
   framebuffer clip and cannot be verified by a viewport framebuffer alone.
   Do not describe a viewport PNG as proof of the composed desktop geometry;
   use an approved whole-window observation when available, or report that
   composition could not be visually verified.
6. If the change updates a maintained visual contract or screenshot machinery,
   regenerate the stable gallery and validate it:

   ```sh
   scripts/capture-theme-gallery.sh
   scripts/capture-theme-gallery.sh --validate-latest
   ```

   Every expected PNG must be freshly written, non-empty, and decodable. Do
   not use a desktop capture API such as `screencapture`. The gallery command
   intentionally captures its complete matrix from one app session; do not
   replace it with a per-image launch loop.

Do not claim visual verification unless a fresh PNG was inspected.
If the environment cannot launch the native app, report the exact command and
the missing GUI capability instead of implying that visual QA passed.

## Testability and observability

- Put geometry, sizing, scene routing, and state-transition rules in small
  pure functions or data structures so they can be tested without a desktop.
- Add a regression test for every brittle UI invariant: panel bounds, minimum
  sizes, clipping, alignment, transparency, scene-to-viewport routing, and
  screenshot naming.
- Prefer semantic/accessibility UI tests with `egui_kittest` for interactive
  controls. Use screenshot captures for visual relationships that semantics
  cannot express.
- Use `TIPTOPTYP_UI_TRACE=1` when diagnosing layout or native-view problems;
  keep the default runtime quiet.
- Preserve unrelated working-tree changes and never replace user edits with
  fixture or screenshot output.

## Performance working agreement

Performance is an ongoing requirement, not a one-off cleanup. See
`docs/performance.md` for profiling commands and interpretation.

- Consider idle repaint frequency, per-frame allocation/work, cache invalidation,
  main-thread blocking, background-job duplication, and scaling with document or
  workspace size when changing relevant code. State when a change has no material
  performance impact rather than running unrelated benchmarks.
- For performance-sensitive changes, record a baseline and an after measurement
  using the same optimized build profile, fixture, viewport/theme, warmup, and
  workload. Preserve the run metadata. Separate idle, active interaction, cold
  startup, and cache-hit/miss measurements; do not compare unlike runs.
- Prefer deterministic regression tests for algorithmic invariants (bounded
  work, cache reuse, no idle repaints, one worker per request) over flaky wall-time
  thresholds. Add a focused regression for each performance bug.
- Keep profiling opt-in and bounded. Never add per-frame disk I/O, unbounded event
  buffers, artificial repaint loops, or document contents to performance logs.
  Use native CPU samples for hot stacks; inclusive wall-time spans are not CPU
  time and must not be summed across nested scopes.
- Check the profiling feature with `cargo clippy --all-targets --features profiling
  -- -D warnings` and `cargo test --features profiling --no-fail-fast` when changing
  instrumentation. Run `cargo fmt --manifest-path xtask/Cargo.toml -- --check`
  when changing the profiling runner.
- Report the measurement, environment, and remaining limitations. A faster local
  run does not establish a cross-platform performance guarantee. GUI profiling
  needs a desktop and is not replaced by headless CI timing assertions.

## Required checks before handoff

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
```

Screenshots and geometry traces are additional checks only when the risk-based
workflow above calls for them.
