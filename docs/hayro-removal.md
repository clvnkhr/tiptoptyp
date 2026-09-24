# PDF thumbnail renderer consolidation (24 September 2026)

PDF hover thumbnails now use the existing process-wide PDFium engine on the
existing asset worker. Document viewing and thumbnails share RGBA extraction.
The worker still renders only the first page, preserves aspect ratio, clears to
white, bounds allocation, and discards superseded results. No subprocess,
workspace storage, extra worker, or idle repaint loop is introduced.

Removed Hayro and 19 other resolved package entries, with no package additions.
Removed its supplemental licenses and regenerated third-party notices. Historical
renderer comparisons remain as historical evidence. This is a dependency-count
reduction; a matched application-size comparison was not performed.

The native regression covers exact RGBA colors, white backgrounds, page rotation,
invalid inputs, pixel limits, and cancellation at every phase. Pure geometry and
preflight tests do not need the native library. Native tests run in the existing
PDFium-equipped CI lane; the environment-driven timing probe is opt-in.
Cancellation cannot interrupt an active native page render. PDFium's existing
thread-safe binding coordinates thumbnail and document calls.

Validation: formatting, strict Clippy, the full test suite (1,232 passed), xtask
(15 passed), and focused PDFium tests including native rendering (5 passed).
The fresh Catppuccin Latte asset-hover framebuffer was inspected at
`.tiptoptyp/screenshots/hayro-removal/1790239118168-0001-asset-hover-pdfium-thumbnail-ready.png`.
The rendered page contains text, Unicode, and math, with the expected white page,
aspect ratio, and unclipped card edges. This framebuffer verifies the thumbnail
viewport, not whole-desktop native composition. An earlier capture caught the
loading state and was not used as completed-render evidence.

After that validation, PDF hover inner padding was reduced from 9 to 4 logical
pixels and outer margin from 8 to 4, halving spacing with whole-pixel rounding.
This final spacing-only adjustment was not retested or recaptured, as requested.

## Repeat the optimized worker probe

Compile `manual-tests/typst-preview.typ` to a PDF using the bundled Typst, then:

```sh
TIPTOPTYP_PDF_PROBE=/absolute/fixture.pdf \
TIPTOPTYP_PDF_PROBE_PNG=/absolute/thumbnail.png \
CARGO_INCREMENTAL=0 cargo test --release --bin tiptoptyp \
  pdf::probes::thumbnail_probe -- --ignored --nocapture
```

The probe performs four first-page parse/render operations at a 720-pixel long
edge. Filesystem caches are warm; run zero is cold within the process and includes
engine initialization, while runs one through three reuse the engine. Each run
opens a fresh PDF. It excludes disk input, PNG encoding, GPU upload, and display.
There is no viewport or theme in this worker measurement. The GUI capture is
separate. These few samples on one small document do not establish large-document
or cross-platform performance, app startup time, or end-to-end hover latency.

Measured on macOS 14.6.1 / Apple M2 Max, Rust 1.98.1, release with thin LTO:

| Renderer | First operation | Subsequent operations |
| --- | ---: | --- |
| Hayro (before) | 5.280 ms | 3.342, 3.139, 3.485 ms |
| PDFium (after) | 27.162 ms | 5.812, 5.282, 7.024 ms |

Both produced 508 × 720 pixels; the after PNG was inspected. PDFium is slower
in this small probe, with a larger one-time initialization cost. Normal app
startup already initializes PDFium before hover rendering. The tradeoff is
removing a duplicate renderer and its dependency graph, not a speed improvement.
Raw measurements and fixture hash are in
[the retained metadata](performance-results/hayro-removal-2026-09-24.json).
Local logs and rendered images are under `.tiptoptyp/hayro-removal/`.
