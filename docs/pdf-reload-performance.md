# PDFium / PDF.js reload comparison

This benchmark uses the actual bundled PDFium worker and PDF.js 6.3.289 inside
native macOS WKWebView. It does not use headless Chromium as a substitute for
the shipping macOS webview.

## Results — 23 September 2026

Apple M2 Max, 32 GiB RAM, macOS 14.6.1; release Rust build, optimized Swift
harness, native system WKWebView. PDFium chromium/7881; PDF.js 6.3.289.
Numbers below pool the two runs (60 measured samples per cell).

| Workload | PDFium median / p95 | PDF.js median / p95 |
|---|---:|---:|
| Changed PDF, 24 pages: page preparation | 9.14 / 9.81 ms | 30.0 / 35.0 ms |
| Changed PDF, 240 pages: page preparation | 16.13 / 16.73 ms | 49.5 / 59.0 ms |
| Same document, 24 pages: rerender page | 7.36 / 7.99 ms | 17.0 / 19.0 ms |
| Same document, 240 pages: rerender page | 7.42 / 7.90 ms | 17.0 / 19.0 ms |

PDFium was **3.28× / 3.07× faster** at changed-document page preparation for
24 / 240 pages. Its unchanged-document rerender was approximately **2.3× faster**.
The PDFium fresh-worker control measured approximately 9.2 / 16.2 ms median;
retaining its parsed document saves the repeated parse/catalog work.

The shipping PDF.js staged-viewer path has additional costs:

| Full staged PDF.js viewer | 24 pages median / p95 | 240 pages median / p95 |
|---|---:|---:|
| Refresh to frame activation | 174 / 190 ms | 208 / 225 ms |
| Refresh through two animation-frame callbacks | 201 / 217 ms | 234 / 251 ms |

All 120 measured staged updates contained the expected changed text, retained
the selected middle page, and had zero sampled blank frames. The full viewer
rendered only that page in the visible region. Its canvas rounding produced
1800 × 2358 pixels, versus the matched engine comparison's 1802 × 2360; this
small difference is another reason to keep the staged and engine tables separate.

These results support continuing the PDFium migration for faster hot reloads.
They also show that the current PDF.js integration costs much more than its
renderer alone: constructing/restoring the full staged viewer accounts for much
of the practical delay. The difference between the two tables is indicative,
not a separately instrumented attribution to any single viewer phase.

PDFium's 16.7 ms p95 on the longer document is worker preparation only; it is
**not** a claim of a 60 Hz end-to-end hot reload. GPU upload and egui scheduling
still need a native presentation measurement. No 19× on-screen speedup is claimed
by comparing 174 ms of viewer work against 9 ms of worker work.

Initial, non-warmed observations (separate boundaries): WKWebView navigation to
initial active viewer took 549–564 ms for 24 pages and 588–591 ms for 240 pages.
PDFium's first worker request, including library initialization, took 21.6–22.6 ms
and 27.7–28.4 ms. The WebKit readiness probe polls every 50 ms; these are not
precise or directly equivalent startup comparisons. Filesystem caches were warm.

Limits: one machine and a text/vector Typst-generated fixture at two lengths;
this does not establish behavior for image-heavy scans, complex transparency,
CJK fonts, TeX-specific PDF structures, or other operating systems. WebKit's
JavaScript clock has roughly millisecond granularity here. Search was inactive.
No memory, energy, sustained-scroll FPS or compile latency claim is made.

[Raw samples, fixture hashes and run metadata](performance-results/pdf-reload-comparison-2026-09-23.json).

## Method

Both engines receive preloaded A/B PDFs generated from `scripts/fixtures/pdfjs.typ`.
B changes “Scroll freely” to “Updated freely”. Test 24 and 240 pages, rendering
the middle page at 4.29 physical pixels per point (1802 × 2360 RGBA pixels).
PDF generation and file reading are outside the timed region. Each mode has five
warmup iterations followed by 30 samples; repeat the entire run twice. Engine
runs and GUI runs are sequential, after compilation/checks finish.

The PDFium measurement ends when its worker publishes page pixels, character
geometry and links. A changed document also rebuilds page sizes and the outline.
The PDF.js engine measurement uses a persistent PDFWorker, materializes page
metadata and the outline, renders the same page, extracts text and annotations,
and reads back RGBA pixels. The benchmark consumes `streamTextContent()`
with a reader, like the viewer text layer: this system WebKit lacks the
ReadableStream async iterator required by PDF.js 6's `getTextContent()` helper. PDF.js text items describe runs; PDFium extracts
per-character bounds. The outputs are comparable preparation work, not identical
APIs or a pure raster-only benchmark. The PDF.js retained-document case can reuse
its operator list; that is intentional cache-hit behavior.

Separately, the full PDF.js host runs its existing staging/restore/activation
path at 900 × 700 CSS pixels, light pages, 2× backing scale. It substitutes
preloaded PDF/state responses to remove PDF HTTP transport, but loads the actual
bundled viewer/worker assets from the Rust server. This gives viewer preparation
latency with warm assets, not file-save-to-preview latency. Every iteration checks
that the middle page contains the changed text and that navigation is retained.
A requestAnimationFrame sampler checks whether an active rendered page disappears.

The host result records both frame activation and two subsequent animation frame
callbacks. These callbacks provide presentation opportunities, not a measurement
of actual display scanout. PDFium worker timing excludes GPU upload, egui frame
scheduling and presentation; do not divide the two values and call the ratio an
end-to-end UI speedup. Initial WKWebView readiness is reported separately, with
50 ms polling granularity. Native process first-request latency is also separate;
neither is an OS-cold-cache startup benchmark.

## Reproduction at the rollback checkpoint

The comparison code and PDF.js assets were removed after this measurement.
Check out commit `00abc5f` in a separate worktree to reproduce the commands below.
The current tree contains only the PDFium viewer.

```sh
cargo test --release --bin tiptoptyp --no-run
swiftc -O -module-cache-path /private/tmp/pdf-bench-swift-cache \
  scripts/benchmark-pdfjs-native.swift -o /private/tmp/pdf-bench-webkit
python3 scripts/benchmark-pdf-reload.py \
  target/release/deps/tiptoptyp-TEST_BINARY_HASH \
  /private/tmp/pdf-bench-webkit .tiptoptyp/profiles/pdf-reload-comparison
```

Use the test executable printed by Cargo. Requires a macOS desktop session and
the bundled Typst/PDFium libraries. The harness writes fixture PDFs, their hashes,
raw samples, engine/browser versions and optimized binary hashes. No performance
threshold is imposed on normal tests, and no production profiling loop is added.

Validation: formatting, all-target Clippy, the full regression suite with four
test threads, and xtask tests passed. The harness completed both native repeats
for both document lengths. No production renderer behavior changed in this
comparison; only the existing ignored PDFium probe was parameterized.
