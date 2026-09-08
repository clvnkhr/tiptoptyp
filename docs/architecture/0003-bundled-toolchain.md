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
`toolchain/manifest.tsv`; changing a version requires changing that audited
manifest and the runtime version constants together.

The packaging task downloads only HTTPS GitHub release assets, verifies the
committed digest before extraction, rejects absolute and parent-traversing
archive members, and streams only the exact allowlisted executable/license
members to owned staging files. It never gives `tar` an extraction directory,
rejects link and special-file entry types, and therefore cannot write elsewhere
before validation. Native executables are checked with `--version`.
cargo-packager treats the results as external binaries, including them in its
normal platform packaging and signing flow.

Linux packages use Typst's static MUSL release payload staged under the GNU app
target expected by the packager. Tinymist continues to use its matching GNU
payload. macOS packages are architecture-specific for now; a universal app
would require combining all three executables before signing.

Cross-target packaging sets `TIPTOPTYP_PACKAGE_TARGET` to the same triple passed
to cargo-packager. This explicitly propagates the target to the package hook;
cargo-packager itself exposes formats, but not its CLI target, to hook commands.

### 2. Bundled is a preference, not an invisible assumption

Typst and Tinymist have independent persisted choices: Bundled (default) or
Custom path. Resolution order is the valid requested custom path, bundled
sidecar beside the running app, source-tree staging path, development
environment override, and finally `PATH`. Commands receive a resolved absolute
path whenever a real executable exists.

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
