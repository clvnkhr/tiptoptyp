# Buffered Tinymist preview

**Tinymist partial (experimental)** is the default Typst preview. Settings →
Preview backend also offers the original **Tinymist (Typst only)** complete
renderer and PDFium. Stored backend choices remain unchanged; new profiles and
settings without a preview choice use the buffered viewer.
Both use the same pinned Tinymist 0.15.2 service and source navigation. TeX and
opened PDFs still use PDFium. This preference changes the live viewer, not PDF
compilation or export. Switching modes restarts the existing service through the
normal backend-change path; no second preview worker is created.

The partial SVG adapter retains every paper rectangle and its position, but only
populates ink for the visible viewport and one viewport above and below, rounded
outwards to whole pages. The window always spans the complete paper width. A
quarter-viewport guard band refills before ordinary scrolling exhausts the buffer.
A distant scroll, page/link jump, or restored history position synchronously
refills its destination before paint. Ordinary buffered demand is coalesced into
one animation frame; idle viewers schedule no render work.

Source patches retain Tinymist's FIFO merge/reset semantics and are applied before
viewport refill. Source and pagination changes invalidate cached coverage. Geometry-only resize
and zoom changes can reuse acknowledged ink when their buffered page range stays
the same; coverage is recalculated for the new scale. Coverage
is acknowledged only after a successful SVG patch; an error cannot mark empty
pages as ready. Current module page metadata supplies initial bounds before any
SVG exists. Actual decorated page offsets are cached after patching; binary
search finds the visible range without scanning every page during scrolling.
Tinymist's offscreen Canvas fallback is disabled only in this experimental mode:
otherwise it allocates and eventually rasterizes every placeholder page, defeating
partial rendering. The original full renderer keeps its existing pipeline.

This bounds populated page ink and viewport work, not the compiler's complete
module, shared glyph definitions, or lightweight page geometry. Very dense
individual pages and the first patch after a distant jump can still take time.
The adapter targets the pinned frontend's private interface; upgrading Tinymist
requires rerunning the real-frontend and native probes.

## Reproducible checks

```sh
node --test scripts/test-preview-navigation.mjs
node scripts/test-preview-e2e.cjs --large --self-test
node scripts/test-preview-e2e.cjs --large --mode both --palette-checks --geometry-checks
python3 scripts/test-preview-partial-native.py
python3 scripts/test-desktop-ui.py --journey partial_preview --release
```

The browser runner requires an existing Playwright installation and browser;
`TIPTOPTYP_PLAYWRIGHT`, `TIPTOPTYP_TEST_CHROME`, and `TIPTOPTYP_TEST_TINYMIST` can
select them. It never downloads tools. The native comparison needs macOS, Swift,
WebKit, and an unlocked desktop. The composed-app journey additionally requires
Accessibility and input-posting permissions. A missing capability is an
unavailable check, never a pass.

The checked-in 48-page fixture has dense equations, page-specific colored ink,
and distant internal links. Probes verify actual renderer mode, visible SVG ink,
first-frame refill, source updates during continuous scrolling, navigation,
resize/zoom, bounded populated DOM, no fallback canvases, and no idle renderer
work. Fresh browser/WebKit PNGs are written below `.tiptoptyp/screenshots`.
Neither standalone viewport snapshot proves composed editor geometry.

Performance probes use the same pinned optimized renderer, fixture, 900×700
viewport, three-second warmup, one workload warmup, and five measured samples.
Every workload warmup and measured sample first resets fit scale, page 18 and
a 15% page offset, then waits for stable frames outside measured counters.
The probes assert and record those matching start states. They measure active synchronous frontend spans and animation-frame gaps, not CPU,
GPU, cold startup, or cross-platform frame-rate guarantees. Each receipt records
input hashes, executable/version, scale, workload, and lane order. Preserve an
original adapter and pass `--baseline-adapter` to the native probe when comparing
old/new code. Run measurements without concurrent builds or other profiling.

## Local measurements (9 October 2026)

Same 48-page fixture and five measured samples per lane on macOS 14.6.1, arm64:

| Active workload | Full mode | Buffered partial mode |
| --- | ---: | ---: |
| Initial Chromium SVG nodes | 106,937 | 6,749 |
| Chromium median per-sample p95 RAF gap | 216.6 ms | 33.4 ms |
| WKWebView median per-sample p95 RAF gap (scale 2) | 435 ms | 149 ms |

These are separate engine comparisons; absolute timings must not be compared
across engines. Both exercised first-frame distant refill, actual page ink,
bounded DOM and idle-work checks. The original full-mode frontend acceptance
checks were also rerun. Fresh initial/source-change browser PNGs and a native
far-page PNG were inspected. Standalone snapshots do not establish composed-app
appearance. Full mode was the default during this comparison, and the native
149 ms result shows that
the experimental mode still has room to improve.

The [measurement receipt](performance-results/tinymist-partial-2026-10-09.json)
records input hashes, pinned executable, optimized native probe, warmup, workload,
all measured sample summaries and limits. Raw receipts remain under
`.tiptoptyp/profiles/partial-render-2026-10-09` and
`.tiptoptyp/profiles/partial-render-native/20261009T144306Z-e1g2m4nm`.

The actual app acceptance journey also passed with native clicks, keys, wheel
scrolling and observed renderer modes; its debug-build receipt is recorded in
[the UI audit](desktop-ui-audit.md#buffered-tinymist-mode-acceptance-9-october-2026).
This interaction result is separate from the optimized standalone timings.

## Follow-up optimizations (9 October 2026)

Partial mode shares one viewport/layout snapshot across coverage, controls and
render requests. Buffered scrolling no longer repeatedly reads the same DOM
rectangles: the deterministic 100-scroll test drops from 700 reads to at most
200. Resize and zoom retain existing page ink when the acknowledged source,
SVG associations and requested page range still match, updating transforms,
paper geometry and the fixed CSS page gaps instead of calling the WASM renderer.
Source work, renderer requests, missing associations and new page ranges take
the normal patch path. Pointer zoom restores the same page-local source point
on both paths, including distant pages and gaps; repagination never restores an
old source point over a new source patch.

Two optimizations also benefit full Tinymist mode. An exact white-paper/black-ink
palette bypasses its mathematically identical SVG color filter, avoiding
unnecessary filter surfaces. Dark, tinted and inverted palettes retain the
existing transfer and clipping. Each retained pseudo-link receives its hover
handlers once, instead of accumulating another pair on every SVG initializer
call. New or replaced nodes are still initialized. Full-mode rendering bounds,
source queue, Canvas fallback and scheduling keep their existing behavior.

Matched five-sample follow-up comparisons used the same 48-page fixture,
900×700 viewport and warmed resize/scroll/zoom workload within each engine:

| Comparison | Before median per-sample p95 frame gap | After |
| --- | ---: | ---: |
| Chromium partial mode, final shared adapter | 33.4 ms | 16.8 ms |
| WKWebView partial mode, before shared full-mode ports | 114 ms | 78 ms |
| Chromium full mode, shared optimizations only | 199.9 ms | 183.3 ms |

The native pair used the same optimized Swift probe at backing scale 2 with low
power mode off. Its adapter hashes were `16ddbf…` and `c7ba6f…`; final shared
adapter `06c0db…` has separate browser acceptance and browser measurements.
This is smoother frame delivery, not a claim of less total work: native median
workload wall time rose from 2518 to 2800 ms and synchronous rescale spans rose
from 288 to 692 ms. These inclusive spans are not CPU time and must not be
summed. Full-mode sample ranges overlap; its roughly 8% p95 reduction is a
modest local result. Timings use white paper and black ink; dark/tinted speed
was not measured. Do not compare unlike engines or the original table above.
The [follow-up receipt](performance-results/tinymist-partial-followup-2026-10-09.json)
retains metadata, hashes, all sample summaries, phase medians and idle checks.

Final full and partial browser acceptance passed internal links/history,
source replacement while scrolling, first-frame ink, resize/zoom and all five
palette transitions. Fresh dark and restored-white PNGs were inspected in
both modes. Partial pointer checks additionally forced the ordinary patch
fallback on page 42; first/second-frame anchor errors remained below 0.51 CSS
pixels with visible ink present. These snapshots are standalone browser
viewports, not composed desktop evidence. A subsequent native app journey
passed with the promoted partial default, including actual default renderer
observation, navigation and full/partial switching; its receipt and the earlier
failed/unavailable attempts are retained in the audit.

The browser probe now waits explicitly for asynchronous readiness and two
stable rendered RAF observations: this Playwright version treats an async
predicate as a truthy Promise rather than polling its resolved boolean. Older
probe timings are retained separately and excluded from the selected browser
pairs. Those selected pairs use probe `94320b…` and assert matching final start
states. A subsequent test-only review tightened the two-frame check to require
both observations to contain fitted page-18 ink at the same viewport. Final
acceptance uses that stricter probe; the recorded timings retain their original
probe provenance and do not establish this stronger first-observation contract. Native phase diagnostics are opt-in (`--phase-diagnostics`), use one
bounded RAF buffer, and cancel before idle checks. Idle admission requires zero
rescale, render and viewport work. An optimized native probe may be reused with
`--native-binary`, only when its source and binary hashes match and the original optimized build
and Swift version are recorded;
timeout and interruption terminate its entire build process group.


## Fractional paper sizes and zoom (9 October 2026)

The compiled page metadata has fractional point sizes (for example ISO A4 is
about 595.276 × 841.89 points), while the pinned SVG exporter rounds each paper
width and height up. Partial-mode layout now uses those rounded dimensions for
scale, paper offsets, cached geometry and pointer anchoring. Requests to the
WASM renderer retain the original unrounded document coordinates. Previously
this mismatch disabled covered-ink reuse for ordinary ISO paper sizes, forcing
SVG patches during every zoom step. The integer-sized original fixture did not
exercise that path.

The browser probe can now run `--fractional-pages --zoom-checks`; this transforms
the existing 48-page fixture to A4 and checks sixteen accumulated zoom steps
against the physical SVG scale. A small covered zoom must retain the existing
ink without a WASM patch. Native `partial_preview` also uses fractional A4 pages
and verifies repeated zoom buttons and actual keyboard events against fresh
visible-paper geometry, rather than accepting any percentage above 100%.

These checks exercise SVG zoom controls and keys. Programmatic WKWebView
magnification is not proof that a physical trackpad pinch gesture works. The
reported tiny-only symptom was not reproduced with buttons/keyboard on the
installed build; the fractional-page cache failure was reproduced independently.


The [fractional-page measurement receipt](performance-results/tinymist-fractional-zoom-2026-10-09.json)
records the same warmed Chromium workload before and after: median partial
renders drop from 59 to 16, while the median p95 RAF gap remains about 16.8 ms.
This demonstrates reduced work, not smoother native pinch input.


## Sustained zoom work and heavier coverage (9 October 2026)

The user confirmed that the fractional-paper fix also resolves the reported
trackpad pinch problem. The harness still distinguishes synthetic Ctrl-wheel
inputs from physical trackpad gestures.

Covered partial zoom now publishes paper placement, scale and pointer anchoring
in one physical SVG layout instead of scaling the previous gaps first. Cursor
ellipses are discovered after patches and reused during zoom; detached cursors
and changed source/cursor state still force a normal patch. When zoom expands
beyond buffered ink, the old SVG is anchored before selecting the first render
window, avoiding a fetch for the unscaled scroll position and a corrective fetch.
Full-mode page tracking now binary-searches paper bounds and checks overlapping
pages, rather than reading every paper's screen transform on each zoom/scroll.

The opt-in [stress fixture](../scripts/fixtures/tinymist-zoom-stress.typ) retains
48 pages but uses fourteen dense 4×4 matrices and nested integral/Gamma estimates
per page, with unequal fractional paper widths. It is separate from the smaller
fast fixture. The compiler-generated test PDF has 48 pages and is about 5 MB;
this is a complexity check, not an application package-size measurement.
The stress probes retain the same page/ink/first-frame/idle contracts. Their
explicit 320,000-node bound allows dense buffered ink but rejects populating the
entire document; the original fixture keeps its 30,000-node bound.

```sh
node scripts/test-preview-e2e.cjs --large --mode partial --stress --zoom-checks --geometry-checks
python3 scripts/test-preview-partial-native.py --mode partial --stress --phase-diagnostics
python3 scripts/test-desktop-ui.py --journey partial_zoom_stress --release
```

The stress journey is opt-in rather than added to the default `all` matrix. It
uses actual native input and effective-backend observations, including repeated
partial zoom on the dense fixture, complete-viewer zoom/last-page tracking on
the standard 48-page fixture, return to the dense partial entry, and restoration
of the original isolated session. Performance results are recorded separately below.


Matched stress runs used Tinymist 0.15.2, a 900×700 view, three-second warmup,
identical page-18/15% starting positions, and preserved optimized native probe
and adapter hashes. Three WebKit samples at 2× backing scale produced:

| Stress workload, median | Before | After |
| --- | ---: | ---: |
| Ten-step native zoom phase, inclusive wall time | 5,062 ms | 2,905 ms |
| Rescale calls in that zoom phase | 22 | 12 |
| SVG patches in that zoom phase | 1 | 1 |
| Whole native resize/scroll/zoom workload | 9,173 ms | 7,119 ms |
| Whole native workload p95 RAF gap | 508 ms | 288 ms |

The zoom sequence took about 43% less wall time. Its worst-step p95 remains
roughly 0.5 seconds; expensive refills still stall. Five matched Chromium samples
had essentially unchanged combined p95 gaps (about 217 ms), with ten fewer
rescale calls. These results demonstrate reduced WebKit interaction work, not
uniformly instantaneous gestures, CPU-time savings, or a cross-platform promise.
The [stress measurement receipt](performance-results/tinymist-zoom-stress-2026-10-09.json)
retains every sample, probe/fixture/binary hashes, phase policy and limitations.
Fresh initial/update browser and initial/far-page WebKit captures were inspected;
these are standalone engine views, separate from native app interaction evidence.


The dense fixture also exposes a full-viewer capacity limit. Both the preserved
pre-optimization adapter and the current full adapter exceeded the standalone
WebKit probe's 125-second budget. A bounded native CPU sample found SVG
compositing/geometry work during scrolling, with the sampled process at roughly
4 GB resident memory. This is recorded as a failed dense-full check, not an
optimization pass. Full-mode native smoke checks therefore use the standard
48-page fixture; the stress fixture remains mandatory for the partial lane.

The user's proposed cancellation policy needs two distinctions. New zoom events
already replace queued viewport work before each frame. The pinned SVG/WASM
patcher itself runs synchronously on WebKit's main thread, so an event cannot
interrupt it merely by setting a cancellation flag. True mid-patch cancellation
would require cooperative chunks or worker-side generation. Any future scheduler
should keep existing ink visible, scale it immediately, preserve ordered source
updates, cancel obsolete viewport generation at safe checkpoints, and publish
only the latest complete refill. Restarting every render throughout a gesture
would risk starvation, and does not remove WebKit's layout/compositing cost.
