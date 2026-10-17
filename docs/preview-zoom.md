# Embedded preview resizing and zoom

The full-document Tinymist SVG frontend previously routed resize events through
`addViewportChange`, its document-render queue, and a render-duration timeout.
Ctrl-wheel also waited for 20 accumulated pixels and selected a discrete scale.

`src/preview_navigation.js` is installed before the frontend loads. For the pinned
document-mode, complete SVG renderer it coalesces resize into one animation-frame
callback, refreshes the renderer's DOM measurements, and calls its existing
`r.rescale`. This retains the frontend's resize scroll anchor while avoiding
`rerender` and WASM document work. Compile updates and ordinary scrolling still
use Tinymist's existing pipeline. No polling, persistent animation, or extra
document/texture cache is introduced.

Ctrl-wheel accumulates pixel-normalized deltas and applies exponential scaling
once per frame, preserving the content point beneath the mouse. Keyboard +/-
scales about the viewport center, and Command/Ctrl-0 restores SVG fit width.
Limits are 0.1–10 times fit width. Text inputs keep their own key handling.
Wry's keyboard zoom is disabled so the SVG adapter owns these keys; WKWebView's
native pinch magnification remains enabled. Native magnification is a separate
WebKit scale: SVG fit-width reset does not reset an existing native pinch scale.

The adapter uses the pinned frontend's `typst-container.documents[0].impl`
interface. A sidecar upgrade must rerun the real-frontend probe. The original
adapter described below excluded partial rendering. Current buffered Tinymist
has its own viewport/refill path; see [partial rendering](preview-partial-rendering.md).
Slides, uninitialized/replaced documents, and non-SVG modes retain frontend handling.

## Evidence (21 September 2026)

The same bundled optimized Tinymist executable served `test.typ` in both runs.
Headless Chrome 153.0.8010.48 on macOS arm64, device scale 2, viewport
900→650→900 by 700 pixels, 60 width steps, 16 ms requested delay per step,
1.5 s warmup, 600 ms settling. The only comparison variable was the adapter.
This measures active frontend calls, not startup, GPU usage, or native frame rate.

| Active resize measurement | Original | Adapter |
| --- | ---: | ---: |
| Document rerenders | 58 | 0 |
| SVG rescales | 147 | 58 |
| Queued viewport changes | 58 | 0 |
| Scale after five -1 px Ctrl-wheel deltas | 1 | 1.010050167 |

A separate 900→700 resize after scrolling preserved the document-space top
anchor within 0.29 document units in both cases (scroll coordinates are rounded).
Counts are more useful than the probe's synchronous `renderMs`: the latter does
not include asynchronous rendering and must not be presented as total render time.

Local artifacts: `.tiptoptyp/profiles/preview-zoom-20260921/` contains the probe,
results, fixture/sidecar/adapter SHA-256 hashes, revision, and dirty-tree metadata.
The probe uses Playwright from the temporary QA installation, with the installed
Chrome binary; no browser package is added to production dependencies.

Run deterministic scheduling/anchoring tests with:

```sh
node --test scripts/test-preview-navigation.mjs
```

These also run in CI. Chromium verifies the real bundled frontend contract but
does not establish WKWebView pinch feel or composed native-window frame rate.
Those still require an interactive macOS assessment. Rust layout/native bounds
were not changed by this adapter.

A separate optimized Swift WKWebView probe was attempted with the same URL and
fixture. The page reached `readyState=complete` and created the document object,
but `moduleInitialized` stayed false and no SVG arrived before its 10-second
readiness deadline, including with the window activated. Consequently it provides
no valid native comparison. `native.swift` and `native-results.txt` retain that
failure in the artifact directory. The rebuilt app's main viewport screenshot
was inspected for startup/layout only; it does not verify native preview motion.

## Investigation of remaining jerkiness (21 September 2026)

After restarting the disposable Tinymist server, the native probe successfully
initialized both versions. The earlier readiness failure is not evidence of an
adapter failure: it happened before the document was initialized, with the adapter
absent as well. Its specific cause remains unconfirmed.

The successful probe used an optimized Swift executable, a real visible WKWebView,
the same document and resize sequence, native magnification enabled, and inactive
scheduling disabled (matching Wry's application configuration). It recorded 60
animation callbacks per run, with no simultaneous production build. This Mac was
drawing battery power and `pmset -g custom` reported battery `lowpowermode 1`.
AC Low Power Mode was disabled. No power settings were changed.

| Native measurement | Original frontend | Adapter |
| --- | ---: | ---: |
| Median animation frame interval | 33 ms | 33 ms |
| Maximum animation frame interval | 35 ms | 35 ms |
| Resize-triggered document rerenders | 31 | 0 |
| Maximum synchronous SVG rescale duration | 3 ms | 4 ms |

A preceding run measured only 0–1 ms between resize delivery and adapter rescale,
so the theory that an extra animation callback adds a frame of latency was not
supported. The dominant observed limitation is approximately 30 Hz delivery, not
long JavaScript stalls in this small fixture. WebKit's
[Page scheduling code](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/Page.cpp)
explicitly adds LowPowerMode to its throttling reasons and adjusts the rendering
frequency. Battery Low Power Mode is therefore the leading explanation for the
observed cadence, but an otherwise identical run with it disabled is still needed
to establish causality. We did not override the user's system power policy.

Evidence lives in `.tiptoptyp/profiles/preview-zoom-cadence-20260921/`, including
the probe source, raw intervals, power settings/source, OS version, and hashes.
This verifies native scheduling with the fixture, not the user's large document,
physical pinch gestures, GPU paint duration, or perceived smoothness. Keep those
distinctions when evaluating the remaining report. No production code was changed
on the basis of an unconfirmed timing hypothesis.

### Live document reproduced the actual problem

The user reported that the visible small-document probes looked smooth, unlike
the app. The newly rebuilt running application and its Tinymist executable were
identified, and the probe then connected to that actual preview endpoint. It
resized a child WKWebView's frame inside a stationary parent, including moving
its left edge, as a divider does. No document edits or settings changes were made.

The real document contained approximately 75,250 SVG DOM nodes versus the much
smaller initial fixture. With the existing adapter, native synchronous rescale
calls took approximately 189–196 ms and delivered only four rescale calls during
the resize sequence. Thus the earlier 30 Hz power observation was insufficient:
this document has an additional, much larger full-SVG layout cost.

A temporary transform-based experiment was worse on this document and was fully
removed. A separate browser experiment enabling Tinymist partial rendering reduced
the SVG DOM to approximately 9,372 nodes. Native partial-rendering tests reduced
individual rescale calls to roughly 20–25 ms, but the standard viewport update
pipeline still does multiple rescale/render passes. Enabling partial rendering
alone is not an established complete fix, and past scroll-refill failures still
need regression coverage. Production continues to use the existing adapter and
full rendering while this remains unresolved. Todo 264/265 are reopened.

The next implementation needs bounded visible-page DOM, efficient viewport update
scheduling, and verified refill after large scroll/zoom jumps. Acceptance must use
a document comparable to this live one, not merely a small welcome document.

The final visible-child comparison used equal three-second warmup periods. Full
rendering had a worst rescale of 190 ms and maximum observed animation-frame gap
of 531 ms; partial rendering had 23 ms and 163 ms respectively. These are one-run
diagnostics, not frame-rate guarantees. The partial branch intentionally leaves
Tinymist's default viewport scheduling active because the production adapter only
intercepts full SVG documents. Raw data and the probe are retained under
`.tiptoptyp/profiles/preview-live-document-20260921/`.

### Input routing and bitmap-transition investigation

The app previously consumed preview zoom shortcuts without dispatching them when
the interactive Tinymist viewer was active. These now dispatch a typed action
through a viewer event. The adapter accepts Option-modified zoom keys and composes
wheel and keyboard inputs in their arrival order within one animation frame.
Seven deterministic JavaScript tests cover input accumulation, clamping, reset,
pointer anchoring, bounded scheduling and geometry invalidation. This does not
establish physical trackpad-pinch correctness: native magnification is separate.

The adapter also supports immediate partial-render demand and avoids identical
rescale passes. A native live-document experiment still had roughly 80–90 ms
active animation gaps. Partial rendering remains disabled in production pending
scroll-refill and interaction verification; the smoothness todos remain open.

The user's bitmap-transition suggestion was tested at the capture boundary using
WKWebView's native viewport snapshot API (900 by 700 points, optimized Swift probe,
the same live document, three-second warmup). A snapshot of the partial-rendered
viewport took about 151 ms in one run. That is asynchronous capture latency, not
CPU time, and does not measure the performance of an implemented transition.
Do not synchronously wait for a snapshot on gesture start or encode a PNG per frame.

The next candidate is one in-memory viewport image captured after content settles,
scaled during a gesture, and replaced only after sharp content is ready. Bound
physical pixels and the number of retained/in-flight images; discard stale capture
results after document, scroll, theme or viewport changes. A 1800 by 1400 RGBA image
alone needs about 9.6 MiB, excluding GPU copies and capture overhead. Never cache a
whole-document image. Test anchor stability, zoom-out edges, source updates during
a gesture, memory release, and time to sharp replacement before shipping it.
This candidate is implemented by the viewport cache described below.

## Optional buffered renderer (9 October 2026)

The earlier investigations above describe the full-mode baseline and unresolved
experiments at their recorded dates. A separate opt-in buffered mode now addresses
scroll refill and offscreen work without replacing that default. See
[Buffered Tinymist preview](preview-partial-rendering.md) for its current contract
and checked-in browser/native probes.


## Cached viewport zoom (10 October 2026)

Settings → Preview → **Smooth Tinymist zoom** is enabled by default on macOS,
for both complete and partial Tinymist viewers. Its JSON key is
`preview_zoom_snapshot`. Other platforms retain the existing zoom path.

After the viewport settles and remains idle for 150 ms, the app copies the SVG
paper and ink intersecting a bounded window: half a viewport to each horizontal
side, and one full viewport above and below. Referenced glyphs, gradients and
palette definitions are copied transitively; the live SVG and scroll position
are untouched. A passive offscreen WKWebView rasterizes that inert SVG/CSS copy.
It has an ephemeral data store, no scripts or external resources, and cannot
activate the app or take keyboard focus. It is reused rather than recreated for
each capture, and removed on hiding/disabling. Once a capture completes, its
copied DOM is replaced with a blank document and the view shrinks to 1×1; the
admitted image keeps its own pixels. Navigation identity rejects late callbacks
from the parked document or an older capture. Oversized copies fall back to the
visible viewport. The native raster targets twice the display density, with a
shared cap below four million pixels; the larger buffer can reduce its density.
There is one capture per view and at most one prepared replacement; source,
palette, manual scroll, visibility and document changes reject old results. Zoom never starts a screenshot. No PNG
encoding, base64 transfer, browser image decoding or CPU resampling is involved
in the production cache.

Core Animation image layers sit above the native WebKit content, clipped
to the entire preview viewport. A separate layer above the image draws its focus/hover outline. Both Tinymist SVG modes use page slices sharing
one immutable CGImage; each visible slice submits only its viewport intersection
and matching texture rectangle. Offscreen
slices are hidden, and only pages inside the captured buffer create layers. An opaque sRGB backdrop covers the entire
clip even outside the cached image, so the older live PDF cannot show through
zoom-out edges. Gesture and focused zoom-key events update its transform on the
app's AppKit thread. Tinymist's synchronous WASM/SVG runs on the separate
WebContent thread, so input can keep moving the cached texture while that
renderer is blocked. AppKit objects must stay on the app's main thread;
moving view operations to an arbitrary Rust worker would violate that requirement.
Core Animation scales the image through its compositor.

Cached zoom clamps to the same scroll limits and horizontal centering as the
SVG, including below-fit zoom and the first/last document edges. Partial rendering
scales page ink while keeping the 10 CSS-pixel page gaps fixed. Document height
and the anchor projection use this piecewise layout; scaling the whole document
photograph would accumulate incorrect gaps near its bottom. Both SVG modes
receive a page-local or gap-fraction anchor, so their fixed-size page gaps do not
shift the content point when sharp SVG replaces the image. Full SVG rescale also
normalizes page placement immediately, using the same direct-root layout rules. Page metadata is
reused until the renderer replaces its offset array.

Divider resizing uses the same cached image and preserves the source point at
the viewport top and the current horizontal scroll, including zoom/pan since the
capture. Native bounds and the opaque clip update immediately; vector
resize waits for release or the quiet period. Repeated equal bounds do no work,
and a mixed zoom/resize burst keeps only the latest requested geometry.

With an admitted image, zoom input now updates only the native texture. Vector
zoom starts on a native pinch-end/cancel event, or after 120 ms without another
wheel, keyboard or button input. Pinches with explicit active phases keep vector
work deferred; a one-second inactivity watchdog handles a missing end event. One
rescheduled main-run-loop timer handles the deadline, including event tracking;
there is no worker per input or idle repaint loop. Losing the image flushes the
latest zoom immediately. Hiding/disabling cancels the pending zoom and timer.
Without a usable idle image the renderer responds immediately. Before an image
or active native request exists, browser resizing retains its normal frontend
owner; startup cannot wait for a missing native acknowledgement.

The renderer gets one absolute-scale request in flight and one latest replacement.
Intermediate input does not add a queue of browser scripts. Coalescing preserves
moving pointer anchors, including equal-scale gestures with a residual pan.
Revision, scale and capture-token acknowledgements prevent an older render from
hiding the newest overlay. The sharp SVG replaces it after the final requested
geometry is ready and gets a paint opportunity. Its last valid raster remains
hidden and available for another gesture while an idle replacement is prepared.
The replacement stays detached until the browser validates its source and the
native side acknowledges acceptance. Input arriving during that handshake
rejects the replacement without discarding the usable image. A cold cache takes
the same bounded renderer path without a bitmap; disabling the setting retains the
original browser/native-magnification behavior.

A raw four-million-pixel RGBA image is about 15.3 MiB. During idle refresh, the
admitted image and one prepared replacement can temporarily retain up to eight
million pixels (about 30.5 MiB), plus capture and GPU storage. This is a bound
on raster pixels, not a measurement of process memory. The pixel cap can limit
very large panes below display density. Before showing retained pixels, the app
checks that they cover every visible part of the paper. A large zoom-out that
exceeds the captured buffer falls back to the live renderer immediately.
This isolates visual feedback; it does not make Typst compilation asynchronous
or cancel an already executing synchronous WASM patch.

The first DOM-image implementation shared Tinymist's JavaScript thread. Its
[historical dense-file receipt](performance-results/tinymist-zoom-cache-2026-10-10.json)
recorded a shorter zoom sequence but still roughly 0.67-second browser frame gaps.
Those measurements do not establish responsiveness of the replacement native
compositor. Browser RAF timing also cannot measure a native overlay's updates.

The native isolation regression uses the production Rust overlay and an identical
optimized binary, viewport and three-second warmup for each lane. It deliberately
blocks WebContent for 800 ms, posts four owned AppKit zoom-key events during the
block, and checks native transforms, bounded vector requests, handoff, and negative
keyboard ownership. The separate real-app `partial_zoom_stress` journey verifies
the effective backend, native image use and sharp coverage in both Tinymist modes.

```sh
cargo test --release --test native_preview_zoom --features desktop-ui-tests --no-run
TIPTOPTYP_NATIVE_ZOOM_TEST=1 cargo test --release --test native_preview_zoom --features desktop-ui-tests
TIPTOPTYP_NATIVE_ZOOM_TEST=1 cargo test --release --test native_preview_zoom --features desktop-ui-tests -- --web-thread
python3 scripts/test-desktop-ui.py --journey partial_zoom_stress --release --trace-preview
```

The [native measurement receipt](performance-results/tinymist-zoom-native-2026-10-10.json)
records three matched runs per lane. During an 800 ms WebContent block, the
browser path responded in 517–699 ms; native layer updates were submitted in
0.11–0.44 ms. All native inputs moved the retained image during the block and
coalesced into two vector requests before the final sharp handoff. These are
AppKit submission timings, not GPU presentation or dense Typst final-render
latency. The real-app dense partial/standard full journey also passed, requiring
native raster hits and current sharp geometry in both modes.

The standalone Swift SVG probe now measures only SVG work. Its old duplicate
bitmap provider was removed; it cannot prove production native input isolation.
Full rendering's final completion callback still schedules idle capture; DOM
mutation alone is insufficient because the frontend clears busy in a later frame.
Home/End explicitly navigate both viewers. Keyboard appearance inversion retires
old pixels before repainting and captures the new palette when idle.

### Deferred gestures and buffered boundaries (10 October 2026)

The latest isolated native regression also exercises zoom-out and four child-view
width changes during an 800 ms WebContent stall. It requires an opaque backdrop,
correctly centered paper, native image updates on every resize, no intermediate
vector request, one final vector request, and sharp handoff. `--background`
prohibits test-app activation and targets only its own window; it does not post
global keyboard/mouse input. Native-layer PNGs verify the owned layer, not the
composed editor window.

```sh
TIPTOPTYP_NATIVE_ZOOM_TEST=1 cargo test --test native_preview_zoom --features desktop-ui-tests -- --background
```

The real-app stress journey now includes divider drags in both Tinymist modes.
At this stage, the final composed-editor run and paired foreground release
timing were unavailable because the user was using the computer and declined
an idle interval. Interrupted
focus-switch samples are excluded. The older successful journey above remains
historical evidence, not verification of these new changes. See the
[buffer/defer receipt](performance-results/tinymist-zoom-buffer-2026-10-10.json)
for current deterministic/native component checks and explicit limitations.


### Border geometry and bounded image work (10 October 2026)

A fresh real-backend reproduction on the checked-in 48-page dense fixture exposed
a 235-pixel height error when partial rendering zoomed to half scale at the last
page. Fixed page gaps now participate in the cached geometry and page-local anchor
projection. The regression replays 24 measured cases covering full/partial mode,
top/bottom edges, both corner anchors and three scales, allowing one pixel for
frontend rounding. A second regression covers resizing after a horizontal zoom/pan.

Viewport crop tests bound every submitted layer frame and texture rectangle even
at extreme scales. A 10,000-page document allocates slices for just the three pages
intersecting the test buffer. Pathological buffers containing over 256 pages use
the normal renderer. There is no per-input pixel copy. Explicit per-event
`CATransaction.flush()` was removed; transactions now use the normal run-loop
commit. This follows [Apple's transaction guidance](https://developer.apple.com/documentation/quartzcore/catransaction/flush%28%29).
The passive capture view is blank and 1×1 between captures, instead of retaining
the copied SVG and a large view frame while idle.

The checked-in real-backend probes run without taking desktop input:

```sh
TIPTOPTYP_PLAYWRIGHT=/path/to/playwright \
TIPTOPTYP_TEST_CHROME=/path/to/chrome \
TIPTOPTYP_TEST_TINYMIST=/path/to/pinned/tinymist \
node scripts/test-preview-zoom-edges.cjs

cargo test --release --test native_preview_zoom --features desktop-ui-tests --no-run
python3 scripts/test-preview-zoom-native.py \
  --binary /path/to/built/native_preview_zoom
```

Use the executable path printed by the optimized test build. The native wrapper
asserts the actual frontend mode and runs the production cache
over both full and partial Tinymist. Its own-window key events update the native
image during an 800 ms WebContent stall, then send one final vector request and
hand off. Fresh isolated layer captures were inspected for retained ink, clipping
and opacity. They do not prove the composed editor's placement or physical pinch
behavior. The foreground `partial_zoom_stress` journey remains unavailable while
the user is using the computer.

Three matched optimized submission runs before/after show no clear CPU speedup:
both are about 10–11 microseconds per cached input. This measures AppKit update
submission, not GPU presentation, memory savings or final Typst render latency.
The [edge/resource receipt](performance-results/tinymist-zoom-edges-2026-10-10.json)
contains raw runs, source/build hashes and the limits of that evidence.


### Continuous cache availability (11 October 2026)

A sharp handoff used to discard the usable raster before the 150 ms idle delay
and asynchronous recapture. Another gesture in that interval therefore had to
wait for WebContent. The last valid image now stays hidden but ready throughout
refresh. A replacement uses detached layers and its own geometry until JavaScript
validates the source and native admission is acknowledged. Input/source races
reject only the replacement; disabled/hidden/source-invalidated views still
release the cache. If the retained image cannot cover the visible paper, input
bypasses the image and dispatches normal vector work immediately.

The optimized own-window regression can deliberately delay a prepared replacement
and start another gesture before admission:

```sh
TIPTOPTYP_NATIVE_ZOOM_TEST=1 cargo test --release --test native_preview_zoom \
  --features desktop-ui-tests -- --background --refresh-race
python3 scripts/test-preview-zoom-native.py \
  --binary /path/to/built/native_preview_zoom --refresh-race
```

Both real Tinymist modes passed, retaining the previous geometry through the race
and recovering with a newly admitted image. The mock also verifies immediate
uncovered-paper fallback, opaque gutters, centering, four divider-width changes
and negative keyboard ownership. Fresh isolated native clip PNGs were inspected;
they do not establish composed-editor placement or physical pinch behavior.

Three interleaved optimized runs used the same 640×420 viewport, fixture,
three-second warmup and back-to-back gestures during 800 ms WebContent stalls.
The baseline had no native image for the second gesture and waited 827–830 ms
for sharp content. The retained image submitted an update in 42–64 µs; sharp
content still arrived in 824–832 ms. This demonstrates cache availability during
a blocked render, not GPU frame rate or faster final rendering. The
[cache-reuse receipt](performance-results/tinymist-zoom-reuse-2026-10-11.json)
retains raw runs, source/build hashes, resource bounds and unavailable checks.

### Idle buffer preparation (11 October 2026)

Buffered capture now serializes the selected papers directly, rather than making
a second dense SVG tree and walking every element/attribute through JavaScript.
An attribute tokenizer preserves local href and CSS url dependencies, decodes
entity-escaped identifiers, skips text/comments/CDATA, and queues each definition
once. It reads definitions afresh on every preparation, so source and palette
changes need no additional persistent cache or invalidation rules. The opaque
paper window, four-million-pixel limit and native rasterization remain unchanged.

The [matched native receipt](performance-results/tinymist-buffer-preparation-2026-10-11.json)
records nine preparations after one warmup per lane, using the same optimized
WKWebView probe, viewport, warmup and pinned Tinymist. Dense partial preparation
fell from 289 ms to 50 ms (83% less synchronous work). Standard full preparation
fell from 31 ms to 7 ms. Canonical buffered SVG is identical to the original
serializer at the same frame, including paper placement and glyph definitions.
These are idle preparation wall spans, excluding image rasterization, encoding,
final SVG render latency and GPU presentation.

```sh
node --test scripts/test-preview-snapshot-buffer.mjs
python3 scripts/test-preview-partial-native.py --mode partial --stress \
  --samples 9 --snapshot-buffer src/preview_snapshot_buffer.js
# Freeze the original serializer before editing for an optional untimed
# canonical-equivalence check using --baseline-buffer /path/to/original.js.
```

The probe's buffer workload is separate from active resize/scroll/zoom phases;
combining their timings would describe different work. Reuse only the same
optimized probe with its recorded source/binary hashes.


### Divider anchors and repeat gestures (12 October 2026)

The explicit snapshot position now owns native zoom and divider handoff.
Applying that position clears Tinymist's older resize anchor. Both SVG modes
supply page-local coordinates to the native cache, whose page slices preserve
fixed gaps. Full rendering also updates the direct paper/background/cursor
positions during rescale, so a later SVG patch cannot change the gap layout.
This does not call WASM or replace full rendering's source pipeline. Partial rendering also carries its native
page-local anchor through viewport repair and restores it after the final page
layout. A delayed renderer rescale therefore cannot reapply a previous pinch
anchor or intermediate SVG position over the cached image's target.

The image handoff also records scroll position from the final settled frame,
after its paint opportunity. A delayed automatic scroll event therefore keeps
the warmed image available instead of treating that anchor correction as manual
navigation. Repeated gestures reuse the retained pixels while idle capture and
replacement admission run; source, palette and actual manual scroll still
invalidate them.

If a zoom temporarily exposes paper outside the captured buffer, the proxy
hides and requests sharp work immediately, but its valid pixels remain available
for a return into the covered region. Zero-delta native wheel phase boundaries
do not discard that cache. Real wheel movement retains its invalidation path.
No extra capture begins on input or idle repaint loop is introduced.

The deterministic tests cover final-frame scroll correction, successive gestures
racing idle replacement, and renderer anchor restoration. The native
`--transition-regression` probe compares cached and sharp paper origin/width,
page-local anchors and captured-versus-sharp ink within 1.1 CSS pixels.
A root SVG edge alone is insufficient: it can stay aligned while page gaps move
the text inside it. Geometry-only capture races reject the pending replacement
while retaining the warmed image.
It starts successive gestures at sharp handoff without waiting for another idle
capture, then checks an uncovered excursion and immediate cache reuse on return.
Use the dense 48-page fixture for partial mode and the standard 48-page fixture
for full mode; these workloads are separate and should not be compared as if
they render the same content.

```sh
node --test scripts/test-preview-navigation.mjs scripts/test-preview-zoom-snapshot.mjs
cargo test --release --test native_preview_zoom --features desktop-ui-tests --no-run
python3 scripts/test-preview-zoom-native.py --binary /path/to/built/native_preview_zoom \
  --mode partial --fixture scripts/fixtures/tinymist-zoom-stress.typ --transition-regression
python3 scripts/test-preview-zoom-native.py --binary /path/to/built/native_preview_zoom \
  --mode full --fixture scripts/fixtures/tinymist-partial-large.typ --transition-regression
python3 scripts/test-desktop-ui.py --journey partial_zoom_stress --release --trace-preview
```

The [transition receipt](performance-results/tinymist-zoom-transitions-2026-10-12.json)
records baseline/after outcomes and their build, fixture and workload metadata.
The native probe uses owned window events and injected production zoom/bounds
requests; it does not establish physical trackpad feel or GPU presentation.
The real-app journey separately compares native and sharp scroll geometry after
actual divider drags, allowing 1.1 CSS pixels for frontend rounding.

The real editor journey passed on the shared page-layout build, with actual
partial/full divider anchors within 0.43 CSS pixels. The isolated native ink
checks passed three times per mode, within 0.85 CSS pixels. Dense partial idle
preparation took 48–70 ms; capture through admission took 651–798 ms, so the
previous usable image stays available throughout that delay. These spans do
not measure GPU presentation or faster final SVG rendering.

The final rebuild also fixes uncached full-mode wheel/keyboard anchors and
safely rejects source anchors referencing removed pages. The unlocked rerun
caught another race: AppKit had already handled a changed viewport, but its
later duplicate DOM resize notification canceled the pending paint/ready
callback. Resizing now preserves an active native flight's acknowledgement;
a notification matching an already acknowledged viewport starts no new work.
New native requests still invalidate older handoffs normally. Read-only probe
state includes the current request, frame readiness and pending handoff.

The current release editor journey passed three narrow/widen cycles per mode
(12 divider transitions), with a maximum anchor error of 0.43 CSS pixels. The
matched native probes also passed both modes, including a forced duplicate
resize between DOM mutation and paint, page-ink error below 0.85 CSS pixels,
and reuse of retained pixels through successive zooms and a temporary uncovered
view. Fresh native-layer PNGs were inspected; these do not establish whole-window
composition. All 106 JavaScript checks, 1,885 Rust tests, 56 Python contracts,
17 xtask tests, formatting and strict Clippy passed.

One earlier attempt failed to reopen the preview controls after scrolling.
The same build passed that interaction on retry; the unexplained single-click
failure is preserved separately in the receipt for follow-up. No automatic
click retry was added. Physical trackpad pinch remains a manual check.


### Scroll ordering and page boundaries (13 October 2026)

An idle wheel event is not proof that the document moved. At a scroll boundary,
retiring the native image immediately left the browser believing its old cache
was still usable, so the next zoom took the cold path. Actual browser scroll
movement now invalidates idle pixels; navigation during an active vector flight
still interrupts that flight's image feedback.

WebKit can report a resize-induced scroll before its resize event. That event
keeps the native image and source anchor, and asks its existing native owner to
finish the resize. Browser frame observations cannot update the native owner's
last handled dimensions while a cache exists. Captures with missing geometry,
rubber-band origins or a mismatched viewport are rejected rather than admitted
as freely transformed images without document limits.

The inert SVG copy retains its parent's gutter color and clips overflow. An
additional viewport-clipped solid layer preserves that background between page
slices, including their fixed 10 CSS pixel gaps. Raster slices exclude chrome
outside the SVG. The raster pixel budget and one-flight rendering queue are
unchanged; gesture input still performs no image decoding or CPU resizing.

`partial_cache_edges` is the focused composed-app journey. The native probe's
`--transition-regression --scroll-race --dark` path additionally forces
scroll-before-resize delivery, verifies actual browser top/bottom scroll limits,
checks cached-to-sharp positions and dark right-gutter pixels, and fails on
frontend exceptions. Its layer PNGs show isolated native overlay pixels, not the
composed editor. Use `scripts/fixtures/tinymist-zoom-boundaries.typ` for a quick
full-renderer lane; keep the same fixture within each before/after pair.

A two-pixel inset formerly exposed old SVG glyph fragments along the right edge
while the cached image moved. The opaque mask now covers every viewport pixel;
a separate native border preserves the current hover/focus stroke. That stroke
is sent only when it changes or the native cache is reconfigured. Neither zoom
nor resize queries browser styles or allocates another raster.

Trackpad magnification uses the window's current cursor position, rather than
the gesture event's potentially fixed center. Coordinates convert through the
actual WebView bounds and flipped coordinate system. Events outside that view
or without its owning window do not capture preview input. Keyboard/button zoom
continues to use the viewport center. The native regression compares opposite
off-center anchors in both renderers; physical trackpad feel remains a manual
check.

Full SVG rounds its CSS box before fitting its viewBox. The cache now uses that
same ink scale and rounded document extent, rather than scaling a previously
rounded capture height. This prevents a one-pixel last-page overshoot from being
amplified by the next gesture.

The headless edge reference now records page-aware full-mode geometry and the
rounded-box metadata. Its older full-mode entries still expected page gaps to
scale, although the committed viewer already kept them fixed. All 24 cases are
replayed by the native geometry unit test; the browser runner preserves its
observations before comparing them so a stale reference has useful failure
evidence.

### Page pixels and host coordinates (13 October 2026)

The composed editor recording showed cached pages in reverse order while the
sharp viewer restored First, Second, Third on release. The WebKit backing layer
already uses AppKit's flipped coordinates; flipping the standalone cache
container again reversed its page slices. The cache now retains ordinary child
layer geometry and the snapshot's top-origin texture crop.

The earlier detached single-marker test was misleading: its raw bitmap context
and the host window used different vertical coordinates, and one full-viewport
layer concealed reversed page order. The test framebuffer now applies the host's
top-down coordinate conversion to its CGContext without changing live layer
frames or texture crops.

The owned native `--pixel-anchors` regression captures three distinct colored
pages while idle, checks their order, then measures both visible markers after
a top-anchored divider resize and an off-center zoom on the second page. Expected
positions come from the source markers, input and fixed 10-pixel page gaps,
independently of native transform reports. Restoring only the duplicate container
flip fails by 184–221 CSS pixels during resize/zoom; the corrected page pixels
match within 0.4 CSS pixels. These offscreen pixels still require a separate
composed-window observation and do not establish physical trackpad behavior.

```sh
TIPTOPTYP_NATIVE_ZOOM_TEST=1 \
TIPTOPTYP_NATIVE_ZOOM_PNG="$PWD/.tiptoptyp/screenshots/agent-review/pixel-anchors.png" \
TIPTOPTYP_NATIVE_ZOOM_RECEIPT="$PWD/.tiptoptyp/profiles/pixel-anchors.json" \
/path/to/optimized/native_preview_zoom --background --pixel-anchors
```

The composed-app journey also preserves the original live-document source point
at the top of the viewport during divider drags. Positioned native Ctrl-wheel
input verifies the original source point remains under an off-center cursor and
that cached feedback was used. It does not synthesize physical trackpad pinch.


The idle buffer and native viewport mask use Tinymist's actual backdrop, including
beneath the final paper. Transparent ancestor colors are composed over the
interface palette in a bounded, idle-only walk. The native pixel regression also
fits all three papers inside the viewport and checks the area below them against
the live gray backdrop. A fresh headless full/partial comparison confirms those
pixels; it does not establish composed editor presentation.

A sharp render is released across two animation-frame boundaries, providing one
paint opportunity before hiding cached pixels. The cached fitted width must match the current live container, and geometry
must remain unchanged between those boundaries. An older dispatched width must
not block serial completion: native keeps the mask for its queued replacement.
New input cancels any older release callback. These checks prevent an older
scale/width from appearing during a quick divider reversal; they keep one pending
handoff chain, with no idle repaint loop. The partial renderer also uses top-left
SVG alignment so CSS height rounding cannot add a fractional top letterbox;
full rendering retains its existing alignment and rounded-axis scale model.

The final `partial_cache_edges` native journey passed for both modes, with six
narrow/widen drags per mode and positioned cursor zoom. The latest 48-page native
frontend probes passed both modes, including unequal page sizes, source anchors,
edge clamps and scroll/resize ordering. Matched optimized handoff probes kept
cached input submissions within 59–72 microseconds in these local runs; ready
acknowledgements increased by about 8–10 ms for the extra paint opportunity.
Those measurements use an owned SVG and fixed renderer stall, not GPU frame
presentation or a cross-platform latency guarantee. Details and limitations are
retained in `docs/performance-results/tinymist-zoom-page-order-2026-10-13.json`.


Preview liveness has explicit request ownership. A resize notification alone
cannot open an active flight. Accepted input carries its revision and renderer
identity through application and both paint boundaries; reset, disposal,
replacement, or a refused WebKit evaluation terminates only that request.
Cancelled input cannot replay an old source anchor. Native rejection resets JS
locally, avoiding an unversioned cancellation that could erase newer work.

Pinned Tinymist reconnects can register the same renderer repeatedly. The adapter
keeps one entry per implementation and accepts one unique live renderer;
separate implementations remain ambiguous. The maintained reconnect test closes
the actual owned WebSocket twice in both modes, updates source to 49 then 50 pages,
and checks navigation, zoom and request acceptance. Its before/after evidence,
plus the optimized queued-resize reproduction and composed native-input journey,
is retained in `docs/performance-results/tinymist-preview-recovery-2026-10-13.json`.

```sh
TIPTOPTYP_TEST_TINYMIST="$PWD/toolchain/bin/tinymist-aarch64-apple-darwin" \
TIPTOPTYP_TEST_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
TIPTOPTYP_PREVIEW_RECONNECT_RECEIPT="$PWD/.tiptoptyp/profiles/preview-reconnect.json" \
node scripts/test-preview-reconnect.cjs
```

Set `TIPTOPTYP_PLAYWRIGHT` to the installed Playwright module when it is outside
Node's search path. The test owns and cleans its browser, server and fixture;
its optional adapter override selects a preserved failing baseline.
