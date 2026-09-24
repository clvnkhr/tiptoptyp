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
