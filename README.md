# tiptoptyp

`tiptoptyp` is a native Typst editor written in Rust. It keeps a long-running
`typst watch` process for canonical PDF builds and, when Tinymist is installed,
embeds Tinymist's interactive vector preview.

## MVP features

- Official `typst-syntax` parsing and incremental syntax highlighting
- Persistent `typst watch` compilation with a 60 ms editor debounce
- Tinymist SVG preview on macOS and Windows, including hover feedback and
  bidirectional preview/source navigation
- Continuous multi-page native fallback with trackpad pinch zoom,
  pointer-anchored scaling, visible page edges, clickable PDF links, and
  dark-page rendering
- Editable UTF-8 text files with Syntect highlighting, direct image previews,
  and direct native PDF viewing
- Inline error/warning line decoration, virtual diagnostic text, and full
  hover tooltips
- Independently toggled Explorer and Problems panels
- Code, Split, and Preview workspace modes
- Sorted, symlink-safe multi-file project tree
- UTF-8-safe literal find/replace with wraparound navigation
- New, Open, Save, Save As, drag-and-drop, dirty-file guards, and atomic writes
- PDF export for saved or unsaved documents; a stale document queues export for
  the next successful watched build
- Persisted System, Light, and Dark interface modes with layout-stable theme
  transitions
- A growing Settings panel for appearance, requested/effective preview backend,
  editor options, binary selection, toolchain health, and visible fallback
  history
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
development, tiptoptyp checks `PATH`. For backward compatibility it still
accepts the legacy `MYTYPST_*` variable names, so existing automation keeps
working; `MYTYPST_TYPST` / `MYTYPST_TINYMIST` are checked before `PATH`. Every
fallback is labelled in the fixed bottom status bar and Settings.

Each binary can instead be set to **Custom path** in Settings. This persisted
choice has priority over the bundled sidecar. An invalid custom path falls back
without losing the selected path, and the reason remains visible. Environment
overrides are retained for automation:

```sh
MYTYPST_TYPST=/absolute/path/to/typst \
MYTYPST_TINYMIST=/absolute/path/to/tinymist \
cargo run --release
```

On macOS, install Poppler for the PDF recovery viewer with
`brew install poppler`. Without Tinymist, or if its embedded viewer fails, the
application uses that native viewer and marks the automatic choice as a
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

System fonts are discovered when the persistent watcher starts and then reused
by incremental builds. To use only Typst's bundled fonts during development:

```sh
MYTYPST_IGNORE_SYSTEM_FONTS=1 cargo run --release
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
MYTYPST_PACKAGE_TARGET=x86_64-apple-darwin \
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
| Save | Cmd/Ctrl-S |
| Save As | Cmd/Ctrl-Shift-S |
| Find | Cmd/Ctrl-F |
| Find and replace | Cmd-Option-F on macOS, Ctrl-H elsewhere |
| Settings | Cmd/Ctrl-Comma |
| Refresh build and project | Cmd/Ctrl-R |
| Native preview zoom | Cmd/Ctrl-Plus, Minus, or 0 |

## Preview architecture

The editor mirrors the current in-memory buffer into a temporary `.typ` file
beside the document and runs one `typst watch` process against it. Keeping the
shadow beside the source preserves relative imports; the nearest ancestor with
`typst.toml` or `.git` is used as the project root. Successful builds provide
the exact PDF bytes used by Export PDF.

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

The shadow source and generated preview directories are removed when their
session ends. A forced process termination can leave temporary recovery
artifacts beside the document; they are ignored by the Explorer and Git and are
safe to inspect or remove. The real `.typ` file is only changed by Save or Save
As.

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
MYTYPST_IGNORE_SYSTEM_FONTS=1 \
  cargo test persistent_watcher_compiles_errors_and_recovers -- --ignored
```

Tinymist's process contract is covered by a fake LSP subprocess test, so the
normal test suite does not require a locally installed Tinymist binary.

## Current boundary

The editor still uses `egui::TextEdit<String>` and lays out the whole buffer.
A rope-backed, virtualized editor is the next performance milestone for very
large books. Linux interactive webview support and source-to-preview cursor
synchronization are also follow-up work; preview-to-source navigation is
already implemented.
