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
interface. A sidecar upgrade must rerun the real-frontend probe. Slides, partial
rendering, uninitialized/replaced documents, and non-SVG modes are not intercepted.

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
No bitmap overlay has been added to production yet.
