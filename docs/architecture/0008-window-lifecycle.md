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
   attaching root previews or dialogs to a popup or another document. Deferred
   document callbacks still lack a supplied handle; that gap remains explicit.

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

## Architecture direction

Use one **WindowHost** per real top-level window and one process-level registry.
The host owns native identity, creation policy and lifetime; views return typed
intents instead of issuing arbitrary window commands. Build on the existing
ChildViewHost generation registry, rather than introducing a second registry.

The registry key is `(ViewportId, generation)`, bound by the platform integration
when a native window is created, never inferred from keyboard focus. A retained
handle alone is insufficient: it can keep an obsolete view alive after recreation.
Use owner tokens to admit preview attachments, file dialogs and focus requests;
reject tokens from earlier generations. Root handle binding is implemented here;
complete deferred binding requires an eframe integration hook, not matching
window titles or enumerating whichever NSWindow happens to be key.

Model lifetime, visibility and activation separately. Minimization and app
inactivity do not destroy a document; close is an acknowledged transaction with
an owner and request identifier. UI painting cannot consume the only copy of a
close request. Keep pending close work until canceled or committed, including
when no UI pass runs. Effects such as Reveal, Focus and Retire are emitted once
from transitions. Background completion and ordinary repaint cannot activate a
window. Modal dismissal can return focus only while the application remains
active and the owner generation still exists.

Reduce the number of real windows. Menus, tooltips and modal cards should ideally
be egui layers inside their owner. Native previews are the reason those layers
currently need separate windows. For texture-backed previews, use in-window
overlays directly. For native previews, put composition behind a PreviewSurface
interface: either render into the owner, or temporarily replace the native view
with its retained snapshot while an overlay is open. Do not switch until
selection, scrolling, accessibility, IME and snapshot freshness are validated.
Settings and independent documents remain true windows. Removing native popup
windows eliminates their activation/reparenting/shadow bugs by construction.

## Acceptance gates for the next host migration

- Sequence tests: create, focus, blur, minimize, restore, popup handoff, dirty
  close/cancel, close/commit, stale callback, recreate, sibling close and quit.
  Generate event permutations and assert no cross-owner effect, no resurrection,
  exactly one close result, and no activation from repaint/background work.
- Adapter tests assert native identity survives visibility/focus changes, and
  generation changes exactly once on genuine recreation. Test actual builder
  patches, not only expected builder values.
- A dedicated native desktop lane must launch an isolated fixture with isolated
  preferences, then exercise Cmd+M, Cmd+Tab/return, restore, traffic-light close,
  Cmd+W, dirty cancel, two documents and Settings. A missing desktop is reported
  as unavailable, not as a passing native test. Verify visible focus and shadow
  with whole-window observation; retain viewport images only for their scope.
- Keep the existing fast tests. Do not replace them with brittle pixel matching
  or broad source-string assertions. Source checks enforce boundaries, not
  behavioral correctness.

The present changes add no timers, workers, per-frame I/O or unbounded buffers.
They remove accidental native recreation during focus changes. No timing or
cross-platform performance claim is made.

## Validation evidence from this pass

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
