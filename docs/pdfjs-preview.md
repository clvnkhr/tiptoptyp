# PDF.js preview

Opened PDF tabs use **PDF.js by default**, including beside a designated Typst
preview. Typst documents still default to Tinymist for source/preview
synchronization. Select **Settings → Preview → PDF.js** to use PDF.js for compiled
Typst output too, in Split or Preview view.

PDF.js is also the automatic fallback when Tinymist is unavailable, exhausts its
five consecutive attempts, or its native view fails. The first four failures keep
the existing retry behavior and any retained Tinymist surface. Fallback preserves
the requested preference and reports **PDF.js · fallback** with the failure
reason. It compiles canonical PDF bytes without Poppler page inspection or
rasterization; explicit export uses the same bytes.

Native PDF.js views are available on macOS and Windows. Other platforms report
missing native web-view support rather than silently selecting raster. A PDF.js
load failure exposes its error and retry button, without another backend switch.
An explicit **Rasterised PDF** preference remains available for now and applies
to both compiled previews and opened PDFs.

The bundled Mozilla generic viewer provides continuous scroll, horizontal pan
when zoomed in, zoom buttons and percentage/fit controls, Cmd/Ctrl +/- and 0,
Ctrl-wheel zoom, text selection, find, outlines, and PDF link annotations.
Internal links stay in the document. HTTP(S) and mail links go through the app's
external-link handler. PDF scripts, annotation editing, automatic printing and
opening a different file within the embedded viewer are disabled. Use the app's
Open and Export commands. On macOS, GestureEvents feed trackpad pinch into the
same PDF.js scale as the other zoom controls.

The adapter keeps the displayed PDF while Typst compiles. It receives only
accepted canonical artifacts, downloads the new immutable bytes, and restores
the page/zoom/position when replacing a PDF for the same document. A different
document starts at page one, fit width. A shortened document clamps the restored
page. Theme changes do not reload the PDF. The new viewer does not modify the
Tinymist zoom adapter or the deferred zoom-fallback investigation.

The newest-first todo audit also verified the existing search and thumbnail
controls in the real bundled viewer: 23 matches across a 24-page fixture,
24 thumbnail entries, a decoded thumbnail, and navigation back to page one by
clicking its thumbnail. The same run retained page 13, 175% zoom and scroll
offset 15,294 exactly across reload, with no remote requests or browser errors.
This closes stale todos 118 and 1; it does not close raster removal (295).

## Ownership and cost

`src/pdfjs.rs` embeds the hash-verified upstream distribution in the application.
Each visible PDF surface has one loopback server with a random capability URL;
it serves an explicit embedded asset map and the current PDF, never arbitrary
workspace files. Host/origin validation, a restrictive CSP, and no CORS prevent
unrelated web pages from reading a document. There is no CDN, telemetry, polling
timer or new compile worker. Closing the document/window or changing backend
drops the native view and server together. Code view hides the retained viewer.

Rust shares the canonical `Arc<[u8]>`; unchanged frames compare pointer identity
instead of copying/hashing PDF contents. Old request URLs cannot return newer
PDF bytes. JavaScript serializes loads and collapses superseded downloads. The
upstream rendering queue and page buffer render the visible vicinity; individual
canvases are capped at 16 Mi pixels. WebKit/Chromium and PDF.js still incur their
own document/worker/canvas memory costs, outside the raster residency accounting.
PDF.js mode disables Poppler compilation/catalog work and raster page requests.
No cross-platform speed or physical gesture smoothness guarantee is implied.

## Raster preview retirement (todos 294–295)

The rasterised PDF viewer is pending deletion. Keep it working for explicit
selection and deterministic viewport captures; do not extend it with new viewer
features. PDF.js owns new PDF-viewing work. Removal has these boundaries:

- Replace the raster surrogate used by app framebuffer captures with an explicit
  native PDF.js evidence strategy. Egui framebuffers cannot capture child views.
- Remove the raster preview setting, page/fit controls and shortcuts, PDF page
  demand workers and preview-only residency/texture state. Audit the shared code
  in `app/raster_view.rs`, `preview.rs`, `pdf_pages.rs`, and `pdf_residency.rs`.
- Remove preview-only Poppler inspection, link extraction and rasterization from
  the compiler and asset loader. Retain the canonical PDF artifact/export path.
- Separate image display and PDF hover thumbnails before deleting shared raster
  helpers in `asset.rs` and `pdf.rs`. Audit capability reporting and packaging;
  do not remove Poppler while thumbnail consumers still require it.
- Preserve PDF.js document replacement, retained page/zoom, theme handling,
  links, multi-window ownership and cancellation tests.

Routing regressions cover explicit raster selection, absent platform support,
five-failure recovery, local-view failure, pause/duplicate-error admission, PDF
tabs beside Tinymist and the explicit capture override. No new polling, worker
or per-frame I/O is introduced. This changes backend admission, not the viewers'
rendering algorithms; no new performance or visual-verification claim is made.

Upstream version, archive hash, extraction exclusions and licenses are recorded
in `assets/pdfjs/PROVENANCE.md`. The aggregate package notice includes the viewer,
CMap, ICC, font and WASM licenses.

## Validation

Deterministic checks:

```sh
cargo test pdfjs
node --test scripts/test-pdfjs-host.mjs
```

The optional real-browser driver compiles a 24-page fixture using the bundled
Typst executable and serves it through the actual Rust asset server. Install
Playwright in a temporary QA directory and point `PLAYWRIGHT_MODULE` at it:

```sh
PLAYWRIGHT_MODULE=/path/to/node_modules/playwright node scripts/check-pdfjs.mjs
```

`CHROME_PATH` overrides the installed Chrome executable; `TIPTOPTYP_TEST_TYPST`
overrides the fixture compiler. Browser evidence is written beneath
`.tiptoptyp/screenshots/agent-review/pdfjs/`. These browser captures test the
actual viewer but are not the composed native app or its viewport framebuffer.
The normal app framebuffer capture substitutes raster pages for native child
views, as documented in `ui-qa-screenshots.md`.

On macOS, `PDFJS_NATIVE=1` also compiles and runs the optimized Swift WKWebView
probe against the same Rust server and captures its child-view framebuffer.
On 22 September 2026 the Chrome 153 / macOS arm64 run retained page 13, 175%
zoom and scroll offset 15,294 exactly across a reload; shrinking to one page,
switching documents, internal links and viewport resizing passed. No remote
requests or page errors were observed. The native probe rendered 24 pages,
scrolled, changed the PDF.js scale from 1.5 to 1.8 with synthetic GestureEvents,
and reset to page width. Its fresh PNG was inspected. This tests native WebKit
rendering and event routing, not physical trackpad feel or the composed app.

Manual acceptance: scroll far down, zoom and pan, follow the fixture's first/last
page links, open an external link, and edit the source while scrolled down.
Check that the new PDF keeps your position and zoom. Switch between Code/Split/
Preview, open a PDF tab, resize the split divider and try a macOS trackpad pinch.
