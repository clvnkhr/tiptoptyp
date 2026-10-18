# Bundled toolchain and editor-display settings

Status: accepted, 2026-09-02

## Context

The initial MVP launched `typst` and `tinymist` by bare command name. That made
results depend on a shell's `PATH`, allowed incompatible versions to drift, and
made a packaged GUI launched from Finder especially unreliable. It also gave
the user no supported way to select a project-specific executable. Separately,
the editor forced unwrapped lines and had no line-number gutter.

The upstream release and packaging mechanisms were inspected before this
decision:

- [Typst 0.15.1](https://github.com/typst/typst/releases/tag/v0.15.1) publishes
  platform CLI archives containing its binary, license, and notice.
- [Tinymist 0.15.2](https://github.com/Myriad-Dreamin/tinymist/releases/tag/v0.15.2)
  publishes separate `tinymist` and `tinymist-viewer` artifacts. tiptoptyp needs
  the former: its LSP process embeds and serves the interactive preview.
- [cargo-packager external binaries](https://docs.rs/cargo-packager/latest/cargo_packager/config/struct.Config.html)
  use target-suffixed staging files, remove the suffix in the package, and put
  sidecars beside the main executable.
- egui 0.36 passes the post-margin available width to a custom `TextEdit`
  layouter. Its visual rows expose `ends_with_newline`, which is the stable
  boundary needed to map wrapped rows back to logical source lines.

## Decisions

### 1. Package a pinned, matched toolchain

Packages ship Typst 0.15.1 and Tinymist 0.15.2. These were the latest stable
releases on the decision date and share the 0.15 Typst generation. The exact
per-target archive names and SHA-256 digests live in
`toolchain/manifest.tsv`. Since 2026-09-17, `build.rs` derives the runtime labels
from that manifest at compile time; there is no second version literal to update
and no runtime manifest read. Missing tools, duplicate tool/target rows, malformed
rows and disagreements between a tool's target versions fail the build. The shared
validator is covered by `tests/toolchain_manifest.rs`. Resolution precedence is
unchanged; these build-time values are not process-environment overrides.

The packaging task downloads only HTTPS GitHub release assets, verifies the
committed digest before extraction, rejects absolute and parent-traversing
archive members, and streams only the exact allowlisted executable/license
members to owned staging files. It never gives `tar` an extraction directory,
rejects link and special-file entry types, and therefore cannot write elsewhere
before validation. Native executables are checked with `--version`.
The verified executable payloads are Zstandard-compressed (`.zst`) under
`Contents/Resources/toolchain/<target>/` for macOS packaging. A per-target
manifest records each payload's uncompressed size and SHA-256. Before first use,
the runtime verifies and extracts the selected architecture into its private
cache, using executable permissions for tools and preserving the native code
signature. Readable licenses and upstream archive provenance remain separate
bundle resources. Cache hits avoid decompression; compressed payloads are never
executed or modified inside the signed app bundle.

Packaging uses deterministic level-19 compression with a declared content size
and frame checksum. Runtime and package verification share a statically linked
Zstandard decoder, bounded to an 8 MiB history window and the manifest's decoded
size. They reject malformed/truncated frames, checksum or SHA-256 mismatches,
trailing data and concatenated frames. Native code signatures and decoded assets
are unchanged; existing content-addressed cache entries remain reusable.

Linux packages use Typst's static MUSL release payload staged under the GNU app
target expected by the packager. Tinymist continues to use its matching GNU
payload. macOS packaging defaults to `universal-apple-darwin`: the hook builds
Apple Silicon and Intel Rust executables in distinct target directories,
validates both with `lipo`, and publishes their combined executable at
`target/release/tiptoptyp` before bundle signing. Both architectures of every
helper and PDFium are included as compressed resources; the runtime selects the
architecture of the running executable. There is no Rosetta requirement on an
Intel Mac or on an Apple Silicon Mac running the native slice.

Single-architecture packaging remains explicit. Set `TIPTOPTYP_PACKAGE_TARGET`
to the same triple passed to cargo-packager, or use the installer's `--target`
option, which sets both. cargo-packager exposes formats, but not its CLI target,
to hook commands. Universal and thin macOS bundles use the same canonical
`target/release/tiptoptyp.app` destination. Direct `cargo build` and `cargo run`
remain normal development builds for the compiler's selected architecture.

### Shared fonts and handwriting models

Since 2026-10-17, production packages store the 11 fallback/terminal/icon fonts
and two handwriting models once under `Contents/Resources/assets/shared/`.
`build_support/bundled_assets.rs` defines the single name/source registry used by
both staging and runtime lookup. Each Zstandard resource has an uncompressed size and
SHA-256 in `manifest.tsv`; no glyphs, variable font axes or model samples are
removed. The two executable slices no longer each embed these large assets.

All fonts are validated and loaded before the first interface frame, then their
immutable bytes are retained once across theme changes. Handwriting models stay
lazy on the existing recognition worker. Its existing decoded-model cache owns
the result, and temporary raw bytes are released after decoding. Missing or
corrupt production resources report an error; production does not fall back to
the source checkout. Development and unit-test builds retain embedded assets.

Shared resources use `~/Library/Caches/dev.tiptoptyp.editor/payloads/shared/` on
macOS, followed by the content hash and filename. Both CPU slices reuse that
same cache. Directories are private and shared data is non-executable. Expansion
uses the same bounded, verified, atomic publication as tool payloads. The signed
app bundle remains unchanged; only the user's cache is written.

`python3 scripts/test-packaged-runtime.py` checks a completed universal bundle
without opening a window. Temporary homes establish first-use and later-cache
states, verify selected helper/library architectures and permissions, reject
rewritten cache hits, and verify font reuse across CPU slices while both models
remain unexpanded. It checks the bundle signature before and after. Rosetta
availability is recorded separately from native execution. The pure failure
contracts run with `python3 scripts/test_packaged_runtime_contract.py`; neither
check substitutes for GUI startup or input-latency measurements.

### 2. Bundled is a preference, not an invisible assumption

Typst and Tinymist have independent persisted choices: Bundled (default) or
Custom path. Resolution order is the valid requested custom path, bundled
compressed payload in the app's architecture-specific resources, development
source-tree staging path, development environment override, and finally `PATH`.
The compressed resource is a resolution token; tool launch expands it to a
verified absolute executable path on the existing worker.

Any route other than the requested one retains a reason. Settings displays the
requested/effective source and executable path, and the fixed bottom status bar
gains a warning. An invalid custom path therefore never fails or changes
preference silently.
Changing Typst causes the next compile request to replace the watcher; changing
Tinymist restarts its LSP generation.

### 3. Keep `typst watch` authoritative

Bundling changes only how the executable is selected. The canonical compile
pipeline remains `typst watch` against the project-local in-memory shadow
document. Sessions persist where filesystem notifications support the private
path and restart for buffer edits on macOS. Tinymist remains a separate
interactive-preview sidecar.

### 4. Treat Poppler as an explicit recovery dependency

The primary interactive path no longer needs a system Typst or Tinymist.
Poppler is not bundled in this change: it is used only for the raster PDF
recovery viewer and has a materially different packaging/licensing surface.
Its availability and fallback state remain visible in Settings.

### 5. Wrap and number without modifying editor text

Line wrapping and line numbers are persisted booleans, both enabled by default.
Wrapping feeds egui's supplied width into the official syntax-highlighted
`LayoutJob`; disabling it restores the horizontal scroll surface and infinite
layout width.

Line numbers are painted in a reserved `TextEdit` margin. They are never
prepended to the buffer or highlight job, so byte/character offsets used by
selection, find/replace, diagnostics, and Tinymist remain exact. A shared
logical-line-to-visual-row map uses `PlacedRow::ends_with_newline`; line numbers
use the first wrapped row while diagnostic backgrounds cover all wrapped rows
and virtual messages anchor after the last one.

## Verification requirements

- Every supported target has exactly one pinned Typst and Tinymist artifact
  with a 64-character SHA-256 digest.
- Corrupt downloads and unsafe archive listings fail before extraction.
- Bundled and valid custom executables have no fallback; invalid custom,
  environment, `PATH`, and missing states always have one.
- A Typst executable change cannot reuse the old watcher session.
- Old settings JSON defaults wrapping and line numbers to enabled; explicit
  disabled values round-trip.
- Wrapped Unicode text preserves the exact galley text and terminal character
  offset; unwrapped text has one visual row per logical line.
- Diagnostics on a wrapped logical line decorate its complete visual-row range.

## Rejected alternatives

- Downloading tools from `build.rs` would make ordinary compilation depend on
  the network and hide supply-chain work in an unexpected phase.
- Depending only on `PATH` cannot make a GUI package reproducible and varies
  with how the app is launched.
- Prepending line numbers to the source or syntax layout would invalidate every
  source offset and break interactive preview navigation.
- Bundling `tinymist-viewer` would add a second viewer executable but would not
  satisfy the existing LSP and source-navigation protocol.
