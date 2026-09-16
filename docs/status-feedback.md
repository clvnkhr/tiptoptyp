# Preview and action feedback

Items 165–167, completed 2026-09-16.

The status bar now displays descriptive preview status, including “Preview ready
in X ms” after a measured build. Previously the status text was hidden behind an
icon, and interactive preview startup supplied only a zero-duration ready state.

Interactive preview enables Tinymist compile-status notifications and measures
the interval between matching start and completion notifications on the receiving
worker. This is observed compilation time, not GPU rendering time. The protocol
has no revision or duration field; coalesced work can span a notification cycle.
Generation and entry-path checks reject unrelated reports. Duplicate start/finish
reports do not reset or erase measured timing, and server readiness does not
overwrite compilation status. Startup without a measured cycle shows no duration;
positive sub-millisecond cycles display 1 ms.

Protocol reference: [bundled Tinymist v0.15.2 editor actor](https://raw.githubusercontent.com/Myriad-Dreamin/tinymist/v0.15.2/crates/tinymist/src/actor/editor.rs).

New-document, successful open, save, and auto-save actions now show feedback;
open/save include elapsed time. Existing failure feedback remains intact. These
join the existing bounded status history and existing format/export/Git notices.
No new polling, idle repaint loop, or per-frame I/O was introduced. No material
performance impact is expected; this was not a performance benchmark.

Follow-up: preview and action messages share one text slot to the left of the
status icon. The newest history entry supplies the visible message; unchanged
notices do not displace subsequent preview results. Double-click still opens
history. Git line counts use `Git: +N · ~N · -N`; the tooltip explains the symbols.
A regression checks message replacement, absence of the duplicate label, and
retention of the earlier action in history.

Preview progress text and its spinner share a centered bounding box. Wrapped text
is measured against available width; focus-wait text remains non-animating.
Explorer section headers use a flexible trailing spacer to left-align text while
keeping the whole header clickable.

## Verification

Deterministic tests cover notification timing and duplicates, entry filtering,
visible status text, successful file actions, centered geometry, static focus
waiting, header alignment, and the full-width disclosure click target. The opt-in
`real_tinymist_reports_preview_compile_cycles` test also passed against the bundled
macOS ARM64 server, observing a matching start/success cycle.

Fresh viewport framebuffers inspected:

- `.tiptoptyp/screenshots/agent-review/1789552124798-0001-main-preview-compiling.png`
- `.tiptoptyp/screenshots/agent-review/1789552134165-0001-main.png`

These verify centered progress and left-aligned headers, not composed native child
view geometry. The deterministic main scene intentionally has no measured timing;
the timed status text is covered by a semantic UI regression.

`scripts/capture-theme-gallery.sh` failed late in its native session with glutin's
macOS `context to have a current view` panic. The script restored the previous
gallery; validation of those images is not evidence of a successful refresh.
The first full test run overlapped gallery replacement and consequently failed
its gallery inventory assertion; tests were rerun after restoration.

The single-message follow-up subsequently completed the full 68-image gallery
refresh and `--validate-latest` successfully. Inspected fresh viewport
`.tiptoptyp/screenshots/agent-review/1789552866309-0001-main.png`: one message
appears left of the tick, with cursor and document counts retained on the right.
Formatting, strict Clippy, all ordinary tests (706 application tests), and all
13 xtask tests pass.

Git-color follow-up: the compact counts use the same semantic success/info/error
colors as the added/modified/deleted gutter markers. One layout job preserves
their left-to-right order within the right-aligned status bar; symbols remain
readable without relying on color. Regression coverage checks each span's color.
Inspected fresh Catppuccin Latte and Mocha `git-editor` captures:

- `.tiptoptyp/screenshots/agent-review/1789554554005-0001-main-git-editor.png`
- `.tiptoptyp/screenshots/agent-review/1789554554524-0001-main-git-editor.png`

This follow-up's complete gallery attempt stalled at `main--github-dark-default`
and timed out after 113 seconds, restoring the previous gallery. All 68 restored
PNGs validate; only the two fresh captures above verify this color change.
The color change adds constant-sized text formatting, with no new IO or repaints.
