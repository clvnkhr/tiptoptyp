# PDFium preview

PDFium is the sole PDF viewer for TeX output and opened PDFs. Typst defaults to
Tinymist for interactive source navigation; select **Settings → Typst preview
backend → PDFium** to view its compiled PDF instead. A selected backend failure
is reported explicitly and never switches renderers. There are no PDF.js or
legacy raster-viewer settings, adapters or assets.

Export uses the original compiler PDF bytes. PDFium paints in the egui viewport,
so framebuffer captures include the actual PDF viewer. Native libraries are
pinned for macOS, Linux and Windows, arm64 and x64. The implementation has been
exercised on macOS arm64; other platforms need packaged smoke tests.

## Usage

The viewer provides continuous scrolling, horizontal pan, page entry/previous/next,
zoom/fit width, pinch zoom, outlines, internal page links and external HTTP(S)/mail
links. Drag over text within a page and copy it; the page context menu also offers
Copy page text. Find is case-insensitive, highlights visible matches and navigates
matching pages. With a PDF page focused, Find, Copy and Select All target the PDF;
Select All selects the current page. Source synchronization remains a Tinymist
feature. Search results report matching pages, not individual match counts.

Current limitations: selection is within one page; search navigation advances
between matching pages. There is no thumbnail strip, cross-page selection,
individual-match navigation, rotation control, form editing, PDF JavaScript or
password-entry UI. Rotation encoded in the PDF is respected. Failed updates
retain the last good page and show an error with an explicit retry action.

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
Startup validates the native library before opening the editor. Package verification
also runs `--check-runtime` from the signed bundle, so a library rejected by macOS
cannot pass by merely reporting `--version`. Local ad-hoc bundles carry the
library-validation entitlement required because they have no Team ID; Developer ID
bundles should sign the app and PDFium with the same team instead. Runtime loading only checks app resource locations and, in development builds,
the repository's generated bundle. There is no PATH/system library fallback or
runtime download. MuPDF is not an application dependency.

## Validation and comparison

```sh
cargo test --bin tiptoptyp pdfium -- --include-ignored --skip native_fixture_probe
cargo run --release -- --ui-theme catppuccin-latte \
  --ui-snapshot-scene pdfium-preview \
  --ui-screenshot-subdir screenshots/agent-review/pdfium \
  --ui-screenshot-exit scripts/fixtures/pdf-preview.typ
```

Native regressions check actual pixels, link extraction, document reuse,
superseded requests, failure recovery, and retention of the displayed texture
through a changed/failed document. Pure tests cover visible-page demand, pixel
limits, Unicode search mapping and routing without Tinymist/Hayro duplication.
The maintained gallery includes the actual PDFium viewport scene.

The optimized macOS arm64 worker probe measured 9.03 ms median / 9.49 ms p95
for changed PDFs and 7.30 / 7.75 ms for retained-document viewport requests.
The fresh-worker control measured 9.15 / 9.77 ms. These are worker-only timings
for page 13 of the 24-page fixture at 4.29 pixels per point; they exclude compile,
GPU upload and presentation. Raw samples and metadata are retained in
[the worker results](performance-results/pdfium-worker-2026-09-23.json).
The ignored `native_fixture_probe` test reproduces the measurement with
`TIPTOPTYP_PDFIUM_PROBE_A` and `TIPTOPTYP_PDFIUM_PROBE_B` pointing to the two
fixture PDFs (`TIPTOPTYP_PDFIUM_PROBE_PAGES` defaults to 24).

A native WebKit/PDFium comparison is now available in
[the reload performance report](pdf-reload-performance.md), with matched engine
preparation timings and separate PDF.js staged-viewer latency.
