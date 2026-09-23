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

## Toolbar and external file drops (2026-09-23)

Find, Pause/Resume, Compile and Settings default to vector icons. Settings →
Editor → Toolbar buttons selects Icons only, Text and icons, or Text only.
Accessible names and shortcut tooltips remain available in each mode. The
Compile gear animates while compilation is active; it does not request idle
repaints, and deterministic captures keep it still.

On macOS, the pinned winit backend does not send pointer movement during an
external file drag. While OS hover events are active, the document's retained
native window supplies the pointer position (at most 30 updates per second).
Drop events are consumed once and queued before checking whether an import or
document dialog is busy. Imports remain serialized, and queued drops retain
their original workspace. A changed workspace rejects the queued drop rather
than importing into the new workspace. The queue is bounded to 64 drop events
and reports overflow. The remembered target is temporary and expires after
one second; a current pointer position takes precedence.

The Explorer drop banner fits its text with eight points of horizontal padding
on each side and stays inside the panel. Selected file/folder icons use the
theme foreground independently of the tree's transparent selection border.

Manual documents and instructions are in `manual-tests/`. Native Finder drag
acceptance remains a manual check; deterministic tests cover routing, native
pointer precedence, repeated queued drops and one-time event consumption.

Verification: formatting and strict all-target Clippy pass, as do 1,218 Rust
suite tests (23 opt-in tests ignored), 14 xtask tests and 5 PDF.js host tests.
Fresh light/dark Problems captures under
`.tiptoptyp/screenshots/agent-review/ui-fixes-verified/` were inspected: providers
sit at the right edge of the message line. The compact drop banner, toolbar,
settings selector, Git controls and panel icons were also inspected in fresh
captures under `ui-fixes/`. The regenerated 22-image gallery passed validation
and its contact sheet was inspected. Framebuffer captures do not establish
native child-view composition or Finder drag acceptance.

These changes add no idle polling. The only added periodic work is bounded to
active macOS file drags and the visible compile animation; imports still run
one worker at a time. No throughput or cross-platform performance claim is made.

## Toolbar, workspaces and commands (2026-09-23 follow-up)

All toolbar and Git buttons follow the Toolbar buttons display preference.
Icons default to compact glyphs sized to the button font, with thinner strokes;
layout glyphs emphasize the relevant panel and maximize/restore have rounded
corners. Accessible action names and hover explanations remain in icon mode.

Resubscribing to the same workspace now replays its immutable cached tree.
Previously a service reset discarded the local tree, then a refresh suppressed
an unchanged snapshot, leaving Explorer empty. Root changes also clear stale
selection and update the active tab's workspace before opening another source.
Explorer shows Loading workspace during a scan, with errors if scanning fails.

Typst and TeX tools have equally visible groups in Settings. The Typst preview
backend preference controls Typst; TeX and opened PDFs use PDF.js on supported
platforms. Bundled paths appear in the Bundled button's hover. Service status
retains actual service errors/recovery state; duplicate Bundled badges are gone.

### Command customization

Each of Typst, Tinymist, Tectonic, TexLab, Badness and tex-fmt has a Command
customization section. Choose the executable with Bundled/Custom path, edit
arguments/environment/working directory, then press Apply command. Draft edits
do not restart workers on each keystroke. Reset command restores defaults.

Processes launch directly through Rust's process API, with piped or file-backed
standard streams. No terminal or shell runs implicitly. Quotes split arguments;
`$VARIABLE`, `$(...)`, pipes and wildcard characters are passed literally. To
use a shell or wrapper, select it explicitly as the custom executable.

- `{args}` expands to the exact generated argument list, preserving paths with
  spaces and non-UTF-8 paths. For example, Typst can use
  `{args} --input draft=true`.
- `{arg:N}` expands one generated argument by zero-based index. Omitting
  `{args}` replaces the whole list; the custom command must still honor the
  application's stdio protocol and generated output location.
- Environment is a JSON object, for example `{"MY_FLAG":"enabled"}`. Values
  override the inherited environment. Working directory defaults to the
  operation's document/project directory; an explicit override should be absolute.

Typst defaults to `watch --diagnostic-format short … --root ROOT INPUT OUTPUT`;
Tinymist starts `lsp` with workspace font paths. Tectonic starts `-X compile`
with its private mirror and output directory. TexLab has no default arguments,
Badness uses `lsp`, and tex-fmt uses `--stdin --quiet`. Environment and argv
errors are reported by the affected tool; the app never silently reparses them
as shell commands. The private output paths remain generated per operation.

Follow-up verification: formatting, strict all-target Clippy, 1,212 Rust suite
tests (24 opt-in tests ignored), 14 xtask tests and 5 PDF.js host tests pass.
Fresh light/dark toolbar, Git panel/chunk, tool-settings and rounded restore
framebuffers under `.tiptoptyp/screenshots/agent-review/followup/` were captured
and inspected, along with the regenerated, validated 22-image gallery.
Workspace replay and first-source-open regressions pass. Installed-app/Finder
interaction and native child-view composition remain manual acceptance checks.
Workspace replay reuses the same cached tree without starting another scan;
command parsing happens at process launch, with no new idle polling.

The PDF-first follow-up selects the first compilable source if the designated
tab is an asset; an existing source entry remains selected when later sources
open. Both TeX and Typst are covered, including tab round trips and compile
scheduling. The diff icon shares Stage/Unstage's lighter border. Formatting,
strict Clippy, 1,213 Rust suite tests, 14 xtask tests and 5 host tests pass.
The fresh Git framebuffer was inspected under
`.tiptoptyp/screenshots/agent-review/pdf-first/`. The real PDF.js browser probe
also passes after changing its rebuild fixture to use changed PDF bytes:
identical-byte rebuilds are now ignored. Seamless changed-document replacement
is tracked separately in todo 337 and `docs/pdfjs-preview.md`.
The full gallery command timed out twice after the Save dialog scenes (67-second
watchdog). A separate four-dialog batch restored the missing generated files;
all 22 files decode and the contact sheet was inspected, but this is not a
successful single-session gallery run. The sequencing timeout remains unresolved.
