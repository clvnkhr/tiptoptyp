# PDF viewer consolidation — 23 September 2026

Rollback checkpoint: `00abc5f` retains both viewers and their comparison harness.

Typst defaults to Tinymist. TeX output and opened PDFs always use PDFium;
Typst can explicitly select PDFium in Settings. Service recovery never changes
the selected renderer. PDFium's pinned native library is validated before the
editor opens; missing/incompatible libraries fail startup.

Removed PDF.js source/assets, embedded HTTP transport, webview adapters,
JavaScript tests, benchmarking harnesses, direct dependencies and notices.
Also removed the legacy full-document raster viewer, compiler-side PDF inspection,
page workers, obsolete settings and screenshot surrogate. Image viewing and
bounded hover thumbnails remain separate. Canonical PDF export is unchanged.
Retired settings values have no aliases. Historical comparison results remain
as evidence, with reproduction tied to the checkpoint.

PDFium rendering and its worker algorithm are unchanged by this consolidation:
visible pages commit as a batch while the last good textures remain displayed.
No extra idle repaint loop or background PDF job was added. Native tests cover
retained pixels across successful/failed updates, supersession and parsed-document
reuse. Routing regressions cover defaults, PDF-first tabs, PDF page jumps and
Tinymist failure without a fallback compile.

Validation on macOS 14.6.1 arm64 (Apple M2 Max, Rust 1.98.1): formatting,
Clippy with warnings denied, 1,215 Rust tests, 15 xtask tests, seven Tinymist
JavaScript interaction tests and 11 focused PDFium tests including native cases.
The full Rust suite leaves 27 opt-in tests ignored; the native cases were run
separately. Cross-platform native packaging has not been exercised here.

## macOS redraw crash found during verification

The gallery's light File menu → dark File menu → light Edit menu sequence
aborted in `egui_glow::Painter::set_texture`: OpenGL returned texture name zero.
The crash report identified the repository capture process, not the installed app.
Glutin 0.32.3's CGL surface check compared only the attached NSView; AppKit's
layer-backed redraw can enter with a different thread-current context. Eframe
then skips rebinding and attempts texture creation on the wrong context.

`vendor/glutin` retains the pinned upstream crate and licenses with a focused
CGL fix: both the current context and attached view must match. A detached view
returns false. The native regression switches two real OpenGL contexts and
checks identity after each switch and clear; CI runs it in the macOS real-tools
job. See [patch provenance](../vendor/glutin/TIPTOPTYP-PATCH.md).

After the patch, two consecutive full gallery runs completed and all 23 PNGs
validated. Fresh framebuffer captures under `.tiptoptyp/screenshots/agent-review/`
were inspected: `pdfium-final` covers the unbranded PDF controls, image tab and
Settings; `pdfium-final-tex` covers a real Tectonic-compiled article. Preview pages,
controls and pane boundaries were populated and unclipped. These are egui
framebuffers, not evidence of Tinymist native child-view composition.

## Matched worker performance

Same release profile, M2 Max, preloaded fixture hashes, middle page at 4.29
pixels/point, five warmups and 30 samples per mode, two runs; no concurrent
build/test/capture workload. Figures pool 60 samples. These measure worker page
preparation, excluding compilation, GPU upload and presentation.

| Workload | Checkpoint median / p95 | After removal median / p95 |
|---|---:|---:|
| Changed PDF, 24 pages | 9.14 / 9.81 ms | 9.00 / 9.64 ms |
| Changed PDF, 240 pages | 16.13 / 16.73 ms | 16.37 / 16.80 ms |
| Retained document, 24 pages | 7.36 / 7.99 ms | 7.44 / 7.71 ms |
| Retained document, 240 pages | 7.42 / 7.90 ms | 7.58 / 8.11 ms |

The small differences do not establish a performance change. PDFium's worker
algorithm was retained; removing the other viewers introduces no duplicate PDF
work. The later OpenGL fix affects UI context binding, outside this worker probe.
[Raw after samples and metadata](performance-results/pdfium-after-removal-2026-09-23.json)
and [baseline](pdf-reload-performance.md) preserve the comparison. This is one
machine and a text/vector fixture, not a cross-platform latency guarantee.
