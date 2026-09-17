# Tab drag diagnostics

Run the current source with the dedicated trace enabled:

```sh
TIPTOPTYP_TAB_TRACE=1 cargo run --release 2>&1 | tee /tmp/tiptoptyp-tab-drag.log
```

Open two or more tabs, drag one by its filename, release it, then try the other
direction. Keep the pointer near the tab row. Share the `ui.tabs.*` output,
including the startup line and the complete failing gesture. If release output
is missing, click once more inside the window to flush the pending gesture.
This launch does not require the broader `TIPTOPTYP_UI_TRACE` flag.
`tee` both displays the output in the terminal and writes the file. `/tmp` is
outside the repository; inspect it with `ls -l /tmp/tiptoptyp-tab-drag.log`.

- `ui.tabs.trace`: confirms the helper is enabled and identifies the process
  and executable. No banner means this instrumented code has not reached the
  tab strip (or the environment flag was not passed to that process).
- `ui.tabs.press`: the tab strip saw a primary press. If the banner appears but
  this line does not, investigate native input delivery or an overlapping window.
- `ui.tabs.native-window-drag`: the toolbar requested native window movement.
- `ui.tabs.sample`: viewport, time, pointer/press origin, button state, drag
  threshold, focus, egui drag owner, captured source, reorder outcome and tab order.
  Trace version 2 also reports the native window rectangle and
  `native_drag_suppressed`, distinguishing window movement from pointer movement
  within the window.
  Up to 16 titles include widget IDs, rectangles, clips and hit/drag/click flags.
  `no-source` means the tab never owned the press; `below-threshold` means the
  pointer has not qualified as a drag; `no-drop-target` means the source ID or
  vertical drop bounds did not match; `same-slot` means the order is unchanged;
  `reordered` means the model accepted the move. `no-space` identifies a strip
  with no available width.
- `ui.tabs.gesture`: flush reason and the number of samples omitted by the cap.

Diagnostics never include document names, paths or text. The startup banner
does include the executable path. Each window retains at most 32 samples, with
movement sampled at most every 100 ms. Press/release samples bypass throttling;
buffered samples are printed on release, a later press, detected lost release,
or a native window-drag request. There is no per-frame output, trace file I/O,
timer or requested repaint. Disabled tracing performs a cached flag check and
does not build diagnostic strings or allocate a sample buffer.

## Native title-bar failure, 2026-09-17

The user trace showed a captured tab, `dragged=true` and `same-slot`, while the
window-relative pointer moved only about two points. No application `StartDrag`
request appeared. The user confirmed that the entire window moved: AppKit's
native title-bar dragging was independent of egui's gesture ownership.

The app now temporarily sets that document's `NSWindow.isMovable` to false on
tab hover, before mouse-down, and throughout the gesture. Leaving the tabs,
ending the gesture outside them, losing focus, replacing the viewport or dropping
the session restores the previous policy. Empty toolbar space retains normal
window dragging. A retained guard changes AppKit properties only on transitions;
it adds no repaint loop or background work.

Pure policy tests cover hover, continued drag outside the row/viewport and
release. A separate main-thread AppKit check creates two unshown windows and
exercises the actual adapter, checking suppression, restoration, an initially
immovable window and sibling isolation:

```sh
TIPTOPTYP_NATIVE_DRAG_TEST=1 cargo test --test native_window_drag
```

This check requires a macOS desktop session and is skipped by default. It
passed locally; it checks native properties, not a simulated desktop mouse drag.
The pointer harness continues to cover tab ordering and document identity.
