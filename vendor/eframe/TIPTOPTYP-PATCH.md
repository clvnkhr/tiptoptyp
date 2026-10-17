# Local eframe patch

Source: crates.io eframe 0.36.2 (MIT OR Apache-2.0), from the locked dependency.

The native ownership integration publishes weak, viewport-keyed native window
bindings before UI callbacks. `window_host::window` exposes exact ownership for
root, immediate and deferred windows. Both native renderers bind their windows;
weak entries expire on native destruction and are pruned on subsequent creation.
No OS focus query, unsafe lifetime extension or global strong window registry is
used. This is a candidate for an upstream native-window accessor.

Keep this patch limited to native ownership and platform integration corrections.
Application lifecycle and rendering policy belong in tiptoptyp, not in eframe.

A second narrow correction allows a close request to run its owning UI callback
when a viewport is hidden/minimized. Deferred viewports otherwise have no logic
callback in which to cancel/commit close. Both native renderers use the same
`requires_ui` predicate; hidden idle windows remain on the logic-only path.

On macOS, the shared CGL configuration always supports transparency, even when
its root NSWindow is opaque. Glutin uses this configuration to set
`kCGLCPSurfaceOpacity` and to finalize every child window; deriving it from the
root's opacity disabled popup alpha. Individual document/popup window opacity
continues to follow its viewport builder.

Transparent Glow children start hidden and become visible only after a successful
first buffer swap. Initial visibility commands update the pending intent and
schedule its first render; explicit hidden children remain hidden. This prevents
an uninitialized surface from flashing black before preview-card content appears.
The native contract asserts the CGL opacity, per-window AppKit opacity, and
hidden-before-first-paint/visible-after-present lifecycle.

On AppKit, a deferred reveal of an inactive child uses `orderFront`, preserving
its creation-time non-activating behavior; winit's `set_visible(true)` otherwise
makes it key. Visibility intent emitted by the first callback is applied before
reveal, so first-frame dismissal cannot briefly show the child.

Both renderers reconcile a focused viewport's cached minimized flag before
visibility-based UI suppression. egui-winit avoids live minimized/maximized
queries on macOS, so a native restore otherwise leaves a prior Minimized(true)
command cached indefinitely. Native focus proves that window is restored;
unfocused/minimized windows retain their existing state. This does not issue a
restore or focus command and does not add polling.

Native platform output uses the primary caret rectangle as the input-method
candidate area in both renderers, including immediate/deferred children and
logic-only output. egui-winit 0.36.2 forwards `IMEOutput.rect` (the entire text
widget) to winit; for a scrolled source document this can extend below the screen.
The native boundary substitutes `cursor_rect` without changing the original egui
output, purpose, composition-interruption flag, clipboard or other commands.
This moves one existing rectangle; it adds no allocation, polling or repaint loop.
See [egui's output contract](https://docs.rs/egui/0.36.2/egui/output/struct.IMEOutput.html)
and [winit's candidate-area contract](https://docs.rs/winit/0.30.13/winit/window/struct.Window.html#method.set_ime_cursor_area).
