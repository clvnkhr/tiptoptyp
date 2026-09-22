# Embedded terminal (todo 170)

Open **View → Terminal** or press **Control+`**. The bottom panel has mutually
exclusive **Problems** and **Terminal** tabs; the existing Problems toolbar
button opens diagnostics. Its height is shared and resizable. Terminal also
works in an empty workspace, without an open document.

A window creates one login shell on first opening Terminal, in that window's
workspace directory. The directory above the grid is the session's starting
directory. Switching tabs, hiding the panel, opening files or changing the
workspace does not restart the shell or change its working directory. Use
`cd` normally; **Restart terminal** stops the current session and starts another
in the currently selected workspace. An exited shell retains its last output.
Closing the editor window stops its shell and foreground job, including the
retained hidden root window on macOS. Other windows own independent sessions.

Click the grid to focus it. Text, IME commits, Tab, Escape, arrows, function keys,
and Control sequences go through Ghostty's key encoder. Ctrl+C interrupts,
Ctrl+D sends EOF, and application cursor/Kitty keyboard modes are honored.
The terminal toggle remains available; macOS Command menu commands remain
application commands. Source editing shortcuts do not modify the document
while the terminal owns focus.

Drag to select visible cells; copy/paste uses Command+C/V on macOS and
Ctrl+Shift+C/V elsewhere. Select All selects the visible grid. Wheel scrolling
moves through Ghostty's scrollback, and typing returns to the live prompt.
Pastes respect bracketed-paste mode, normalize line endings and strip terminal
controls such as ESC. A paste larger than 256 KiB is rejected with a message.

The renderer supports ANSI/256/true colors, inverse/faint/bold/italic text,
underline/strike-through, combining characters, double-width cells and the
alternate screen. It uses the app's light/dark foreground/background and a
matching ANSI palette. Program-defined palette overrides are retained.
Cursor painting is static: the terminal adds no idle blink timer.

This first integration has one session per window. Mouse reporting to TUI
programs, image protocols, hyperlinks, search and selection spanning multiple
scrollback viewports are not implemented. Use the keyboard to operate TUI
programs. macOS is the repository's supported native target; Linux and Windows
are not certified by the local macOS checks.

## Native dependency and build

`libghostty-vt = 0.2.1` is pinned; Cargo.lock pins its matching sys crate, which
builds Ghostty commit `a887df42c56f6de86c0fe6da9c4eeca37931e083`. This is the real
Ghostty VT C library, linked statically through the safe Rust wrappers, with
`portable-pty` managing the shell. No Ghostty application installation or
runtime library lookup is needed in the packaged app.

Install Zig **0.15.2** on PATH before building. The pinned source requires that
version, even though current upstream instructions describe a newer compiler.
The first Cargo build fetches the pinned Ghostty sources and Zig dependencies.
For a prefetched build, the sys crate supports `GHOSTTY_SOURCE_DIR` and
`GHOSTTY_ZIG_SYSTEM_DIR`; use the exact pinned revision. CI installs Zig 0.15.2.
Native dependency notices are tracked in [licenses](licenses/README.md) and
included by `cargo xtask generate-notices` in the packaged notices.

Upstream references: [Ghostty's embedding example](https://github.com/ghostty-org/ghostling),
[versioned Rust bindings](https://docs.rs/libghostty-vt/0.2.1/libghostty_vt/),
[portable-pty](https://docs.rs/portable-pty/0.9.0/portable_pty/).

## Ownership and bounded work

`src/terminal/engine.rs` owns the Ghostty adapter. Its thread-affine handles
never cross threads; only immutable grid snapshots do. Unchanged rows reuse
Arc storage. `session.rs` owns startup, PTY reads/writes, cancellation and
reaping on background threads. There are no per-frame process calls or idle
polling timers. Each PTY read is at most 4 KiB, a batch is at most 16 events,
and both input/output event and writer queues hold at most 64 messages.
Input saturation is reported instead of silently losing a keystroke; output
uses bounded backpressure. Snapshots replace the previous snapshot rather than
queueing frames. Repaint requests target only the owning visible viewport and
coalesce until that viewport reads a snapshot. Hidden sessions keep processing
output without requesting UI repaints.

Scrollback has an 8 MiB budget, rounded by Ghostty's page allocator with an
active-screen minimum. Despite the wrapper's “lines” comment, the pinned
native implementation measures `max_scrollback` in bytes. Grid dimensions are
bounded to 500 × 200 cells; APC strings are limited to 1 MiB. The untouched,
closed terminal owns no process, worker or Ghostty grid.

`src/app/terminal_panel.rs` owns panel selection and keyboard admission.
The existing panel layout reserves space before the editor/preview; there is
no new native child view or rendering surface.

## Verification

Focused regressions exercise real PTY input/output, working directory, resize,
device-status replies, EOF, shell/job cleanup, bounded queues, hidden repaint
suppression, ANSI and Unicode rendering, alternate screens, palette changes,
row reuse, scrollback limits, paste encoding, IME, panel controls and editor
input isolation. The `terminal-panel` QA scene feeds a fixed VT transcript
through the same Ghostty adapter without starting a shell.

Validation results and performance measurements are recorded below after the
final run.
