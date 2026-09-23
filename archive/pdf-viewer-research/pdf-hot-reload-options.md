# Historical research

This document describes the comparison checkpoint `00abc5f`, before PDFium became the sole PDF viewer. Paths and options below refer to that commit.

# Fast PDF hot reload: renderer and architecture options

Investigated 2026-09-23. Research and an isolated engine probe only; application
code and dependencies are unchanged. See also [printpdf investigation](printpdf-migration-plan.md).

## Recommendation

Prototype a persistent native page surface with **PDFium**, benchmarked against
**MuPDF** and a persistent PDF.js page surface. MuPDF is the leading measured
engine in the small probe below; choose it for a production prototype if its
AGPL/commercial licensing fits the project. PDFium is the default migration
candidate when that licensing choice is undesirable. This is an experiment
priority, not a claim that one engine is universally fastest.

Retain Tinymist's compiler-aware preview for Typst. Its incremental representation
offers an opportunity that a generic PDF decoder cannot infer from freshly
serialized documents. TeX and arbitrary opened PDFs still need a PDF engine.

The main architectural change is to retain the viewer and update only the visible
page surfaces, with generation-tagged text and links, instead of constructing a
fresh generic viewer for every compile. Faster parsing alone does not supply
seamless updates, and retaining old pixels alone does not make new content arrive
sooner. Measure both continuity and freshness.

## What the native probe measured

[Raw samples and metadata](performance-results/pdf-engine-candidates-2026-09-23.json)
and [reproduction script](../scripts/benchmark-pdf-candidates.py) are retained.

macOS 14.6.1 / arm64, native binary wheels, separate sequential engine processes.
For each document size: alternate two genuinely changed PDFs from the existing
Typst viewer fixture; bytes already in memory; reopen the document, obtain the
middle page, render it to a 1800×2358 three-channel bitmap, extract plain text.
Five warmups then 40 samples. Text assertions verify the alternating revision.

| Engine | Pages | Total p50 | Total p95 | Open/page p50 | Render p50 | Text p50 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| MuPDF 1.26.10 | 24 | 2.90 ms | 3.38 ms | 0.34 ms | 2.04 ms | 0.48 ms |
| PDFium 153.0.7999.0 | 24 | 6.64 ms | 7.56 ms | 0.96 ms | 5.48 ms | 0.17 ms |
| MuPDF 1.26.10 | 240 | 4.06 ms | 4.42 ms | 1.36 ms | 2.17 ms | 0.52 ms |
| PDFium 153.0.7999.0 | 240 | 7.22 ms | 7.91 ms | 1.50 ms | 5.49 ms | 0.17 ms |

Binding versions: PyMuPDF 1.26.5, pypdfium2 5.13.0, system Python 3.9. The default
free-threaded Python could not install PyMuPDF; a separate temporary environment
with binary wheels was used. These are the tested versions, not assertions about
the latest engine releases. The application would use native Rust/C interfaces,
not launch Python or an executable per render.

Limitations: one simple, text-heavy Typst fixture family, one machine, warm engine
processes, no visual fidelity evaluation. Timings exclude compilation, file reads,
document teardown, GPU upload, UI presentation, selection-layer construction,
full-document search and memory measurement. First-iteration samples are recorded
separately but exclude module startup. No TeX or complex-image benchmark yet.
Different antialiasing and text-extraction implementations are not fidelity parity.
Do not compare these numbers to the historical 205.6 ms end-to-end staged PDF.js
result or claim a corresponding speedup. Both native engines warrant further work.

## Candidate assessment

### MuPDF: strongest measured engine candidate, licensing decision required

Its display lists let us interpret a page once and replay it for zoom levels,
tiles, rendering and text/search. Cloned contexts can share caches; document
access still needs serialization, while prepared display lists can be rendered
on other threads. This is useful for a bounded rendering worker design, not an
invitation to create one thread per page.

Native integration would render directly to app-owned pixel buffers/textures,
retain display lists for the visible vicinity and expose structured text, links
and outlines to an app-owned viewer. A MuPDF WASM worker inside the existing
webview is also feasible as a lower-hosting-change experiment, but WASM transfers,
DOM work and text-layer construction must be measured separately from native.

Display-list reuse is clearly useful within a revision. It is not proof that a
list can be reused after a new PDF changes its referenced fonts or images. New
compiler bytes still need opening; cross-revision reuse is our responsibility.
MuPDF is offered under AGPL or commercial licensing, a material adoption decision.

Sources: [display lists](https://mupdf.readthedocs.io/en/latest/reference/javascript/types/DisplayList.html),
[threading](https://mupdf.readthedocs.io/en/latest/reference/c/overview.html#multi-threading),
[licensing](https://mupdf.readthedocs.io/en/latest/license.html).

### PDFium: strongest alternative for a cross-platform native prototype

Offers native page rendering, text extraction and document navigation primitives.
Its progressive rendering API provides pause/continue/close operations, valuable
for yielding to a newer compile rather than completing obsolete expensive pages.
Progressive rendering is not incremental replacement of changed PDF bytes.

Use one long-lived owner worker initially, with visible-page priority and bounded
work. PDFium is not generally thread-safe: the Rust wrapper's mutex serializes
calls, so adding threads around it does not provide parallel PDFium execution.
If measured workloads justify parallelism, investigate isolated persistent worker
processes; account for duplicated documents, IPC and memory before adopting them.

Bundle and pin the native library for every supported platform/architecture,
including signing/notarization and license notices. The Rust wrapper does not
ship the engine. Do not introduce a system-install/PATH dependency. Keep PDF
JavaScript disabled/omitted and own links, file opening and export in the app.

Sources: [progressive rendering API](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_progressive.h),
[Rust integration, distribution and threading](https://github.com/ajrcarey/pdfium-render),
[license](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/LICENSE).

### Apple PDFKit / Core Graphics: useful macOS-specific competitor

PDFKit supplies an actual viewer, selection, search, thumbnails and outlines;
Core Graphics can draw PDF pages into our own contexts. A PDFKit integration may
need less viewer rebuilding than a bitmap engine. A custom Core Graphics surface
offers more explicit update control but gives up that ready-made viewer.

This is Apple-specific and OS-version-dependent. Assigning a new PDFDocument is
not documented as a no-flicker hot-reload transaction. Prototype position retention,
readiness and visual handoff before treating it as a solution. Maintaining a
separate Windows/Linux viewer is an ongoing cost. No PDFKit timing was measured.

Sources: [PDFKit](https://developer.apple.com/documentation/pdfkit),
[Core Graphics page drawing](https://developer.apple.com/documentation/coregraphics/cgcontext/drawpdfpage(_:)).

### Hayro: useful Rust-only comparator, not yet the speed-first recommendation

Already bundled for our PDF utilities. Our current `src/pdf.rs` creates a parsed
PDF and RenderCache per render request, so a retained-document experiment is more
representative than timing that utility unchanged. Its interpreter/device
separation also leaves room for another rendering backend.

However, upstream explicitly calls out remaining fidelity gaps and says
performance has not yet been a focus. It does not provide the full interactive
viewer we need. A GPU backend would still require parsing, interpretation and
text/navigation integration; GPU acceleration is not inherently hot reload.

Source: [Hayro architecture and limitations](https://github.com/LaurenzV/hayro).

### Other directions

- **Poppler as a bundled library:** supports region/page rendering, but adds
  another native engine without demonstrated reload advantage here. Lower
  priority than the two measured candidates. Do not revert to per-page CLI
  processes or user-installed utilities. [API](https://poppler.freedesktop.org/api/cpp/classpoppler_1_1page__renderer.html).
- **Typst's incremental vector representation:** compiler-aware updates can avoid
  the PDF serialization/reparse cycle. We already use the Tinymist family of
  tooling. Revisit our explicit `--partial-rendering=false` only with the existing
  zoom, clipping and cache invariants covered. This does not handle Tectonic PDFs
  or arbitrary imported PDFs. [Incremental rendering project](https://github.com/Myriad-Dreamin/typst.ts).
- **PDF-to-SVG conversion:** printpdf's known fidelity gaps remain blockers;
  converting whole documents also adds work. SVG is a presentation format, not a
  guarantee of incremental updates. [Earlier investigation](printpdf-migration-plan.md).

## Current PDF.js overhead worth isolating

The stable host survives, but `src/pdfjs/host.js` creates a fresh iframe containing
the full generic viewer per accepted revision. `src/pdfjs/frame.js` waits for
`pagesPromise` before restoring the position. In the bundled viewer, the normal
eager path resolves that promise after fetching every page proxy; large-document
and disabled-auto-fetch paths differ. Restored search also waits for a non-pending
find state before continuing. These are observable gates, not measured attribution
of the total latency.

A lighter PDF.js viewer should keep its shell/controls alive, open a pending
document, fetch current visible pages first and restore navigation without waiting
for offscreen pages or full search work. Geometry must be correct before handoff;
blindly deleting the existing waits risks scroll jumps. Reusing a worker may save
startup, but does not establish reuse of decoded resources across PDF documents.

Use this as the control prototype. It separates unnecessary viewer startup from
engine cost, retains a mature decoder and may remove enough overhead to make a
native migration unnecessary. Public page APIs support per-page rendering/text:
[PDFPageProxy](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib-PDFPageProxy.html).

## Architecture for fastest correct updates

1. Accept immutable compiler artifacts directly in memory. No staging file,
   command launch or PNG encoding/decoding in the preview critical path.
2. Keep viewer controls, scroll containers and workers alive. At most one active
   revision plus one latest pending revision; cancel/drop obsolete results.
3. Parse lazily where supported. Render the current visible region before
   offscreen pages, thumbnails, global search counts or document-wide analysis.
4. Stage final-resolution pixels and matching visible text/link geometry while
   the old revision remains interactive. Upload/swap on a frame boundary. Never
   expose old text or links over new graphics, or a partially painted new page.
5. Preserve a page-relative viewport anchor and re-sample navigation immediately
   before commit. Page insertion/deletion requires an explicit anchor policy.
6. Cache by revision initially. Only add cross-revision page/tile reuse when
   dependency-aware signatures include resources, inherited geometry and render
   parameters. Identical page object numbers/content streams are not sufficient.
7. Bound worker queues, raster/SVG/cache memory and obsolete-document lifetimes.
   Retain success on failure and make a stale preview identifiable to the user.

Opening a newly emitted PDF is not the same as applying the PDF format's
incremental-save append mechanism. Our compilers supply new canonical artifacts;
none of the reviewed APIs establishes automatic safe application of those changes
to a live viewer. Optimize reopening and visible work first; do not build a PDF
object diff engine until measurements show it is necessary.

## Next experiment and decision gates

Build the same persistent-surface fixture for lightweight PDF.js, native PDFium
and native MuPDF (subject to the licensing choice). Start with final-resolution
visible output and text/links, not a complete replacement settings/UI project.

Measure artifact-arrival → parse → visible pixels ready → interactive layer ready
→ presented frame. Use identical changed PDFs, zoom, viewport and physical pixel
count. Include TeX equations, complex fonts, TikZ, images/transparency and page
reflow; test near the beginning and far into long documents. Record cold and warm
p50/p95, memory peak/settled state, frame continuity, cancellation and idle work.

A useful proposed target is warm p95 under 50 ms for the agreed ordinary editing
fixtures, with zero blank/stale commits and correct interactive layers. This is
a prototype target, not a claim already achieved or a guarantee for arbitrary
PDFs. Test 100 rapid alternating updates and document-size changes; require
fidelity and bounded memory before selecting on speed. Capture and inspect fresh
visual evidence and use geometry traces for any native child-view prototype.

Choose the fastest complete implementation that passes those gates. If a native
engine only wins the microbenchmark but loses after presentation/text integration,
do not migrate. No production switch or new runtime dependency is made by this
investigation, and the full application test suite was not rerun for research
artifacts alone.
