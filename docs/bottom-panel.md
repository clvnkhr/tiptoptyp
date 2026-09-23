# Bottom panel (todos 291–293, 303–305)

Use **Panel** in the toolbar or View menu, or **Command+5** on macOS /
**Control+5** elsewhere. It opens or closes the shared bottom panel and remembers
whether Problems or Terminal was selected. The compact toolbar uses a panel
icon. **View → Terminal** and **Control+`** still toggle Terminal directly.
Both toggles work while the terminal owns keyboard focus and without an open
document. A window retains its shell when the panel is hidden.

The panel can grow to the full available content height. The header's square
**Maximize panel** control toggles between that height and the previous resized
height. **View → Maximize/Restore Panel** uses **Command+Option+5** on macOS /
**Control+Alt+5** elsewhere; it is configurable in Keyboard Shortcuts and works
while Terminal has focus. Maximizing a closed panel opens its remembered tab.
The toolbar/status bar remain available; source, Explorer and native preview
surfaces are hidden while the panel fills the content area.

Explorer section headers have the same maximize/restore control. Maximizing
one section temporarily opens it and hides every sibling section. Restore
recovers the previous section sizes, collapsed states and ordering. The
workspace header and Explorer search remain available.

The maximize and restore controls use the same overlapping-window outline
(todo 316). Maximize emphasizes the upper window; restore emphasizes the lower
window. Hit targets, shortcuts and saved panel sizes are unchanged.

Problems shows its diagnostic count in a badge beside the tab. Terminal has one
22-point square refresh button in a narrow right strip. Refresh stops the shell
and starts a new one in the current workspace. Its tooltip is `(start: <path>)`,
showing where this session started. Shell `cd` commands can change its live
directory. Startup/exit status uses spare header space instead of another row.

The focus deadlock was caused by calling the viewport-scoped `terminal_id`
inside an egui memory write closure. Computing the ID reads the same context
lock. IDs are now resolved before taking memory locks; tab, close, toolbar,
menu and shortcut transitions use the same focus/visibility adapter.

Regression coverage includes the original lock failure, root/secondary-window
command routes, remembered tabs, input isolation, count alignment, strip/grid
bounds, the terse restart tooltip, maximize/restore geometry and preserved
Explorer section state. The old lock regression failed
at egui's 10-second debug lock timeout and passes after removing reentrancy.
The idle path adds no worker or repaint timer, still reads one terminal snapshot
per visible frame, and formats the path only while the restart icon is hovered.
No material performance impact is expected; this is not a native timing claim.

Fresh light-mode Terminal and Problems viewport captures were inspected under
`.tiptoptyp/screenshots/agent-review/bottom-panel/`. The reduced stable gallery
was regenerated in one app session and all 22 images validated. These are app
framebuffers, not composed desktop evidence. [Terminal](ui-snapshots/latest/main-terminal-panel--catppuccin-latte.png)
and [Problems](ui-snapshots/latest/main-problems-panel--catppuccin-latte.png)
show the compact controls and badge.

## Maximize and font-fix verification (22 September 2026)

### Current Nerd Font icons (23 September follow-up)

Terminal text retains its fixed monospace metrics. Private-use icon cells use a
separate family with the discovered Nerd Font first, so a fallback text face
cannot intercept an icon. Symbols-only and patched text fonts have different em
metrics; icon ink is fitted to a consistent height while preserving aspect ratio.
A following blank cell with the same background may provide extra drawing width.
This changes neither Ghostty's column allocation nor copying, selection indices,
or cursor movement. Adjacent text and different backgrounds remain boundaries.
Powerline separators retain the existing cell-fitting behavior.

Backgrounds are painted before glyphs, so the blank cell's selection background
cannot erase an icon extending into it. Transformed glyphs remain cached in the
bounded 256-entry cache. Discovery and font loading still happen only when the
font configuration changes; there is no added idle polling.

Old Nerd Fonts codepoints are not remapped and no legacy font fallback is added.
The prompt configuration must use codepoints supported by its current font.
The optional `terminal-icons` capture scene uses installed fonts and modern branch,
package and Rust glyphs; it is excluded from the portable stable gallery.

The release cached-transform probe used the same 95 Hack/NotoEmoji glyphs,
13-point font, 7.82666 × 16-point cells, white ink, scale 1, macOS arm64, and
10,000 warmed repetitions before/after. It measured 42.57 ms before and 45.31 ms
after for 950,000 lookups (about 44.8 vs 47.7 ns each). Cold-cache results were
recorded separately and are not compared. Logs and installed-font coverage
results are in `.tiptoptyp/terminal-font-evidence/current-icons/`. This bounded
single-run microbenchmark is not a whole-frame, idle, or cross-platform claim;
it does not measure the new background-paint pass.

Formatting, strict all-target Clippy, 1,214 Rust suite tests and 14 xtask tests
pass. The opt-in font probe verifies current branch/Rust/package codepoints in
installed Fira Code Nerd Font Mono and Symbols Nerd Font Mono while preserving
ASCII advances. Fresh light/dark terminal framebuffers under
`.tiptoptyp/screenshots/agent-review/nerd-icons/` were inspected, including tight
icon/text boundaries, normal/bold/italic text, and colored emoji.
The full 22-image gallery completed in one session, passed validation, and its
fresh contact sheet was inspected. The previously observed dialog-sequencing
timeout did not recur in this run; no fix for that intermittent issue is claimed.

Required formatting, strict all-target Clippy, 1,170 tests (20 explicitly ignored)
and all 14 xtask tests pass. Semantic tests exercise a 450-point normal panel,
maximize, restore, the shortcut while Terminal has focus, the native menu,
window shrinkage, and Explorer sibling visibility/collapsed-state restoration.
The original section-height budget test caught an empty button's implicit text
padding; exact square allocation now keeps every header within its old budget.

Fresh light Terminal, maximized Terminal, maximized Files and normal Terminal
frames were inspected under
`.tiptoptyp/screenshots/agent-review/terminal-fonts-final-verified/`
(`1790076091769-0001` through `1790076091937-0004`). Digits, ANSI styles and
colored Apple emoji are legible; maximize fills the available body, the restore
icon stays in its header, and maximized Explorer hides sibling headers/bodies.
The regenerated 22-image gallery passed decoding/transparency checks and its
contact sheet was inspected. It still has only three dark samples.

`TIPTOPTYP_UI_TRACE=1` retained this normal-layout boundary in
`.tiptoptyp/terminal-font-evidence/terminal-capture.log`:

```text
ui.preview.bounds available=(1067.5,30.0)-(1392.0,642.0) clip=(1059.5,30.0)-(1400.0,642.0) egui=(1067.5,30.0)-(1392.0,642.0) native=(1067.5,30.0)-(1392.0,642.0) viewport=(0.0,0.0)-(1400.0,886.0) zoom=1.000 egui_ppp=2.000 native_ppp=2.000
```

These captures verify app framebuffers and allocated bounds. Native child-view
composition was not visually verified in this pass. Maximization explicitly
hides all native preview adapters; it does not move them over the terminal.

Diagnostic providers (such as Tectonic and tinymist) are metadata separate from
the message and hints. Problems paints the provider against the right edge of
the message line only when it fits; narrow or wrapped rows omit that label.
The hover tooltip retains the provider in either case.
