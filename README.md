# tiptoptyp

`tiptoptyp` is a native Typst editor written in Rust. It uses Tinymist for an
interactive vector preview and Typst plus Poppler for canonical PDF output and
a rasterised recovery viewer.

## MVP features

- Official `typst-syntax` parsing and incremental syntax highlighting
- Debounced canonical PDF compilation with project-local private artifacts
- Tinymist SVG preview on macOS and Windows, including hover feedback and
  bidirectional preview/source navigation
- Continuous multi-page rasterised fallback with trackpad pinch zoom,
  pointer-anchored scaling, visible page edges, clickable PDF links, and
  dark-page rendering
- Editable UTF-8 text files with Syntect highlighting, direct image previews,
  and direct native PDF viewing
- Inline error/warning line decoration, virtual diagnostic text, and full
  hover tooltips
- Explorer sections for files, document contents, subfiles, symbols, and packages
- A per-project Typst preview entry point for multi-file documents
- Navigable Problems rows and compact, normalized diagnostic hover cards
- Code, Split, and Preview workspace modes
- Sorted, symlink-safe multi-file project tree
- UTF-8-safe literal find/replace with wraparound navigation
- New, Open, Save, Save As, undo/redo, formatting, auto-save, drag-and-drop,
  dirty-file guards, and atomic writes
- PDF export for saved or unsaved documents; a stale document queues export for
  the next successful watched build
- Thirty-two bundled interface themes, with independent light and dark choices:
  Tiptop, Paper, Ocean, Forest, Catppuccin, Solarized, Gruvbox, GitHub,
  Rosé Pine, Tokyo Night, Kanagawa, Everforest, Ayu, Flexoki, and Dracula
- Whole-theme inversion followed by a configurable hue rotation, applied in
  lockstep to chrome, diagnostics, controls, and both syntax highlighters
- TextMate `.tmTheme` and modern `.sublime-color-scheme` import for either the
  light or dark slot, with each inferred palette shared by app chrome,
  diagnostics, and syntax highlighting
- A separate compact Settings window for appearance, preview backend, editor
  options, hover timing, binary selection, and toolchain health
- App-window-only UI QA screenshots under `.tiptoptyp/screenshots`, plus a
  reproducible checked-in light/dark gallery under `docs/ui-snapshots/latest`
- Line wrapping and line numbers enabled by default, independently toggleable
  without changing the source buffer
- Pinned Typst and Tinymist sidecars in packaged builds, with per-tool custom
  executable paths

The decisions behind the dual preview pipeline are recorded in
[`docs/architecture/0002-interactive-editor.md`](docs/architecture/0002-interactive-editor.md)
and
[`docs/architecture/0003-bundled-toolchain.md`](docs/architecture/0003-bundled-toolchain.md).

## Requirements

- Rust 1.95 or newer
- `curl` and `tar` when fetching the pinned sidecars for a development build
- Poppler's `pdftoppm` on `PATH` for native PDF rendering; `pdftohtml` from the
  same package enables clickable link hotspots

Packaged releases include Typst 0.15.1 and Tinymist 0.15.2. For a source-tree
run, fetch those exact, hash-verified binaries once:

```sh
cargo run --manifest-path xtask/Cargo.toml -- fetch-sidecars
```

The downloaded archives, staged executables, licenses, and provenance records
are generated under `toolchain/` and ignored by Git. If they are absent during
development, tiptoptyp checks `PATH`. `TIPTOPTYP_TYPST` and
`TIPTOPTYP_TINYMIST` can override that development fallback. The pre-rename
`MYTYPST_*` names remain lower-priority aliases so existing automation and
local configurations continue to work. Every fallback is labelled in the
fixed bottom status bar and Settings.

Each binary can instead be set to **Custom path** in Settings. This persisted
choice has priority over the bundled sidecar. An invalid custom path falls back
without losing the selected path, and the reason remains visible. Environment
overrides are retained for automation:

```sh
TIPTOPTYP_TYPST=/absolute/path/to/typst \
TIPTOPTYP_TINYMIST=/absolute/path/to/tinymist \
cargo run --release
```

On macOS, install Poppler for the PDF recovery viewer with
`brew install poppler`. Without Tinymist, or if its embedded viewer fails, the
application uses the rasterised viewer and marks the automatic choice as a
fallback. Linux currently uses this route because the Wry child-view
integration is limited to macOS and Windows in this MVP.

## Run

```sh
cargo run --release
```

Open a document directly:

```sh
cargo run --release -- path/to/document.typ
```

Open a workspace directly:

```sh
cargo run --release -- path/to/project
```

Launching the packaged `tiptoptyp.app` without a path opens a compact workspace
chooser with recent folders. The bundle registers the text, Typst, PDF, and
image formats supported by the editor, so Finder's **Open With** and dropping a
file on the app icon open that file and infer its workspace. The Explorer's
folder button, **File → Open Folder**, and Cmd/Ctrl-Shift-O change workspace at
any time.

Choose **System**, **Light**, or **Dark** under **Settings → Appearance**, then
pick independent Light theme and Dark theme palettes. System appearance swaps
between those two choices with the OS. Either slot can instead import a Sublime
color scheme. The importer infers editor colors, selection and cursor colors,
semantic status colors, and cohesive panel/control surfaces from the same file.
**Invert** is applied first and the selected hue rotation second, across the
complete interface and syntax theme. Bundled palette provenance is documented
in [`docs/theme-sources.md`](docs/theme-sources.md).

To use only Typst's bundled fonts during development:

```sh
TIPTOPTYP_IGNORE_SYSTEM_FONTS=1 cargo run --release
```

## Package

Install the official cargo-packager CLI, then build a native package:

```sh
cargo install cargo-packager --locked
cargo packager --release
cargo run --manifest-path xtask/Cargo.toml -- verify-package
```

The package hook downloads and verifies the pinned platform archives, checks
both reported versions, performs the release build, and installs Typst and
Tinymist as external sidecars beside `tiptoptyp`. Typst's `LICENSE`/`NOTICE`,
Tinymist's `LICENSE`, and per-artifact provenance are included as resources.
The committed target/URL/hash matrix is
[`toolchain/manifest.tsv`](toolchain/manifest.tsv).

For a cross-target build, give the same target to cargo-packager and its hook;
cargo-packager does not otherwise expose its `--target` value to hook commands:

```sh
TIPTOPTYP_PACKAGE_TARGET=x86_64-apple-darwin \
  cargo packager --release --target x86_64-apple-darwin
```

macOS packages are ad-hoc signed by default so the app, sidecars, and sealed
resources form a valid local bundle. Public distribution must replace `-` in
the packager signing configuration with a Developer ID Application identity
and provide notarization credentials.

## Keyboard shortcuts

| Action | Shortcut |
| --- | --- |
| New | Cmd/Ctrl-N |
| Open | Cmd/Ctrl-O |
| Open folder | Cmd/Ctrl-Shift-O |
| Save | Cmd/Ctrl-S |
| Save As | Cmd/Ctrl-Shift-S |
| Export PDF | Cmd/Ctrl-Shift-E |
| Undo / Redo | Cmd/Ctrl-Z / Cmd/Ctrl-Shift-Z |
| Find | Cmd/Ctrl-F |
| Find and replace | Cmd-Option-F on macOS, Ctrl-H elsewhere |
| Format Typst | Option/Alt-Shift-F |
| Settings | Cmd/Ctrl-Comma |
| Refresh build and project | Cmd/Ctrl-R |
| Rasterised preview zoom | Cmd/Ctrl-Plus, Minus, or 0 |
| App-only UI screenshot | Cmd/Ctrl-Shift-F12 |

## Preview architecture

The editor mirrors the in-memory preview entry into
`<project>/.tiptoptyp/typst-*`, preserving the project's relative layout with
private links/copies. The nearest ancestor with `typst.toml` or `.git` is used
as the project root. Successful builds provide the exact PDF bytes used by
Export PDF. On macOS a fresh watcher is started for editor-buffer changes
because native notifications suppress changes below dot-directories; Tinymist
still supplies the low-latency interactive preview.

Tinymist runs as a separate LSP sidecar. tiptoptyp sends `didOpen` and full-buffer
`didChange` notifications, starts Tinymist's documented default preview, and
embeds the localhost frontend it returns. Tinymist therefore owns incremental
SVG rendering, source spans, hover behavior, and click mapping. tiptoptyp handles
`window/showDocument` to reveal and select the corresponding native editor
range.

The fallback is intentionally a recovery viewer. egui has no built-in PDF
renderer, so Poppler rasterizes the watched PDF at 144 DPI and extracts link
rectangles for native interaction. It cannot recover Typst source spans, but it
does provide continuous pages, stable scroll state, pinch zoom, explicit
boundaries, and a preview-only dark transform. Exported PDF bytes are never
rasterized or colour-modified.

The shadow source, formatting backing files, PDF render data, and screenshots
all live below `.tiptoptyp`; ephemeral sessions remove themselves normally and
the directory is ignored by Explorer and Git. A forced termination can leave a
session there that is safe to inspect or remove. The real `.typ` file is only
changed by Save, Save As, formatting followed by auto-save, or normal editing
auto-save.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
```

The real watcher integration test is ignored by default because it requires
Typst, Poppler, and native filesystem notifications:

```sh
TIPTOPTYP_IGNORE_SYSTEM_FONTS=1 \
  cargo test persistent_watcher_compiles_errors_and_recovers -- --ignored
```

Tinymist's process contract is covered by a fake LSP subprocess test, so the
normal test suite does not require a locally installed Tinymist binary.

## Current boundary

The editor still uses `egui::TextEdit<String>` and lays out the whole buffer.
A rope-backed, virtualized editor is the next performance milestone for very
large books. Linux interactive webview support is also follow-up work;
bidirectional source/preview navigation is already implemented on supported
platforms.
