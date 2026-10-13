# Window lifecycle and UI test audit

2026-09-23. Trigger: intermittent close/focus/shadow failures after macOS
minimize and app switching. The reported sequence is not yet reproduced; the
findings below are independently identifiable defects and coverage gaps, not
proof that all three symptoms have one cause.

## Findings and changes

1. Root document startup requested transparent native backing; secondary
   documents requested opaque backing and a shadow. Popup children already
   explicitly requested transparency. `window_policy` now defines document
   and popup roles; startup, secondary documents and persistent tools use the
   document policy. Document shadow, decorations and traffic-light capabilities
   are explicit. This removes role drift; a shadow still needs desktop evidence.
2. Interactive tooltip builders change `active` as their focus changes.
   In egui 0.36.2, `ViewportBuilder::patch` sets `recreate_window` when this
   creation option changes. Replacing native surfaces during focus handoff is
   unnecessary and loses native identity. Both viewport entry points now strip
   this creation hint for existing windows. Explicit Focus commands remain the
   mechanism for user-requested activation. Regression tests exercise egui's
   actual patch implementation, including repeated focus/blur cycles.
3. The root document rediscovered its native parent from AppKit's key window,
   despite eframe supplying its exact window handle. It now retains the supplied
   handle even while inactive. This prevents a stale egui focus observation from
   attaching root previews or dialogs to a popup or another document. The shared-host implementation below also binds deferred documents directly.

## What the tests actually establish

The suite has useful semantic widget, close transaction, child lifecycle,
owner-routing and pure geometry coverage. However:

- Most tests inject egui input. They do not execute AppKit activation,
  miniaturization, native traffic-light actions or WindowServer shadows.
- Builder-field assertions cannot catch runtime builder patch semantics. The
  new regression tests close that particular gap, without claiming OS coverage.
- `native_window_drag` is opt-in and normally exits successfully without running
  its native assertions. It constructs hidden NSWindows; its success does not
  establish a working visible close/minimize/focus sequence. Its retained-owner
  assertions now cover handle conversion and sibling isolation as well.
- A viewport framebuffer excludes native child composition and exterior shadows.
  It cannot certify either, regardless of how many theme screenshots pass.
- Root close cancellation in `logic` and document retirement in `ui` deserve
  native coverage while occluded/minimized. eframe's logic-only path preserves
  pending input, so absence of a UI pass alone is not proof of a dropped event.

## Implemented shared host

`window_host` is the single registry for document and child-window lifetimes.
`ChildViewHost` delegates to it; immediate and deferred rendering share
`ChildPaint` for style, native theme, input and capture handling. Document and
persistent-tool creation share `window_policy::document`; transient surfaces
share the popup policy. The old independent child registry is removed.

The small eframe patch binds each viewport to its actual native window before
calling its UI. Bindings are weak and renderer-assigned, with monotonic native
identities. Both native renderers publish them. `native_window::for_viewport`
uses this exact binding for root/deferred previews and Settings file dialogs;
key-window/active-window identity discovery is removed. No native handle is
inferred from keyboard focus. The bundled real-window test checks four distinct
owners and invariant identities through focus, minimize and visibility changes.

Host tokens include a registry-wide, non-reused generation. Closing or replacing
a native surface invalidates callbacks and pending results. Retired entries are
pruned after native removal; reusing an egui ID cannot resurrect an old token.
Native bindings retain no windows; callback validation rejects expired bindings.

Close requests have sequence numbers and remain pending until the document
accepts delivery into its existing revision-checked save/discard transaction.
The input hook admits them before painting. A busy document retains its request.
The eframe adapter also dispatches close to hidden/minimized deferred viewports:
those have no separate logic callback to answer it. Hidden idle windows still
skip UI. Native tests cancel and then commit close while a document is minimized.

All production native focus commands go through the host. Requests distinguish
user activation from a return after child dismissal. Returning focus requires a
live, visible, restored owner and current platform application activation.
Explicit activation waits for asynchronous restoration to make the window
visible before dispatch; it is consumed once on native input, without a timer
or idle repaint loop. A pending request started while active is canceled if the
application becomes inactive. Native recreation/retirement cancels old requests.
Preview creation uses current native focus, rather than delayed egui focus input.

## Tests and native lane

- Enumerate all 100,000 five-event sequences over ten lifecycle operations:
  show, hide, retire, close request, acknowledgement, owner retirement/resume,
  native replacement, stale callback and duplicate close. Assert sibling
  isolation, irreversible token invalidation and exactly-once acknowledgement.
- Exhaust every focus-policy input combination. Test pending restore delivery,
  stale focus after reopen, hidden close delivery, generation changes, and
  bounded registry storage over 1,000 close/reopen/prune cycles.
- Exercise egui's actual builder patching, existing child-lifecycle and semantic
  window tests. Architecture checks enforce the shared registry and focus
  boundary; these checks do not substitute for behavioral tests.
- `python3 scripts/test-native-windows.py` builds a dedicated macOS app bundle,
  launches it, and requires its fresh success report. It uses a copied executable,
  unique bundle ID and output directory. A bare command-line test can remain
  inactive on macOS; treating that as an app-bundle acceptance test is unreliable.
  `--prepare-only` prepares the same fixture for a desktop controller.
- The native fixture checks document/tool/popup decorations, buttons and native
  shadow properties, distinct exact owners, stable identity, minimize/restore,
  focus handoff, minimized close/cancel/commit and Settings hide/reopen. It has
  a 20-second internal deadline and the runner has a 45-second external watchdog.
  Missing desktop access, timeout or a missing report fails; none silently pass.
  The manually selected `native_windows` CI job runs this lane separately from
  the ordinary headless suite and retains its report.

These are exhaustive bounded model checks, not a proof about every possible
OS event sequence. Actual Cmd+Tab keyboard routing, IME/accessibility behavior
inside native previews, and the WindowServer's visible exterior shadow still
need desktop observation. Native `has_shadow` is a property assertion, not a
pixel-level claim.

## Further simplification

Moving transient cards into their owner's egui layers remains a separate preview
composition migration. Native Tinymist child views currently cover egui layers.
Removing their overlay windows requires a tested texture/snapshot composition
interface, including selection, scrolling, IME and accessibility. This change
keeps their existing composition and makes their native ownership/lifetime
explicit; it does not silently replace interactive previews with screenshots.

The host adds no workers, timers, per-frame I/O or unbounded event queues. Weak
bindings and registry access clone an Arc rather than the complete window map.
Detached operation completions are scoped to the application context, so an
independent shell cannot drain another shell’s queue. A parallel profiling test
exposed the former process-global queue race; the isolation regression exercises
two contexts and exactly-once draining.

Retired entries are collected; ordinary repaint cannot issue focus or recreate
windows. No CPU/FPS or cross-platform performance improvement is claimed.

## Initial audit evidence (before shared-host implementation)

Required format, strict Clippy, 1,219 Rust tests and 15 xtask tests passed (27
Rust tests ignored by the normal suite).
The opt-in AppKit test was also run directly and passed retained-owner and
sibling-isolation assertions. A debug native fixture launched with
`TIPTOPTYP_UI_TRACE=1` and `--ui-theme catppuccin-latte --ui-snapshot-scene main
--ui-screenshot-subdir screenshots/window-lifecycle test.typ`. Its fresh
`.tiptoptyp/screenshots/window-lifecycle/1790187592659-0001-main.png` was inspected:
editor and preview layout rendered without clipping or opaque popup artifacts
in the main scene. No popup scene was captured. Native observation showed the
close/minimize/fullscreen accessibility controls. After Cmd+M, native minimize,
and Cmd+Tab inputs, clicking the native close control exited the fixture with
status 0. The accessibility tree did not establish whether minimize/app-switch
transitions completed; this is a close smoke check, not a reproduction of the
reported failure. The observation crop excludes exterior shadow, which remains
unverified. No native preview bounds change was made.

## Shared-host validation (2026-09-24)

On macOS arm64 14.6.1, Rust 1.98.1, the standard suite passed 1,231 tests and
profiling passed 1,239, with 27 explicitly ignored in each configuration.
The packaging suite passed all 15 tests. Formatting and strict all-targets
Clippy were checked, including profiling and the native fixture feature.
The final rebuild disabled incremental compilation after the local disk filled;
only this worktree's disposable incremental cache was removed.

The fresh bundle prepared by `scripts/test-native-windows.py --prepare-only`
was launched through the desktop controller. Its report at
`.tiptoptyp/native-window-tests/run-26de3sog/result.txt` was:
`completed=true cancellations=1 closed=true failure=None`.
This verifies the native contract described above; it is not screenshot evidence
or a remote CI result. No preview geometry or maintained screenshot contract was
changed by the shared-host refactor.

## Alpha regression correction (2026-09-24)

The opaque document policy exposed a renderer dependency missed by the original
native tests: eframe selected its shared CGL configuration from root transparency.
Glutin then set the whole context opaque and finalized transparent child windows
without alpha. Transparent popup padding consequently appeared black. The native
regression reproduced `kCGLCPSurfaceOpacity = 1` with an opaque document root.
The macOS renderer now requests alpha support independently of root NSWindow
opacity. Documents keep opaque native windows and shadows; popups preserve alpha.

New transparent child surfaces stay hidden until their first successful buffer
swap, addressing the unpainted first-open flash. Visibility changes before that
swap update the pending intent, including explicitly hidden children. No idle
repaint loop or delay is added. Native tests now cover the actual CGL setting and
NSWindow opacity, plus initial hidden/revealed state, rather than relying on
viewport builders or framebuffer pixels alone.

Settings close previously focused its hosting root, regardless of which document
was used last. The shared host now records actual window activation in a unique
recency list and returns to the latest visible, restored, live document or tool
window. Transient undecorated cards are excluded. Focus is read from all live
native bindings because activation can precede the destination’s own repaint. Explicit
close removes the closed window from that history; application deactivation
still prevents focus stealing. Regressions cover repeated activation, retired
and minimized candidates, secondary-document return, and background close.

The final native bundle report
`.tiptoptyp/native-window-tests/run-0r89jt1h/result.txt` records
`completed=true cancellations=1 closed=true failure=None`. It includes Settings
return to the secondary document, then fallback to the surviving root after the
secondary closes, without initial activation of the passive popup. The fresh
function-tooltip framebuffer at
`.tiptoptyp/screenshots/alpha-fix-final/1790237500021-0001-diagnostic-function-tooltip.png`
was inspected: the rounded card and text are intact, outside pixels are RGBA
`(0, 0, 0, 0)`, and its interior is opaque. This is framebuffer evidence combined
with native opacity/first-presentation checks, not a high-speed desktop recording
of every possible preview-loading flash. The change adds no timers or idle
repaints; focus history is bounded by live windows and read at input admission.

Final checks: 1,233 standard tests and 15 packaging tests passed, along with
formatting and strict all-targets Clippy (including the native test feature).

## Find and empty-workspace corrections (2026-09-24)

Find/replace keeps its native host across focus handoffs and temporarily hides
it when the owner and tool both lose focus. A bounded 150 ms grace period lets
native focus events arrive without repeatedly destroying and reopening the
popup. Creation-only activation options also check the live native binding,
because a parent's input snapshot can omit its child. Find shortcuts are
consumed in the child's input pass; explicit close returns focus to the editor.
Modeless controls resize without stale minimum/maximum constraints fighting
Replace expansion. The card is capped at 460 logical pixels with smaller padding.

Closing the last tab clears document popups and retires pending writing checks.
Previously the writing-check debounce could request an active-tab path from an
empty tab set and panic, taking the window down. The empty workspace remains
open and does not grant a window-close request.

Regressions cover repeated native Cmd+F closure, hidden-host registration,
search retention, single-click Replace expansion/collapse and close, focus
handoff grace, unconstrained modeless resizing, and the expired writing-check
deadline after closing the last tab. Activity tests also verify identical label
positions across every state. These changes add no recurring idle timer or
background work; focus handoff schedules only a bounded follow-up repaint.

Validation: formatting, strict all-target Clippy, the full Rust suite (including
1,069 application unit tests), and all 15 tooling tests passed. The 23-image
maintained gallery regenerated in one session and validated. Fresh framebuffers
`.tiptoptyp/screenshots/panel-fixes/1790265410193-0001-find-replace.png` and
`1790265410482-0003-main-activity-panel.png` were inspected: the Find controls fit
on one compact row, Replace adds one row, and Activity labels wrap without state
suffixes. The trace is retained beside them. This offline fixture uses PDFium
and emits no native preview bounds; these captures verify individual viewports,
not composed desktop overlap or every native focus transition.


## Translucent backgrounds (2026-10-11)

macOS document and persistent-tool windows retain native alpha capability. Their
decorations, shadow and traffic-light policies stay explicit. Appearance has a
transparency toggle and background opacity (80% by default). Turning the effect
off paints the original opaque background; toggling never replaces a window.
Other platforms retain opaque backgrounds.

The initial implementation inserted an `NSVisualEffectView` below the renderer's
other children. That was still **above the parent's OpenGL output**: the installed
editor became entirely grey while its higher native Tinymist child remained
visible. Offscreen renderer captures could not show this native overlay. A new
native regression failed before the fix; a composed-window observation also
reproduced the user's screenshot.

The initial recovery removed the native blur child. The restored implementation
uses a sibling `NSVisualEffectView` below the existing renderer, with AppKit's
behind-window blending and under-window material. Soft blur is on by default in
normal builds and can be disabled in Appearance. A radius slider (0–50, default
8 rather than the material’s 30) adjusts its existing native Gaussian filter.
A decorative-view `updateLayer` override reapplies the radius when AppKit changes
the material. This uses private filter KVC; missing filters retain the native
material. No custom backdrop or content-view replacement is introduced. Renderer
children and the responder chain stay intact. The owner handle is explicit,
with no key-window lookup or activation. Native configuration is cached by
viewport identity, enabled state, blur preference, radius and appearance.
Theme changes reuse the material view; opacity changes require no native update.
AppKit resizes the underlay; a bounded parent check repairs fullscreen reparenting.
The viewport cache removes the underlay on AppKit's main thread when released.

The theme palette remains opaque. Only the viewport-local UI style and background
paint change: the window tint is drawn once, nested panel frames remain clear, and
editor/Explorer/terminal surfaces add a light shade. Text, controls, diagnostics,
images and PDF page colors retain their normal appearance. There is no added idle
repaint loop, per-frame native view mutation, image blur or resampling.

Deterministic tests cover JSON limits, local palette isolation and stable settings
geometry. Native tests enforce no decoration children over CGL output, stable
content/window identity, resize and sibling-window isolation. Framebuffer PNGs
show renderer pixels but cannot prove native composition; that needs a separate
whole-window observation.

### Blur placement investigation (2026-10-11)

An isolated copy of the actual editor was tested with a temporary AppKit probe.
The effective backend was Tinymist partial, with native readiness confirmed by
the desktop inspection endpoint. Fresh CUA window observations reproduced the
old child placement hiding the editor, while a sibling `NSVisualEffectView`
ordered below the existing `WinitView` kept the editor and native preview visible.
The sibling's parent was the content view's existing superview; the renderer was
not reparented or replaced. Find received native typing, and window zoom resized
the underlay from 1400×885 to 1512×887 points. Content-view and first-responder
identities stayed unchanged when attaching/removing the effect.

The initial probe observation did not establish how desktop content behind the
window was blurred; the contrasting backdrop was not visible in its window image.
A repeated-transition probe experiment was invalidated by using a numeric signal
that is not SIGUSR1 on macOS; it is not counted as coverage. That investigation
changed no behavior. Subsequent restoration enables the sibling placement in
normal builds and defaults, with native geometry/lifecycle tests and a real-editor
journey covering both preview backends, Settings, repeated toggles, appearance,
resize and keyboard input.

Do not replace the window's content view with a wrapper: winit 0.30.13's
`WindowDelegate::view` casts `window.contentView()` to `WinitView`, assuming it
still owns that object. The restored underlay must retain that identity, remain
below the renderer, follow native geometry, and be removed with its owner.
AppKit's behind-window blending is the intended effect; a CPU screenshot blur
or renderer migration is not required by this placement experiment.

Probe source, binary hash, native geometry and limitations are recorded in
`.tiptoptyp/profiles/native-blur-investigation/review.json`. Apple references:
[behind-window blending](https://developer.apple.com/documentation/appkit/nsvisualeffectview/blendingmode-swift.enum/behindwindow)
and [under-window material](https://developer.apple.com/documentation/appkit/nsvisualeffectview/material-swift.enum/underwindowbackground).
