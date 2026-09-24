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
