# Local eframe patch

Source: crates.io eframe 0.36.2 (MIT OR Apache-2.0), from the locked dependency.

The native ownership integration publishes weak, viewport-keyed native window
bindings before UI callbacks. `window_host::window` exposes exact ownership for
root, immediate and deferred windows. Both native renderers bind their windows;
weak entries expire on native destruction and are pruned on subsequent creation.
No OS focus query, unsafe lifetime extension or global strong window registry is
used. This is a candidate for an upstream native-window accessor.

Keep this patch limited to native ownership. Application lifecycle and rendering
policy belong in tiptoptyp, not in eframe.

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
