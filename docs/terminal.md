# Embedded terminal (todo 170)

Open **View → Terminal** or press **Control+`**. The bottom panel has mutually
exclusive **Problems** and **Terminal** tabs. The **Panel** toolbar button or
**View → Panel** (Primary+5) hides or reopens the last selected tab. Its height
is shared and resizable. Terminal also works in an empty workspace, without an
open document. See [bottom-panel controls](bottom-panel.md).

A window creates one login shell on first opening Terminal, in that window's
workspace directory. The refresh icon in the thin right-hand strip restarts the
shell in the current workspace. Its tooltip shows only `(start: <path>)`, the
session's starting directory; `cd` may change the live directory. Switching tabs,
hiding or maximizing the panel, opening files or changing the workspace does
not restart the shell. An exited shell retains its last output.
Closing the editor window stops its shell and foreground job, including the
retained hidden root window on macOS. Other windows own independent sessions.

Click the grid to focus it. Text, IME commits, Tab, Escape, arrows, function keys,
and Control sequences go through Ghostty's key encoder. Ctrl+C interrupts,
Ctrl+D sends EOF, and application cursor/Kitty keyboard modes are honored.
The Panel and Terminal toggles remain available; macOS Command menu commands remain
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


### Font rendering and fixed cells (todo 302)

The clipped version numbers reported on 22 September 2026 came from the font
fallback order. Apple Color Emoji was ahead of the primary text face and also
claimed ordinary digits, spaces, `#` and `*`. Its 13-point advances did not fit
the terminal's approximately 7.83-point cells at font size 13; the cell clip
cut away part of those glyphs. Libghostty's stored text was intact.

ASCII now uses the primary monospace face. Apple Color Emoji is excluded from
egui's outline fallback chain because its bitmap glyphs rasterized to zero ink.
On macOS, the terminal instead shapes each emoji grapheme with HarfRust and
decodes its actual system-font color bitmap through skrifa. This preserves the
normal Apple emoji artwork, including flags, skin tones, keycaps and ZWJ
sequences. The read-only system font is mapped once on first use; its roughly
180 MiB file is not copied into a heap buffer. A bundled Noto Emoji outline
fallback preserves missing-glyph coverage in the editor/UI and on other platforms; their color emoji are not certified here.

Ghostty remains authoritative for grapheme widths, spacer cells, wrapping,
selection and cursor positions, including application-controlled DEC mode 2027
for joined emoji sequences. Single emoji normally occupy two cells; private-use
Nerd Font symbols usually occupy one. Glyph ink is fitted uniformly inside that
allocation without changing the terminal's column count or stretching glyphs.
The existing asynchronous font catalog supplies an installed Nerd Font fallback
(prefer Symbols, then Mono); there is no runtime font download or installation.
Primary ASCII and selected editor/UI faces keep their existing font choices.

Transformed text layouts have a bounded 256-entry cache keyed by source layout,
cell geometry and bold state. Color emoji have a separate 128-entry cache,
including negative lookups, invalidated when pixel density changes. Warm frames
reuse the layouts/textures; only cache misses decode or transform glyphs. No
per-frame font reads, worker, idle timer or repaint loop was added. Regression tests cover ASCII metrics at
four scales, visible ink, color sequences, cell fitting, cache bounds and reuse.
The QA transcript includes versions, ANSI styles, emoji and CJK. Its synthetic
font catalog deliberately avoids machine-dependent Nerd Font files; the
installed-font probe checks real FiraCode and Symbols Nerd Font Mono metadata,
ASCII metrics and private-use glyph ink instead.

A local release microbenchmark on macOS 14.6.1 / aarch64 used the same 95
pre-laid-out Hack/NotoEmoji glyphs, white at 13 pt and 1× density, with
7.82666×16-point cells. After one warmup pass, 10,000 repetitions (950,000
glyphs) took 3.50 ms for the previous clone-only paint input and 40.34 ms with
cached fitting: about 39 ns added per glyph. The initial 95-glyph fitting pass
took 70 µs; shaping/decoding seven system color emoji at 26 ppem (13 pt, 2×) on first use took 758 µs.
These measure glyph preparation, not complete frames or native startup; they
are not a cross-platform timing guarantee. Idle repaint behavior is unchanged.
Metadata and raw results are retained in `.tiptoptyp/terminal-font-evidence/`
(`font-probes.log`, `color-probe.log`). Reproduce with:

```sh
cargo test --release --bin tiptoptyp terminal_font_probe -- --ignored --nocapture
cargo test --release --bin tiptoptyp system_emoji_sequences_render -- --nocapture
```

## Native dependency and build

`libghostty-vt = 0.2.1` is pinned; Cargo.lock pins its matching sys crate, which
builds Ghostty commit `a887df42c56f6de86c0fe6da9c4eeca37931e083`. This is the real
Ghostty VT C library, linked statically through the safe Rust wrappers, with
`portable-pty` managing the shell. No Ghostty application installation or
runtime library lookup is needed in the packaged app.

Install Zig **0.15.2** on PATH before building. The pinned source requires that
version, even though current upstream instructions describe a newer compiler.
The repository's `mise.toml` pins this version; see the
[Zig setup and missing-executable troubleshooting](../README.md#set-up-zig-for-source-builds)
for mise, Homebrew, and manual installation commands.
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

Validation on macOS 14.6.1 / Apple silicon, 22 September 2026, after integrating
master `d1dbae1` (PDF.js preview and the table editor):

- `cargo fmt --all -- --check` and strict all-target Clippy passed.
- `cargo test --no-fail-fast`: 1,147 passed, 18 explicitly ignored.
- `cargo test --manifest-path xtask/Cargo.toml`: 14 passed; xtask formatting passed.
- Both preview JavaScript test files: 12 passed.
- The complete 70-image gallery was freshly generated in one app session;
  `scripts/capture-theme-gallery.sh --validate-latest` decoded and validated all 70.

Fresh light/dark terminal framebuffers were inspected for readable ANSI colors,
Unicode/cursor placement, header alignment and clipping. Captures exposed a
header that consumed the available height; the fixed header is constrained to
one horizontal row. A semantic geometry regression reproduces the zero-height
grid before the fix and now checks the remaining grid height. The current
[Latte capture](ui-snapshots/latest/main-terminal-panel--catppuccin-latte.png)
shows the subsequent [compact controls and focus-lock fix](bottom-panel.md).
The reduced gallery retains dark samples for the main window, one dropdown and
one popup; terminal and other component scenes use light mode.
Local fresh captures are under
`.tiptoptyp/screenshots/agent-review/terminal/1790068806135-0001-main-terminal-panel.png`
and `1790068806298-0002-main-terminal-panel.png`.

The local `.tiptoptyp/terminal-evidence/terminal-capture.log` retained this trace:

```text
ui.preview.bounds available=(846.5,30.0)-(1392.0,642.0) clip=(838.5,30.0)-(1400.0,642.0) egui=(846.5,30.0)-(1392.0,642.0) native=(846.5,30.0)-(1392.0,642.0) viewport=(0.0,0.0)-(1400.0,886.0) zoom=1.000 egui_ppp=2.000 native_ppp=2.000
```

The bounds reserve the bottom panel beneath the preview. These are viewport
framebuffers; native child-view composition was not visually verified. CUA could
select the separately installed app but not this standalone test binary, so that
other running app was left untouched.

## Idle measurement

The baseline is a preserved binary built from `git archive d1dbae1`; the after
binary includes the integrated terminal and final header fix. Both used the
optimized `profiling` profile, `--features profiling` and
`RUSTFLAGS='-C force-frame-pointers=yes'`, Rust 1.98.1 on the same macOS aarch64
machine. The `main` scenario used the same isolated source/sidecar hashes,
Catppuccin Latte, a 1400×886-point viewport at 2× scale, no manual interaction,
and an 8-second measurement with no concurrent builds. The terminal was closed.
The runner used `--binary` to retain the exact binary hashes; its metadata says
build skipped because compilation was performed separately as described here.

The first pair used a 3-second warmup: master still delivered 89 editor passes
(mean 808.49 µs), while the after run delivered none. This is different settling,
not evidence of a speedup. A second pair used an 8-second warmup for both builds. Both settled runs recorded no editor
passes or repaint requests during the measurement window; there is no per-pass
timing to compare. No closed-terminal idle repaint was observed.

[Run metadata and summaries](terminal-performance.json) retain all four runs,
binary hashes, build provenance and workload details. These are bounded local
idle observations, not CPU measurements or a speedup claim. Shell startup,
open-terminal output throughput, active interaction and other platforms were
not benchmarked. Deterministic tests protect lazy startup, row reuse, bounded
queues/scrollback and hidden-terminal repaint suppression.
