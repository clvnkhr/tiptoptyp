# Source editor interaction regressions

The 22 September 2026 newest-first pass closes todos 301–296.

Diagnostic hover cards carry their document revision, diagnostics generation,
and logical line. A retained native hover is dismissed when its document changes
or its diagnostic disappears/changes. An identical diagnostic in a new publication
keeps the card. Invalidating a card also clears its handoff geometry, focus state
and hover delay; unchanged frames do not rebuild diagnostics or scan their list.

Source resize anchoring retains one reference to the last laid-out galley per
document viewport. A width/height change keeps a visible caret at the same screen
height, clamped to the remaining viewport. If the caret is offscreen, the logical
line at the previous viewport center anchors the reflow. Explicit navigation and
edits take precedence, and a different document/revision cannot reuse the anchor.
ScrollArea's committed offset is included in the saved geometry so repeated
resize frames do not accumulate drift. Text still respects the normal document
top/bottom scroll bounds. No second layout, background worker or idle repaint
loop is added.

Find and Replace use viewport-scoped field IDs. Their shortcuts focus an open,
unfocused bar; repeating the shortcut while that bar is focused closes it.
Invoking Replace from a focused Find-only bar first exposes replacement controls.
Closing with a shortcut, Escape or the close button retains the selected search
match. Reopening/refocusing restores that match and navigation continues from it.
Changing the document/query still invalidates the revision-keyed search session.
The counter shows the one-based selected match and total; `0/0` means no matches,
and `0/n` means no result from that result set has been selected yet. Invalid
regular expressions retain their explicit error label.

Sticky context uses egui's Middle order; Find/Replace uses Foreground. Paint-call
order alone was insufficient because clicking an Area promotes it within its
order. The regression explicitly promotes the sticky Area and checks the topmost
hit-test layer in its overlap with Find. Icon buttons also expose their existing
tooltip labels to accessibility, allowing the semantic tests to use real controls.

## Checks

Deterministic coverage exercises resolved/edited/replaced/unchanged diagnostic
cards; repeated 800 → 500 → 400 → 650 → 800 point editor resizing with wrapped and
unwrapped Unicode text and visible/offscreen carets; stale/no-resize anchors;
Find and Replace refocus/close/reopen, current-match counters and wraparound;
and one cached search scan across navigation. The existing source-jump, clipboard
and secondary-window tests render Find only through the real editor overlay,
removing their former duplicate rendering of the same bar in a different layer.

The full suite passes: formatting, all-target Clippy with warnings denied,
1,175 Rust tests (21 opt-in tests ignored), 14 xtask tests, and 5 PDF.js host tests.
The real Chrome PDF.js probe also confirms that older todos 118 and 1 are already
implemented: 23 search matches, all 24 thumbnail entries, a decoded thumbnail,
thumbnail navigation and exact same-document page/zoom/scroll restoration. See
`docs/pdfjs-preview.md` for the viewer's ownership and platform limits.

Verification logs are under `.tiptoptyp/todo-evidence/`. The new
`find-sticky-context` capture is targeted-only; the maintained gallery remains
22 images (19 light and 3 dark). Viewport framebuffers do not prove native child
composition or physical mouse/trackpad behavior.

The fresh light `find-sticky-context` framebuffer
`.tiptoptyp/screenshots/agent-review/todo-find-sticky/1790077928756-0001-main-find-sticky-context.png`
was inspected: both search rows cover the sticky header, the controls are unclipped,
and the current result reads `1/4`. The refreshed standard Find frame shows `0/2`
because that fixture has not selected a result yet.

An opt-in release probe isolates anchor preparation from rendering on macOS
14.6.1 arm64, Rust 1.98.1, thin LTO/one codegen unit. A 10,000-line fixture
(20,001 visual rows) uses bundled monospace at 13pt/1x, a 500-point wrap width and
500×600 viewport. After 100 warmup iterations, 10,000 unchanged-size checks took
41.417 µs versus 41.375 µs for galley-reference retention alone. The resize path
took 102.448 ms versus 41.458 µs for reference retention, about 10.24 µs added
per resize. These are geometry microbenchmarks, not whole-frame, idle CPU, cold
startup or native composition measurements, and establish no cross-platform
performance guarantee. Run with:

```sh
cargo test --release --bin tiptoptyp measure_resize_anchor_preparation -- --ignored --nocapture
```
