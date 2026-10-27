# tiptoptyp

A native desktop editor for **Typst and LaTeX**, written in Rust. Source editing,
interactive previews, local spelling and grammar checks, Git comparisons and an
optional Overleaf connection share one workspace. macOS is the primary tested
platform.

## What it does

- **Write:** syntax highlighting, completions, folding, indentation guides,
  word wrap, rainbow brackets, local-definition-use highlights and custom text
  highlights. Edit snippets as Typst or TeX with linked fields such as
  `{{1:x}}`; each snippet can use its own placeholder delimiters.
- **Preview:** buffered Tinymist partial rendering for Typst, with full Tinymist
  and PDFium available in Settings. TeX output and opened PDFs use PDFium.
  Navigate between source and preview, search PDFs and choose the previewed
  document independently of the active editor tab.
- **Check prose:** optional Harper and LTeX+ spelling and grammar checks, with
  mathematical vocabulary and editable settings. Filter Problems by provider,
  severity and text. Optionally check selected prose with Liquid and review
  the complete before/after comparison before applying corrections.
- **Edit tables and mathematics:** a grid workbench for Typst tables/grids,
  TeX tabular-family environments, arrays, matrices, cases and aligned formulas.
  Supported computed cells show locked compiler samples; literal cells remain
  editable. Supported table actions insert missing TeX package imports. See
  [table support and limits](docs/table-editor.md).
- **Work with projects:** Explorer files, contents, subfiles, symbols, packages,
  labels and references; multi-file entry points; Git status, staging, commits,
  history and remote operations. The [Git diff editor](docs/git-diff-editor.md)
  shows the whole document, with an editable right column and an overview of
  changes. On macOS, [local file versions](docs/local-file-versions.md) provide
  native save history, comparisons and restore.
- **Use TeX tools:** Tectonic or a system TeX engine, latexmk/BibTeX/Biber
  orchestration, formatting, TeXdoc manuals, DOI/arXiv citation import and a
  cancellable LaTeXdiff wizard.
- **Stay in the workspace:** a command palette, a Ghostty-based terminal with
  multiple panes, clickable links and scrollback search, plus Problems and
  Activity panels. Connect an Overleaf project for native source editing.
- **Customize:** searchable Settings, validated JSON editing, configurable
  shortcuts, editable snippets/templates, bundled and imported themes, font
  previews and optional editor effects. macOS also supports background
  transparency and directional blur; fullscreen disables both.

The built-in **Settings → Help** covers shortcuts, preview controls, snippets,
tool configuration and suppression comments. Shortcuts in that help reflect
custom bindings.

## Run from source

### Requirements

- The Rust **1.98.1** toolchain selected by `rust-toolchain.toml`, including
  rustfmt and Clippy. Install Rust through rustup if it is not already available.
- Zig **0.15.2** on `PATH`, and Git, for the pinned libghostty-vt terminal build.
  The first build fetches its source dependencies. Installing the Ghostty app
  does not supply Zig.
- A native compiler/SDK; on macOS, install Apple's Command Line Tools.
- `curl` and `tar` for preparing bundled tools.

With [mise](https://mise.jdx.dev/), the checked-in `mise.toml` selects Zig
without changing your global version:

```sh
mise trust mise.toml
mise install
mise exec -- zig version                 # must print 0.15.2
```

Use `mise exec --` before Cargo commands if mise is not activated in your shell.
Alternatively, on macOS:

```sh
brew install zig@0.15
export PATH="$(brew --prefix zig@0.15)/bin:$PATH"
zig version                             # must print 0.15.2
```

On other platforms, install the matching
[Zig archive](https://ziglang.org/download/#release-0.15.2). A missing or wrong
Zig version must be fixed before rebuilding; `cargo clean` will not supply it.
See [terminal build details](docs/terminal.md#native-dependency-and-build).

### Prepare tools and launch

From the repository root, fetch the pinned typesetting tools and PDF renderer:

```sh
cargo xtask fetch-sidecars
cargo xtask fetch-pdfium
cargo run --release
```

For a macOS source build, prepare the optional LTeX+ checker and its matching
Java runtime with `cargo xtask fetch-ltex`, then enable it in Settings. Downloads
and generated payloads live under the Git-ignored parts of `toolchain/`.

Open a document or workspace directly:

```sh
cargo run --release -- path/to/document.typ
cargo run --release -- path/to/document.tex
cargo run --release -- path/to/project
```

Direct Cargo launches are named **tiptoptyp Dev**. Packaged production builds
use **tiptoptyp**. Settings → Status and `tiptoptyp --version` identify the
compiled version and build ID.

Settings → Tools lets you select custom executables, arguments, environment
variables and working directories. Development builds can also resolve tools
from `PATH` or `TIPTOPTYP_TYPST`, `TIPTOPTYP_TINYMIST`, `TIPTOPTYP_TECTONIC`,
`TIPTOPTYP_TEXLAB`, `TIPTOPTYP_BADNESS` and `TIPTOPTYP_TEX_FMT`. The app reports
which executable it selected. PDFium is prepared separately and is required
for PDF viewing; Poppler is not required.

## Preview and compilation

| Document/viewer | Live preview | Save a PDF beside the source |
| --- | --- | --- |
| Typst + Tinymist | Buffered partial renderer by default; full renderer selectable | **PDF** button or Compile PDF shortcut |
| Typst + PDFium | Compiled PDF | Compiler button or Compile PDF shortcut |
| TeX + PDFium | Compiled PDF | Compiler button or Compile PDF shortcut |

Automatic preview updates use private build files. They do **not** mean a PDF
has been saved beside the source. Explicit compilation saves it there; unsaved
documents ask for a destination. **Export PDF** chooses another output path.
The separate PDF button appears only for Typst with Tinymist.

**Settings → Preview** selects the Typst viewer, navigation gestures and smooth
Tinymist zoom. TeX and opened PDFs always use PDFium. A Tinymist failure does
not silently switch renderers. macOS and Windows have native Tinymist webviews;
Linux currently requires the PDFium choice for Typst.

For TeX, **Settings → Tools → TeX tools** groups the compiler, build driver,
bibliography options, Biber cache clearing and formatter settings:

- **Tectonic** is bundled and selected by default. It manages its own TeX/BibTeX
  reruns and downloads missing TeX packages. **Use cached TeX packages only**
  enables offline builds once the required packages are cached.
- **pdfLaTeX, XeLaTeX and LuaLaTeX** use an installed TeX distribution. They
  default to finite latexmk builds; bibliography orchestration can be disabled.
  The direct-engine alternative runs basic passes without BibTeX/Biber automation.
- **TexLab** supplies completions, hover and diagnostics. **Badness** supplies
  formatting/linting; **tex-fmt** is an alternative formatter. Configure each
  independently. TexLab's own builds are disabled to avoid duplicate work.

TeX builds run in a private project mirror and can be cancelled. Included files
are read from disk, so save them before rebuilding. SyncTeX navigation requires
`synctex` from a TeX distribution, including when Tectonic is the compiler.
Private build paths are mapped back to the original source. See the
[TeX service architecture](docs/architecture/0007-tex-services.md).

## Spelling and grammar

**Settings → Grammar & spelling** groups the independent checkers and their
configuration. Harper and LTeX+ run locally and exclude technical source
constructs rather than checking raw TeX/Typst commands as prose. Both can be
configured or disabled independently; enabling both may produce overlapping
suggestions. Normal and verbose diagnostic presets expose different levels of
advice, with their actual configuration available to edit.

LTeX+ and its matching Temurin Java runtime ship in macOS packages; enable the
checker in Settings. No system Java installation is needed for that bundled
configuration.

**Liquid** is optional and runs only on selected text. Right-click a selection
and choose **Check selection with Liquid**. The first use downloads its model
and native ONNX Runtime for this Mac; later checks run offline without Python.
Long selections are checked in chunks, with suggestions shown as they arrive.
Settings controls which prose and formatting bodies reach the model, and how
math and other protected content are represented. Review the exact input and
before/after diff before applying a correction.

Liquid currently supports Intel Macs on macOS 12+ and Apple Silicon Macs on
macOS 14+. Model/export research is documented in
[grammar comparisons](docs/grammar-comparison.md); Python is used there only
for developer experiments and model preparation.

## Overleaf

Choose **File → Connect to Overleaf**, also available in the command palette,
and open a project in the app's browser. Sign in there once; Safari's session
is separate. Hide the browser to keep the connection running while you edit
source in the native editor.

Overleaf remains the saved source of truth. With Auto-save enabled, edits are
sent after a typing pause; saving completes only after server acknowledgement.
The local workspace supplies the compiler copy. Choose whether to keep that
copy and where it lives in the connection window. Temporary copies are removed
when no longer in use. Reopening a project requires a successful sync before
editing; offline drafts and conflicts require review rather than silently
overwriting the server.

This integration is experimental and depends on Overleaf's internal frontend
interfaces. Upstream changes or authentication requirements can interrupt it.
It is not an official Overleaf client. See the
[connection and synchronization design](docs/overleaf-protocol.md).

## Useful default shortcuts

Bindings can be changed in Settings. **Cmd/Ctrl** means Command on macOS and
Control elsewhere; **Option/Alt** follows the platform convention.

| Action | Shortcut |
| --- | --- |
| Command palette | Cmd/Ctrl-Shift-P |
| Open / Save | Cmd/Ctrl-O / Cmd/Ctrl-S |
| Open workspace | Cmd/Ctrl-Shift-O |
| Find in the focused editor, Explorer section or terminal | Cmd/Ctrl-F |
| Find and replace | Cmd-Option-F on macOS; Ctrl-H elsewhere |
| Compile and save PDF | Cmd/Ctrl-R |
| Export PDF | Cmd/Ctrl-Shift-E |
| Format document | Option/Alt-Shift-F |
| Toggle comment | Cmd/Ctrl-/ |
| Reveal in preview | Cmd/Ctrl-Shift-J |
| Complete | Control-Space |
| Explorer | Cmd/Ctrl-1 |
| Code / Split / Preview layout | Cmd/Ctrl-2 / 3 / 4 |
| Bottom panel | Cmd/Ctrl-5 |
| Terminal | Control-` |
| Settings | Cmd/Ctrl-, |

Preview-only shortcuts require preview focus; its border indicates focus.
**Escape** returns to the source editor. **Settings → Help** lists the current
preview bindings and other editing tips.

## Build and install on macOS

The installer produces a **universal app for Apple Silicon and Intel**, requiring
macOS 12 or newer. It fetches pinned resources, builds both architectures,
verifies the bundle and copies it to `/Applications/tiptoptyp.app`:

```sh
cargo install cargo-packager cargo-about --locked
./scripts/install-macos-app.sh
```

Quit an existing app before replacing it. The script does not quit or relaunch
it for you. Set `TIPTOPTYP_APPLICATIONS_DIR="$HOME/Applications"` to use your
own Applications folder. You can copy the complete universal `.app` to either
kind of Mac; optional features retain their platform requirements above.

For a smaller single-architecture app:

```sh
./scripts/install-macos-app.sh --target aarch64-apple-darwin
./scripts/install-macos-app.sh --target x86_64-apple-darwin
```

To package and verify without installing, use the same production config and
explicit target as the installer:

```sh
cargo packager --config packager-prod.toml --target universal-apple-darwin
cargo xtask verify-package --target universal-apple-darwin
```

### What is bundled

| Component | Pinned version / distribution |
| --- | --- |
| Typst | 0.15.1 |
| Tinymist | 0.15.2 |
| Tectonic | 0.17.0 |
| TexLab | 5.26.0 |
| Badness | 0.24.0 |
| tex-fmt | 0.5.7 |
| LTeX+ | 18.7.0 |
| Java for LTeX+ | Temurin 21.0.10+7 |
| PDFium | Platform-specific, hash-pinned builds |
| Liquid model + native runtime | Optional download, excluded from the app bundle |

Tool and renderer binaries ship compressed. The app verifies and extracts the
current architecture into its cache when needed; later launches reuse it.
Fonts and handwriting models share a compressed, architecture-independent
payload. Licenses and provenance remain readable in the bundle, including the
project's `LICENSE` and `THIRD_PARTY_NOTICES`. Artifact versions and hashes are
recorded in the [toolchain manifests](toolchain/).

Local bundles use ad-hoc signing. Public macOS distribution requires the
appropriate signing/notarization or App Store setup and compliance with every
bundled component's license. The source license alone does not settle those
third-party obligations.

## Development and testing

Run the required checks from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
```

Use focused deterministic tests while iterating. Native interaction journeys
and framebuffer captures cover behavior and appearance that headless tests
cannot prove. Some integration tests are ignored by default because they need
native input, a real service or external tools. See
[the working agreement](AGENTS.md), [test coverage](docs/testing-coverage.md),
[desktop journeys](docs/desktop-ui-journeys.md) and
[screenshot contracts](docs/ui-qa-screenshots.md).

Profiling is opt-in and saves its metadata under `.tiptoptyp/profiles`:

```sh
cargo xtask profile --scenario settings
cargo xtask profile --scenario large --sampler none
cargo bench -p tiptoptyp-core --bench document
```

See [performance guidance](docs/performance.md) for comparable workloads and
platform limitations. The source editor still uses egui text editing with
whole-buffer layout; large-document performance remains active work.

`cargo clean` removes build artifacts, not documents, preferences or installed
apps. The task runner has its own target directory; clean it separately with
`cargo clean --manifest-path xtask/Cargo.toml` when needed. On macOS, ordinary
`cargo run` also prunes old incremental sessions after the process exits;
`TIPTOPTYP_KEEP_CARGO_CACHE=1` disables that cleanup.

## Further reading

- [Editor and preview architecture](docs/architecture/0002-interactive-editor.md)
- [Bundled toolchain and packaging](docs/architecture/0003-bundled-toolchain.md)
- [PDFium preview](docs/pdfium-preview.md)
- [TeX completions](docs/tex-completion.md)
- [Table and mathematical grid editor](docs/table-editor.md)
- [Git diff editor](docs/git-diff-editor.md)
- [Local file versions](docs/local-file-versions.md)
- [Terminal](docs/terminal.md)
- [Theme provenance](docs/theme-sources.md)

## License

The first-party code and documentation are **source available** under
[PolyForm Perimeter 1.0.1](LICENSE), rather than an open-source license.
You may use and modify the app, including at work, and redistribute it for
permitted purposes. You may not provide a competing product made from the
software. The full license controls these permissions and restrictions.

Calvin Khor retains the right to offer separate commercial licenses, sell the
app through the App Store or other channels, and accept donations. This source
license does not require the author's builds to be free of charge.
Documents and other output you create with the editor are not covered by the
editor's source license.

Third-party code, dependencies, bundled tools, fonts, themes, models, data,
quotations and externally sourced fixtures retain their own licenses. See
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES), the notices in their files and
directories, and the license files shipped in app resources. A commercial
release must also satisfy those separate distribution obligations.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms that preserve the
project's ability to offer commercial releases.
