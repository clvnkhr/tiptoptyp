# PDFium comparison backend

Select **Settings → Preview backend → PDFium (comparison)** to use native PDFium
for compiled Typst/TeX output and opened PDF tabs. Select **PDF.js** to compare.
Defaults remain unchanged: Tinymist for Typst, PDF.js for TeX/opened PDFs. The
comparison setting is persisted and applies to both source previews and PDF tabs.
Export always uses the original compiler PDF bytes; PDFium does not rewrite them.

This is an additional viewer, not a replacement of PDF.js. It paints in the egui
viewport, so it works without a PDF webview and its pixels are included in app
framebuffer captures. Native libraries are pinned for macOS, Linux and Windows,
arm64 and x64; the implementation has been exercised on macOS arm64. Other
platforms need packaged smoke tests before making a support/performance claim.

## Usage

The viewer provides continuous scrolling, horizontal pan, page entry/previous/next,
zoom/fit width, pinch zoom, outlines, internal page links and external HTTP(S)/mail
links. Drag over text within a page and copy it; the page context menu also offers
Copy page text. Find is case-insensitive, highlights visible matches and navigates
matching pages. With a PDF page focused, Find, Copy and Select All target the PDF;
Select All selects the current page. Source synchronization remains a Tinymist
feature. Search results report matching pages, not individual match counts.

Comparison limitations: PDF.js still supplies the richer viewer (thumbnails,
cross-page selection, individual search-result navigation and viewer rotation
controls). PDFium respects rotation already encoded in a PDF. No form editing or
PDF JavaScript is enabled. A failed PDFium update retains old content and shows a
retry message; it never silently changes engines. Password entry is not implemented.

## Reload and scheduling

One latest-wins worker per visible PDF surface owns the parsed document and retains
it between viewport/zoom requests. All PDFium calls use the binding's process-wide
serialization. Background work never reads UI state. Requests/results carry an
immutable revision, page demand and render scale; superseded results cannot commit.
The mailbox contains at most one pending request and one completed render batch.

Only visible pages are rendered. A successful batch supplies pixels, character
geometry and links together. The UI uploads and replaces that batch in one frame,
retaining the previous textures throughout preparation. The scroll container and
controls survive document updates; a page-relative anchor is restored when page
geometry changes. Search runs after the visible batch is published and caches its
page results across viewport requests. There is no new idle polling or repaint loop.

PDF bytes are passed in memory, with no temporary PDF, process launch, PNG encoding
or browser transport in the preview path. Opening a changed PDF still parses a new
document; there is no claimed cross-revision PDF object cache. Cancellation occurs
before parsing, during catalog/text processing and between page renders. An active
native page render is not yet interrupted through PDFium's progressive API.

Limits: 256 MiB input, 10,000 pages, 100,000 extracted characters per page,
approximately 16 megapixels per page and 24 megapixels per demanded batch. Retained
GPU pages are restricted to that batch; old/new textures coexist during handoff.
PDFium's internal decoded resources are additional memory. These limits do not
constitute a sandbox or a guarantee against all malformed-document resource costs.

## Bundling

Source builds prepare the library with:

```sh
cargo run --manifest-path xtask/Cargo.toml -- fetch-pdfium
```

`package-build` does this automatically. The library is PDFium chromium/7881,
paired with `pdfium-render = 0.9.4` and its `pdfium_7881` ABI feature. The download
has a per-target SHA-256 pin in `xtask/src/pdfium.rs`; extraction validates archive
paths and entry types. The library, provenance and dependency licenses are included
in the app's `pdfium` resources, and notices are included in THIRD_PARTY_NOTICES.
Runtime loading only checks app resource locations and, in development builds,
the repository's generated bundle. There is no PATH/system library fallback or
runtime download. MuPDF is not an application dependency.

## Validation and comparison

```sh
cargo test --bin tiptoptyp pdfium -- --include-ignored --skip native_fixture_probe
cargo run --release -- --ui-theme catppuccin-latte \
  --ui-snapshot-scene pdfium-preview \
  --ui-screenshot-subdir screenshots/agent-review/pdfium \
  --ui-screenshot-exit scripts/fixtures/pdfjs.typ
```

Native regressions check actual pixels, link extraction, document reuse,
superseded requests, failure recovery, and retention of the displayed texture
through a changed/failed document. Pure tests cover visible-page demand, pixel
limits, Unicode search mapping and routing without Tinymist/Hayro duplication.
The maintained gallery includes the actual PDFium viewport scene.

For a fair interactive comparison, use the same file, viewport, theme, page and
zoom in both backends, then edit while scrolled down. Check page additions/removals,
selection, search, links and failed compilation. Distinguish compilation latency
from rendering. The earlier Python engine probe is not an end-to-end benchmark
of this viewer, and no speedup over the existing staged PDF.js viewer is claimed
from those numbers.

The optimized macOS arm64 worker probe measured 9.03 ms median / 9.49 ms p95
for changed PDFs and 7.30 / 7.75 ms for retained-document viewport requests.
The fresh-worker control measured 9.15 / 9.77 ms. These are worker-only timings
for page 13 of the 24-page fixture at 4.29 pixels per point; they exclude compile,
GPU upload and presentation. Raw samples and metadata are retained in
[the worker results](performance-results/pdfium-worker-2026-09-23.json).
The ignored `native_fixture_probe` test reproduces the measurement with
`TIPTOPTYP_PDFIUM_PROBE_A` and `TIPTOPTYP_PDFIUM_PROBE_B` pointing to the two
24-page fixture PDFs.

A native WebKit/PDFium comparison is now available in
[the reload performance report](pdf-reload-performance.md), with matched engine
preparation timings and separate PDF.js staged-viewer latency.
