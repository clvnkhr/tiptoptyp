# Local glutin 0.32.3 patch

Source: crates.io glutin 0.32.3 (upstream MIT/Apache-2.0 licenses retained).
Original crate SHA-256: `12124de845cacfebedff80e877bb37b5b75c34c5a4c89e47e1cdd67fb6041325`.
The CGL module permits deprecation warnings because implementing this adapter
necessarily calls Apple's deprecated OpenGL API.

`ContextInner::is_view_current` now checks `NSOpenGLContext::currentContext`
in addition to the attached NSView. AppKit can invoke a layer-backed redraw
with another context current. The upstream surface-only predicate lets eframe
skip rebinding, and the subsequent texture allocation returns zero and aborts.
A detached view returns false instead of panicking. No renderer fallback is used.

Reproduction: the single-session gallery's light File menu → dark File menu →
light Edit menu sequence. Validate with `scripts/capture-theme-gallery.sh`.

Native context regression (no windows):
`cargo test -p glutin --lib native_thread_current_context -- --ignored`.
