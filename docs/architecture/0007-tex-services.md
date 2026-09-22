# Native TeX services

Status: implemented, 22 September 2026.

The app owns compilation, editor intelligence, formatting and linting separately.
These are per-window services, with exact document keys and bounded queues. They
never route native TeX through Typst, miTeX or Tinymist's preview protocol.

| Feature | Default | Alternatives / independent controls |
| --- | --- | --- |
| Build | Tectonic | Disable builds; standard LaTeX is an explicit unimplemented engine choice |
| Editor intelligence | TexLab | Disable the service, completion, hover, or its diagnostics separately |
| Formatting | Badness | tex-fmt or disabled; only one formatter at a time |
| Linting | Badness | Independently enabled, even with TexLab or formatting disabled |

All four executables use the existing pinned sidecar/custom-path resolution.
Discovery happens at startup or explicit settings refresh, never during paint.
Missing tools report the affected feature without silently selecting another
provider. Packaging records release URLs and hashes. Tectonic has no native
Windows ARM release: that target uses its official Windows x64 executable.

## Builds and publication

Tectonic's finite build adapter owns a private source mirror and output directory
below the project's `.tiptoptyp`, along with the child process and its logs.
The main buffer overlays its on-disk entry; included files remain disk-backed,
matching the existing Typst policy. Relative paths retain project layout. Build
outputs must never overwrite the user's PDF, source or auxiliary files.
Tectonic manages the TeX/BibTeX reruns; the app does not invent a pass count.
Shell escape is disabled. Package downloads are allowed by default, with an
independent cached-packages-only setting. Private outputs live until the build
session is retired, and canonical PDF bytes are snapshotted before cleanup.

Edits replace queued builds. Switching engines, roots or settings retires the
previous child. Failed or cancelled builds cannot publish an earlier PDF.
The existing compiler publication, artifact-generation and PDF.js paths remain
responsible for preview/export. Standard LaTeX has a typed settings slot and a
clear unsupported result, without pretending that a different executable can
reuse Tectonic's command line. SyncTeX navigation is separate future work.

## Editor services

TexLab supplies completion, hover and editor diagnostics. Its automatic build,
external viewer, ChkTeX and formatter are disabled: the app owns those roles.
Badness runs in LSP mode for versioned diagnostics and formatting. If neither
Badness feature is enabled, its process is stopped. tex-fmt is invoked with
stdin/stdout only for an explicit formatting request; it never edits files.

A concrete TeX coordinator owns these processes. It shares bounded JSON-RPC
framing and standard feature codecs with Tinymist, without a plugin registry.
Every request/result carries the settings generation and document key. Source
changes are coalesced, document switches send close/open, and replies from
previous documents, versions or providers cannot edit the active buffer.
Formatting uses the existing atomic edit/undo and protected save workflow.
Settings changes retire the previous generation and clear its diagnostics;
enabled providers republish fresh reports. UTF-16 LSP positions are converted against the exact source before UI publication.

## Validation contract

Test defaults, independent toggles, routing, private output paths, cancellation,
missing tools, Unicode diagnostics, malformed/stale replies and one-transaction
formatting. Exercise the pinned real tools against private fixtures, including
TeX error/recovery and Typst regression builds. Preserve source/PDF fixtures.
Run the repository's required checks. No full theme-gallery recapture is needed
for service behavior; settings controls receive semantic coverage.

## Upstream contracts

- [Tectonic compile options](https://github.com/tectonic-typesetting/tectonic/blob/master/src/bin/tectonic/compile.rs)
- [TexLab configuration](https://github.com/latex-lsp/texlab/wiki/Configuration)
- [Badness editor setup](https://badness.dev/guide/editor-setup.html)
- [tex-fmt CLI](https://github.com/WGUNDERWOOD/tex-fmt)

## Concrete limits

This implements the editor's existing completion, hover and diagnostic surfaces.
It does not claim every LSP method. Build roots use the existing preview-entry
selector, not automatic TeX magic-comment discovery. Included files are read from
disk, with saves and explicit builds triggering a refresh; external dependency
watching and multi-buffer overlays remain future work. Build diagnostics retain
raw output, and known fatal wrapper messages become details of the located error.
TexLab can publish unversioned diagnostics under LSP; those are accepted only for
the current URI/session and cleared on edits. Versioned diagnostics reject older
versions. Every formatting/completion/hover response has an exact request key.

The pinned real sidecars are Tectonic 0.17.0, TexLab 5.26.0, Badness 0.24.0 and
tex-fmt 0.5.7. Archives are verified against the hashes published in official
release metadata. `toolchain/tex-licenses.tsv` records each upstream license URL
and hash; packaging verifies and includes them alongside binary provenance.

Only macOS arm64 runtime behavior has been exercised here. Windows and Linux
have pinned release mappings and portable Rust implementations; this is not a
claim of native GUI or runtime validation on those platforms. Unix cancellation
kills the process group, including custom wrappers. Windows currently reaps the
direct tool process; these adapters disable external builds/viewers/shell escape,
but custom wrappers that spawn descendants need a future Windows job-object owner.


## Regression and runtime evidence

The deterministic suite covers independent controls, native TeX/Typst preview
selection, UTF-16 locations (including split-surrogate rejection), preservation
of all diagnostic providers in mixed tabs, exact reply admission, one-transaction
formatting/undo, private output cleanup, cancellation and dotted entry names.
Build request generations also reject old engine/root results at the same editor
revision. Location conversion sorts requested positions then walks source once;
no whole-document index is rebuilt per diagnostic. Ordinary Typst documents do
not start the TeX coordinator, and a stopped coordinator blocks on its queue.

Real macOS arm64 sidecar tests exercise TexLab command completion and hover,
Badness lint publication and formatting, and tex-fmt formatting while leaving the
source file untouched. Tectonic builds unsaved source with a relative `\input`,
reports a located undefined-command error, recovers, and cleans its private
workspace. First-use downloads hit upstream timeouts during cache warmup; a
subsequent run completed. Replacement and shutdown remain cancellable during
network work, and a stalled build has a ten-minute ceiling.

The settings controls are covered with semantic egui tests. No screenshots were
retaken, and this work does not claim fresh framebuffer/native composition QA.
Local logs, binary hashes and matched optimized Typst measurements are retained
in `.tiptoptyp/tex-services-evidence/` (ignored runtime evidence).

Required checks pass: formatting, strict all-target Clippy, 1,198 tests (23 opt-in
tests excluded from the normal suite), and all 14 xtask tests. The exact real-tool
probes also pass. The ongoing edits to `todo.typ` in master are kept separate
from this integration; the standard LaTeX follow-up is recorded above.

The matched optimized Typst success/error/recovery cycle measured a median of
1.050 s before and 1.041 s after (-0.8%). Each stage used one
warmup and five samples with Rust 1.98.1, Typst 0.15.1 and the same fixture on
macOS 14.6.1 arm64. This is a headless compiler-worker observation, not a GUI or
cross-platform performance guarantee. No material Typst performance regression
was observed. New TeX services have no pre-existing runtime baseline.
