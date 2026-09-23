# Historical research

This document describes the comparison checkpoint `00abc5f`, before PDFium became the sole PDF viewer. Paths and options below refer to that commit.

# PDF preview migration feasibility

Investigation date: 2026-09-23. This is a proposal, not an implemented migration.
Local repository inspected at `aa65b66`. No application or dependency changes
were made for this investigation.

## Recommendation

Do not replace PDF.js with printpdf as the production renderer today. Proceed
only with a bounded, fidelity-first prototype if pursuing a replacement. The
demo demonstrates a useful presentation technique, but its HTML fast path does
not measure parsing and displaying newly compiled Tectonic/Typst PDFs.

The desired contract is seamless replacement of the canonical compiler PDF:
retain usable old content until new visible content is ready, preserve navigation,
never publish stale revisions, and never silently omit PDF content. None of this
requires an engine to mutate an existing parsed PDF in place. A renderer change
does not by itself provide incremental parsing or eliminate document reload work.

## Verified upstream evidence

Inspected printpdf commit
[`6ff6bb5f808f85f39dfe1809e73277ccb2be0a0e`](https://github.com/fschutt/printpdf/tree/6ff6bb5f808f85f39dfe1809e73277ccb2be0a0e),
whose manifest identifies version 0.12.8. The deployed demo's `script.js` was
byte-identical to this revision's script (SHA-256
`3c8dc4a95b3e691fd06ce5806425c06c7cb7ff92cdd9511a8add1bd87b3ad8c2`).
This verifies the JavaScript match, not reproducibility of the deployed WASM.

The [demo source](https://github.com/fschutt/printpdf/blob/6ff6bb5f808f85f39dfe1809e73277ccb2be0a0e/script.js)
shows three distinct paths:

- HTML editing: `Pdf_HtmlToDocument` → in-memory document → `Pdf_PageToSvg`
  for every page → replace page DOM. PDF serialization occurs on download via
  `Pdf_DocumentToBytes`, not on every preview update.
- PDF import: uploaded bytes → `Pdf_BytesToDocument` → the same SVG path.
  This is the relevant path for our compiler outputs.
- HTML reference view: the original HTML is assigned to an iframe's `srcdoc`.
  That view is a browser HTML rendering, not a PDF rendering.

The demo debounces edits by 400 ms, serializes renders and coalesces requests.
It prepares all SVG strings before clearing/rebuilding the viewer. It renders
every page and duplicates smaller SVGs into thumbnails, with a size guard for
large ones. This is not a virtualized, changed-pages-only rendering algorithm.
Preparing strings also does not prove that fonts, images and painting are ready.

The [renderer source](https://github.com/fschutt/printpdf/blob/6ff6bb5f808f85f39dfe1809e73277ccb2be0a0e/src/render.rs)
contains production-blocking concerns for our no-compromise requirement:

- `Op::PaintShading` does nothing, explicitly skipping gradients silently.
- Inline-image operations report that SVG rendering is not fully implemented.
- Imported glyph IDs are converted to Unicode SVG text; missing Unicode mappings
  become U+FFFD. Rendering a glyph correctly must not depend on its extractable
  Unicode mapping. Mathematical and subset fonts require direct testing.
- Fill/stroke color-space operations have empty TODO branches.

These are source findings, not measured failure rates across a PDF corpus.
The [README](https://github.com/fschutt/printpdf/blob/6ff6bb5f808f85f39dfe1809e73277ccb2be0a0e/README.md)
also labels SVG rendering, text extraction and portions of font/image reading
experimental. Rust/WASM support and the MIT license make integration possible;
they do not establish rendering parity or speed.

## Current baseline and alternatives

Our production implementation already retains the native webview and stages a
second PDF.js viewer before committing a replacement. It preserves navigation,
search settings and sidebar state, retains the successful viewer on failure,
and bounds the surface count at two. Identical PDF bytes do not trigger reload.
See [the current architecture](pdfjs-preview.md).

Existing [local measurements](performance-results/backlog-2026-09-23.json):
24-page warm Chrome fixture, 900×700, device scale 2, optimized build: old direct
replacement 118.9 ms with seven sampled blank frames; staged replacement
205.6 ms with zero sampled blank frames. Three native WebKit replacements also
reported zero blank frames. An earlier native probe timed out for an unresolved
reason. These are limited historical measurements, not a new printpdf comparison
or a guarantee of reliability. Staging spends extra memory and time for continuity.

| Direction | Benefit | Main cost / gate |
| --- | --- | --- |
| Retain staged PDF.js | Existing text, search, links and full viewer; least migration risk | Diagnose timeout; measure transient memory and update latency |
| Custom viewer using PDF.js page APIs | Persistent page containers and app-owned replacement policy; reuse existing decoder | Rebuild/adapt viewer controls, search, text, links and accessibility; still parses new PDFs |
| Custom viewer using printpdf SVG | Rust parser/renderer with browser vector presentation | Known fidelity gaps; substantial viewer work; no demonstrated performance advantage |
| Custom viewer using existing Hayro | Reuses bundled Rust renderer and PDF utilities | Existing integration supplies raster pages/links, not our full text/search viewer; parity remains a separate project |

For an initial architecture experiment, a custom PDF.js page surface isolates the
presentation improvement from a simultaneous decoder replacement. If removing
PDF.js itself is the objective, compare printpdf and Hayro behind the same small
prototype interface before committing to either. Do not expand the retiring
raster viewer or assume that SVG automatically gives accurate selection/search.

## Phased migration plan and decision gates

### 1. Establish the acceptance corpus and baseline

Use canonical PDFs generated locally by the bundled Typst and Tectonic tools,
plus redistributable imported PDFs. Include text, equations, ligatures, subset
fonts, CJK/RTL, missing ToUnicode maps, TikZ gradients, clipping, transparency,
images, links, outlines, rotation and mixed page sizes. Cover 1, 24, 100 and
500 pages, and rapid edits that add/delete/reflow pages. Include corrupted inputs.
Never upload local documents to the public demo.

Replay identical changed PDF bytes into each candidate. Record cold startup,
warm byte-arrival-to-visible-commit latency, p50/p95 over repeated samples,
blank frames, main-thread stalls, peak/settled memory, idle work and cancellation
latency. Separate compiler time from renderer time. Preserve fixture hashes,
engine versions, release profile, machine, viewport, scale, warmup and workload.
Investigate the outstanding WebKit timeout before claiming a reliable baseline.

Gate: an agreed repeatable workload and baseline. Proposed acceptance is zero
blank/stale commits in a 100-update stress run, no idle polling, memory bounded
by explicit cache budgets, and no correctness regressions. Choose numerical
latency/memory budgets from these measurements rather than inventing guarantees.

### 2. Build a disposable printpdf import prototype

Use the native Rust parser and SVG renderer on a background worker, presenting
results in a local persistent webview. Keep the compiler's original PDF bytes
for export: never parse/reserialize the exported document through printpdf.
Disable default HTML-generation features where the import/render build permits;
enable and test the image/font capabilities actually required. Pin the exact
candidate version and audit its resolved dependencies/licenses/build footprint.

Start with one visible page and its text/links. Compare fresh renders against
independent PDF output and inspect material differences. Test decoded text,
glyph placement, link bounds and copy order separately from visual comparison.
Exercise actual imported PDFs, not printpdf-generated documents alone.

Gate: stop if shading, glyphs, images or clipping are missing. Zero warnings is
not a fidelity test: shading is currently skipped silently. Fix upstream or
budget an explicitly maintained fork before continuing. A silent PDF.js fallback
does not satisfy complete migration or the no-compromise requirement.

### 3. Introduce a small backend boundary

Separate canonical artifacts from viewer presentation. Use document identity,
monotonic revision and page index in all requests/results. Define operations for
opening immutable bytes, page geometry, visible-page output, text geometry,
links/outlines and disposal. Keep parsing objects owned by a worker; avoid
serializing full documents/resources through JSON once per page as in the demo.

Likely integration points: `src/pdfjs.rs` (transport/artifact publication),
`src/app/pdfjs_view.rs` (native view ownership), `src/pdfjs/host.js` and
`frame.js` (presentation/state), `src/preview.rs` (routing) and `src/pdf.rs`
(shared PDF utilities). Leave Tectonic/Typst compilation and Tinymist source
preview intact. Backend abstraction should follow the prototype's actual needs,
not become a general-purpose PDF framework.

### 4. Implement bounded, transactional page replacement

Maintain persistent page containers, old visible output and one pending revision.
Parse away from the UI thread; prioritize the visible region and a small prefetch
window. Render thumbnails lazily. Abort/drop superseded jobs where possible;
check revision on every result even when parsing itself cannot be interrupted.
Cap document bytes, pages, decoded images, SVG output, caches and pending work.

Prepare the new visible region offscreen, including fonts/images, text and links.
Commit its geometry and content together after readiness; never mix interactive
layers from different revisions. Preserve page-relative position, zoom, rotation,
search, sidebar and keyboard focus; re-sample navigation immediately before commit.
Handle changed page sizes/counts and user navigation into an unprepared region.
Retain the previous revision on errors. A failed new revision must never appear
to be the current successful one.

Begin with full parsing plus visible-region staging. Add unchanged-page reuse
only after correctness: cache identity must include content, inherited geometry,
fonts, images and other referenced resources, not merely page content streams.
Neither printpdf's demo nor our compiler pipeline supplies safe PDF change sets.

### 5. Reach viewer and platform parity

Implement/test continuous scroll, pan, fit modes, anchored pinch/zoom, text
selection/copy, cross-page search/highlights, outlines, thumbnails, internal
destinations and external-link routing. SVG text presence alone is insufficient.
Specify selection behavior across edits; semantic selection cannot always survive
when selected text disappears. Keep sidebar closed initially and preserve settings
on same-document updates. Test PDF tabs, source preview ownership, splits, popouts
and multiple windows. Do not describe Tinymist source synchronization as something
a new PDF renderer automatically provides.

Bundle all assets offline. Treat generated SVG and embedded links as untrusted:
sanitize/allowlist elements, attributes, URLs and resource references; apply CSP,
deny external resource fetches and route permitted links through the app.
Retain capability-scoped transport and immutable revision URLs. Test malformed
PDF/SVG inputs and resource exhaustion. Audit accessibility/keyboard behavior.

Validate macOS WKWebView and Windows WebView2 separately. Linux currently lacks
the native PDF.js path; a webview-based replacement does not automatically solve
that platform gap. Either scope parity to existing supported platforms or fund
Linux hosting explicitly. Geometry traces and approved whole-window observation
are needed for native composition; root framebuffer captures are insufficient.

### 6. Switch only after the gates pass, then remove old code

Keep experimental selection internal during evaluation. Before changing defaults,
run corpus, behavioral, security and repeated reload/memory tests; inspect fresh
visual evidence; run repository-required checks and packaged offline smoke tests.
Compare performance under the same baseline conditions. No silent fidelity loss,
unbounded work or reproducible update hang is acceptable.

After acceptance, route opened PDFs, TeX PDFs and Typst PDF fallback through the
new backend. Remove obsolete PDF.js assets/adapters/settings rather than keeping
compatibility aliases. Update notices, packaging, preview docs and QA drivers.
Resolve todo 295's shared thumbnail and screenshot dependencies before deleting
the retiring raster viewer. Preserve immutable compiler artifacts and export.

## Scope and remaining uncertainty

The source audit establishes that printpdf is not presently a drop-in replacement.
It does not establish its latency, memory usage or failure rate on our corpus;
no new candidate binary, browser benchmark or visual capture was run. No runtime
code changed, so application test suites were not rerun for this document.

Planning scale: corpus/prototype work is days; a complete custom interactive
viewer is multiple weeks of work, with renderer-correctness fixes potentially
dominating that estimate. Re-estimate after phase 2. The sensible next deliverable
is a scored corpus comparison and a go/no-go decision, not a default-backend switch.
