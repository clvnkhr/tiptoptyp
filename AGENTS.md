# Agent working agreement

This repository treats UI evidence as part of the implementation, not as an
optional follow-up. The rules below apply to every agent working in the tree.

## Mandatory UI workflow

For any change that can affect layout, preview rendering, native child views,
themes, controls, menus, dialogs, or screenshots:

1. Run the focused Rust tests while iterating.
2. Capture a fresh deterministic app-window screenshot for the changed scene:

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
3. Inspect the resulting PNG, not just the command output. Check alignment,
   clipping, edges, transparency, focus state, and the changed interaction.
   The PNG must be a new capture under `.tiptoptyp/screenshots`; a stale file
   or an unrelated desktop screenshot is not evidence. When available, use
   the local image inspection tool to view it.
4. For native preview or child-window changes, also run with geometry tracing
   enabled and retain the relevant `ui.preview.bounds` lines:

   ```sh
   TIPTOPTYP_UI_TRACE=1 cargo run --release -- test.typ
   ```

   The trace reports the available, clipped egui, native, viewport, and scale
   rectangles. This is required because native child views are outside egui's
   framebuffer clip and cannot be verified by an app-window screenshot alone.
5. If the change updates a maintained visual contract, regenerate the stable
   gallery and validate it:

   ```sh
   scripts/capture-theme-gallery.sh
   scripts/capture-theme-gallery.sh --validate-latest
   ```

   Every expected PNG must be freshly written, non-empty, and decodable. Do
   not use a desktop capture API such as `screencapture`.

Do not report a UI fix as visually verified unless a fresh PNG was inspected.
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

## Required checks before handoff

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
```

For UI work, the screenshot steps above are required in addition to these
checks, even when the code-only tests pass.
