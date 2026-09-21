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
- Drop files onto an Explorer folder to copy them into the project, or onto
  the code editor to open them. Explorer also supports confirmed file deletion.
- Fuzzy completion filtering while typing, code-first reference suggestions,
  and font completion after `text(font: ` or an opening quote
- Font samples rendered in the selected font in completion and on hover in
  Settings font pickers; fuzzy font search in Settings and document font menus
- Installed package removal with a confirmation for the selected version
- An Explorer Git panel for status, diffs, staging, unstaging, commits, recent history,
  fetch, fast-forward pull, and push
- Pause/resume control for automatic preview updates. Compile writes the
  current canonical PDF beside its saved Typst entry; unsaved documents ask
  for a destination. Export PDF remains available for choosing another path.
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

- Rust 1.98 or newer
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
`TIPTOPTYP_TINYMIST` can override that development fallback. Every fallback is
labelled in the fixed bottom status bar and Settings.

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
fallback. Tinymist server and preview-start failures first receive up to four
retries, one second apart; the fifth consecutive failure selects the fallback.
Linux currently uses this route because the Wry child-view
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

Launching the packaged `tiptoptyp.app` without a path restores the newest recent
workspace that still exists, falling back to the current folder on first run.
The bundle registers the text, Typst, PDF, and image formats supported by the
editor, so Finder's **Open With** and dropping a file on the app icon open that
file and infer its workspace. **File → Change Workspace Root**,
Cmd/Ctrl-Shift-O, or double-clicking the Explorer path opens the workspace
chooser with recent folders and an option to select another folder.

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

Open **View → Git** for repository operations. Git must be on `PATH`;
remote operations use the repository's configured remotes, upstream branch,
and credentials. Save editor changes before staging or committing. Pull only
fast-forwards, so divergent branches require resolution outside this panel.
**Diff** shows unstaged edits against the staging area; **Staged diff** shows
changes included in the next commit. Both open a selectable, color-coded viewer
from the Git panel. **Unstage all** clears the staging area and keeps working
files intact. Staging excludes `.tiptoptyp` temporary files; any already staged
temporary files remain visible so you can unstage them.

The Explorer shows Git badges (`A` added, `M` modified, `D` deleted, `?`
untracked, `U` conflicted); hover for staged/unstaged details. Beside the line
numbers, green marks additions, blue marks modifications, and red marks
deletions compared with the last commit, including unsaved editor changes.
Click a marker to open that chunk's diff. Each document window keeps its own
Git state and selected diff, including when two windows edit the same file.
Git decorations refresh automatically and after refreshing the Explorer.

## Package

### Identify or replace a development build

Settings → Status and `./target/release/tiptoptyp --version` show the package
version and compiled build ID (Git revision, dirty marker, Unix build timestamp).
The timestamp refreshes when Cargo reruns the build script; an unchanged cached
build keeps its ID. This identifies builds, rather than preventing old copies
from launching. The executable does not invoke Git at runtime.

Quit the running app before replacing it. For a completely clean local build:

```sh
cargo clean
cargo build --release
./target/release/tiptoptyp --version
./target/release/tiptoptyp
```

`cargo clean` removes Cargo artifacts, not installed/copied `.app` bundles or
running processes. The standalone runner has a separate build directory:
`cargo clean --manifest-path xtask/Cargo.toml` cleans that too, if needed.
Neither command removes your documents or preferences. Usually `cargo build
--release` alone is sufficient; cleaning forces all dependencies to rebuild.

### Build a native package

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
The canonical `ttt` application mark is embedded in direct executable launches
and supplied as native ICNS/ICO/PNG artwork to packaged builds.
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
| Toggle tooltip under mouse | Cmd/Ctrl-Shift-Space |
| Toggle tooltip at caret | Cmd/Ctrl-Shift-K |
| Dismiss focused tooltip | Escape |
| Settings | Cmd/Ctrl-Comma |
| Compile PDF beside source | Cmd/Ctrl-R |
| Interface scale | Cmd/Ctrl-Plus or Minus |
| Rasterised preview zoom | Cmd/Ctrl-Option/Alt-Plus, Minus, or 0 |
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

Use focused deterministic tests for UI behavior and fresh screenshots when
correctness depends on pixels. Native preview geometry also requires
`TIPTOPTYP_UI_TRACE=1` bounds output. See [`AGENTS.md`](AGENTS.md) for the workflow and
[`docs/ui-qa-screenshots.md`](docs/ui-qa-screenshots.md) for the scene contract.

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

### Performance

```sh
cargo xtask profile --scenario settings
cargo xtask profile --scenario large --sampler none
cargo bench -p tiptoptyp-core --bench document
```

The profiler builds an optimized, symbolized executable with frame pointers,
uses isolated fixtures, and saves CPU samples, bounded subsystem timings, and
run metadata under `.tiptoptyp/profiles`. Native sampling defaults to macOS
`sample`; Linux can select `--sampler perf`. See
[`docs/performance.md`](docs/performance.md) for setup, workloads, comparison
rules, and limitations. Performance review is part of the ongoing working
agreement in `AGENTS.md`.

## Current boundary

The editor still uses `egui::TextEdit<String>` and lays out the whole buffer.
A rope-backed, virtualized editor is the next performance milestone for very
large books. Linux interactive webview support is also follow-up work;
bidirectional source/preview navigation is already implemented on supported
platforms.
