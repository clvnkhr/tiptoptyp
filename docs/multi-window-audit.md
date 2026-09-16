# Multi-window audit (todo 159)

Audited on 2026-09-15: process/native-menu dispatch, Finder-open delivery,
document-window lifetime, focused child ownership, shared settings/history,
close coordination, background completion routing, and explicit root viewport
references.

## Fixes

- Refresh the active document from current viewport focus before dispatching
  native commands. A document's Settings and other scoped children count as
  that document's focus; a hidden root's stale focus does not.
- When the active secondary closes, select a surviving document instead of
  unconditionally selecting the hidden root.
- Dock reopen focuses and restores an existing document, including a minimized
  one. Only when no document remains does it reveal the retained root. Native
  document commands do not steal focus from an already visible Settings window.
- With no documents, New, Open, New Window, Open in New Window, Settings, and
  process Quit remain usable. Document-only menu commands are disabled and
  ignored if already queued. Clipboard commands still work in a focused child
  such as Settings, without revealing the hidden document host.
- An accepted root-window close replaces its document with a clean untitled
  buffer. Discarded source, file destination, undo history, and autosave deadline
  cannot survive the close. The document epoch changes, rejecting late results
  and save receipts for the closed document. Dirty primary quit confirmations
  explicitly reveal their owner when needed.
- Finder-open sends explicitly wake the root process dispatcher. Startup events
  remain queued until the shell exists; disconnected sends do not request a
  repaint.
- Merge every pending Settings edit against its owner's applied settings,
  inactive owners first and active last. Independent field edits survive;
  active-owner precedence applies only to conflicting fields. Existing MRU and
  explicit workspace-removal rules remain intact. No settings snapshot is
  cloned by synchronization when no window has submitted an update.
- Detached process-operation completion notices go to the active owner rather
  than always to the primary document.

## Root references that are intentional

The eframe root remains the process/event-loop host, not necessarily the active
document. Native menu/open callbacks and exclusive background-operation
completion wake that host. Process Quit is coordinated across document keys
before closing it. The root frame counter in `viewport_fonts` gates shared font
atlas maintenance once per shell frame. Screenshot batch orchestration and
profiling also belong to the process host. Document workers, dialogs, preview
parents and child IDs retain their own session/viewport ownership.

## Verification and limits

Focused deterministic tests cover surviving-window selection, current document
and child focus before owner paint, no-document command policy, independent
settings edits/conflicts, startup and live Finder-open delivery, root-only wakeup,
failed-send silence, and discarded-buffer/epoch/autosave/history reset. Existing
tests cover close transactions, owner-specific async completion, scoped child
input, settings-child scrolling, and one-shot window activation.

Required checks: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test --no-fail-fast`, and
`cargo test --manifest-path xtask/Cargo.toml`.

These are state/routing changes, not a visual-layout contract change. No native
Dock/Finder interaction or composed desktop screenshot was exercised in this
pass; headless focus/event tests do not prove AppKit focus behavior end to end.

No material steady-state performance change is expected and no CPU/FPS claim is
made. Focus selection is bounded by window count and the fixed owned-child set;
there are no new timers or continuous activation/repaint requests. Open-event
wakes and preference merges are event-driven. Closing the retained root uses
the existing untitled-document service reset once, not per frame.

The dormant-host follow-up identified here is implemented below.

## Dormant-host lifecycle (todo 160)

Implemented 2026-09-16. `DocumentLifecycle` separates active, dormant, and
resume-pending states. Closing the retained root cancels compile/index deadlines,
pauses the compiler watcher, stops the Tinymist session and recovery retries,
releases the native preview, invalidates asset requests and supersedes workspace
and index results. It clears the document without starting replacement services.
Already-running read jobs may finish, but their results cannot be accepted;
started exclusive mutations remain alive and deliver completion through the
process mailbox. File-import/package completions and pending native dialogs
continue to be polled by the host.

eframe 0.36 does **not** call `ui` when the root and all its descendants are
hidden. It calls `logic`, with current window state but stale widget input.
Process commands and dormant-host completions therefore run in `logic`, without
painting or consuming old input. Visible child windows get a small host UI pass
instead of editor layout, preview rendering, workspace polling or compilation.
Only focused child input is consumed there.

The last root UI pass registers a hidden deferred Settings viewport. Native
Settings commands can reveal this existing viewport directly from `logic`.
Closing it while dormant hides rather than destroys it, allowing another
no-window reopen. Preferences, clipboard editing, tool pickers, shortcuts and
other Settings-owned children continue to work. A new document request reveals
the retained root; queued document replacements and preference changes are
applied before one service restart on the visible resume pass. New Window with
no existing documents reuses that clean root. A late new-window dialog result
does the same instead of waiting forever for an invisible parent's UI pass.

Idle compiler and Tinymist workers use blocking receives when no session is
active; only a live watcher/server needs timed polling. Shutdown and new work
wake those same channels. No polling timer or artificial repaint loop was added.

### Measurements

Run with `cargo xtask profile --scenario no-window --warmup 6 --seconds 6`:
macOS/aarch64, optimized `profiling` build with frame pointers, native `sample`
at 1 ms, Catppuccin Latte, the same small isolated fixture and unchanged initial
viewport. The automatically captured initial document PNGs were inspected; they
are not evidence of native hidden-window or Dock composition.

- Baseline: `.tiptoptyp/profiles/1789509105749-62015-no-window-0`.
- First after-run: `.tiptoptyp/profiles/1789539645260-64582-no-window-0`.
- Final after-run (including stale-input/late-dialog guards):
  `.tiptoptyp/profiles/1789540835085-69175-no-window-0`.

Both completed the six-second measurement with no recorded UI spans or root
repaint requests. The baseline compiler was in timed polling and Tinymist used
timed semaphore waits, with Tinymist stdout/stderr reader threads retained. In
the first after-run, all 5,250 compiler samples were in the blocking condition
wait and all 5,250 Tinymist samples were in the blocking channel/semaphore wait;
the Tinymist reader threads were absent. Sample counts are observations of wait
stacks, not CPU percentages or counts of wakeups. Coarse `ps` readings were 0.0%
in both runs; there is no credible percentage-speedup claim at this idle floor.
The after-run also contained unrelated AppKit/accessibility activity. Sidecar
CPU, energy use and end-to-end reopen latency were not separately measured.
The final run reproduced the blocking waits in all 5,316 samples of each worker,
with no retained Tinymist reader threads, no recorded UI/repaint work, and coarse
process CPU time unchanged at 1.39 seconds across the measurement. Fixture and
toolchain-manifest hashes match the baseline; each run retains its binary hash
and full build/platform metadata.

### Regression coverage

Tests exercise repeated suspend/resume (one activation only), cancellation of a
pending resume, service-request suppression in dormant/resume-pending states,
native-command and canceled-dialog delivery through `Context::run_logic`,
no-window Dock/New Window/Settings routing, retained hidden Settings registration
and close commands, and rejection of stale closed-document keyboard input.
Existing worker tests cover mailbox wakeup/disconnection and owner-close mutation
completion. Native Dock/Finder clicks and file-picker interaction remain manual
QA limits; this pass does not claim they were exercised end to end.

## No-window reopen follow-up (2026-09-16)

The dormant Settings viewport was registered only while the document lifecycle
was inactive. On reopening, the first active root frame omitted it, so eframe
removed its native surface before switching the shared OpenGL context back to
the root. On macOS a detached CGL view reaches glutin 0.32.3's
`is_view_current` assertion (`context to have a current view`). Settings itself
worked because opening it did not activate the document lifecycle.

Once Settings has been allocated as a dormant host, its native surface now
remains registered through document resumes, Settings closes and modal states.
It is hidden rather than destroyed. Hidden registration reuses the existing
presentation snapshot instead of rebuilding settings/toolchain/font data on
each editor frame; no polling timer or recurring repaint is introduced.

Native menu eligibility and dispatch share the same predicate. With no document,
New Window, Open, Open in New Window and Settings remain available; New and
document commands do not. Clipboard commands may target a visible focused child,
but not a hidden child with stale focus. Menu updates also run in hidden logic,
where no root UI pass is guaranteed.

Regression coverage exercises retained Settings registration before/after
resume and Settings close, no-window command rejection, New Window/Open routing,
and hidden-child focus. These are deterministic state/viewport-output tests,
not native CGL or AppKit menu verification. No rendering/layout contract changed.

## New-window shadows — 2026-09-16 (164)

The report was narrowed to newly created document windows, not the initial
window opened with a file. Secondary document builders unnecessarily requested
transparent backing, unlike persistent Settings windows. They now explicitly
request opaque backing and native shadows. The working root builder is unchanged;
popup children retain their independent transparency and no-shadow policy.
No per-frame native calls, shadow invalidation loop or preview-bound changes
were added. Builder tests cover both initial activation and later frames.

An isolated release app copy with its own bundle identifier was launched in
non-persistent QA mode. Using its native File → New Window menu created and
focused `Untitled.typ`; the editor and WKWebView preview rendered, and quitting
the QA process completed successfully. The existing user app was not operated.
The computer-use app capture is cropped at the window boundary, so this verifies
the native creation path and content, **not the exterior shadow pixels**. Manual
shadow confirmation remains necessary. Root framebuffer
`.tiptoptyp/screenshots/agent-review/1789550158961-0001-main.png` was also inspected;
it does not capture the secondary window or its exterior shadow.

## Independent document repaints (todo 174)

The document UI was coupled through `show_viewport_immediate`: input, a caret
blink or a worker completion in one document made the shell paint every document
and switch native OpenGL surfaces between them. Compilers and Tinymist workers
were already separate. Moving AppKit/Wry objects to worker threads would not
address that repaint multiplication safely. This matches the
[egui immediate-viewport contract](https://docs.rs/egui/0.36.1/egui/struct.Context.html#method.show_viewport_immediate).

Secondary documents now use deferred viewport callbacks. A root pass registers
their continued existence without laying them out or swapping their buffers.
Their input, timers and worker completions wake only their owner. Focus changes,
document replacement/dirty transitions, settings edits, new-window requests and
close answers notify the process dispatcher. Native menu commands explicitly
wake their destination, and process dispatch also works in root logic when the
root is hidden/occluded. Webview navigation callbacks retain their owner repaint
target rather than a context whose active viewport can change.

Native editors remain UI-thread-owned, with no `unsafe Send`, cross-thread native
handles or rendering lock around the shell. A deferred callback carries a unique
token; a thread-local weak registry resolves it only on its creating event-loop
thread. Closing the host removes the registration, so late callbacks cannot
touch a closed/replaced document. The root supplies its eframe native frame;
deferred documents use only their own retained native parent for previews and
dialogs. Existing native bounds and window appearance are unchanged.

Shared preference broadcasts coalesce and apply on each owner's own viewport
pass, without echoing back as edits. Explicit workspace-history removals also
invalidate queued broadcasts so sleeping windows cannot resurrect forgotten
entries. No polling loop, continuous repaint or per-frame disk logging was added.

Regression tests cover deferred registration without editor painting, repeated
wheel-input frames without parent/sibling passes, root frames without child
painting, owner-local settings application, late callback safety/thread affinity,
clean child closing, queued-history removal and one-shot native command delivery.
The existing dirty-close, process-close, focus, Settings and dormant-root tests
remain applicable. These test scheduling/state rather than wall-time thresholds.

Reproduce the isolated four-window native workload with:

```sh
cargo xtask profile --scenario multi-window --warmup 8 --seconds 8
```

The runner opens the same small fixture in four independent sessions once before
warmup; it does not manufacture interaction or a repaint loop. Its `--binary`
option permits repeated measurements against a preserved optimized baseline,
with the actual binary/fixture/tool hashes retained in each run's metadata.

Validation: formatting (root and xtask), strict all-target Clippy both with and
without `profiling`, the complete ordinary suite (59 library and 741 application
tests plus supporting suites), the profiling suite (747 application tests plus
supporting suites), and all 13 xtask tests passed. This changes scheduling, not
the maintained visual layout or native preview bounds; gallery regeneration and
native geometry traces are not required for this change.

### Matched native measurements

2026-09-16, Apple M2 Max, macOS 14.6.1 (23G93), arm64, rustc 1.96.0;
optimized `profiling` build with frame pointers. Four copies of the same 812-byte
fixture, unchanged main QA scene/Latte launch override and inherited secondary
preferences, default window sizes (1400 × 900 points, constrained by the display),
same display/scale, 8 seconds warmup followed by 8 seconds idle measurement,
native `sample` at 1 ms. No typing, scrolling or window resizing was scripted.
The final three runs were sequential with no concurrent builds or test runs.

| Run | Editor passes / total UI ms | Shell mean ms | Process CPU advance, s |
| --- | ---: | ---: | ---: |
| Preserved immediate baseline | 384 / 380.46 | 14.08 | 0.97 |
| Final deferred build A | 73 / 65.89 | 1.02 | 0.21 |
| Final deferred build B | 136 / 108.16 | 0.78 | 0.35 |

Artifacts under `.tiptoptyp/profiles/` (metadata, summary, CPU sample, logs,
process readings and initial framebuffer):

- Baseline repeat: `1789574977093-82556-multi-window-0`.
- Final A: `1789574955871-81866-multi-window-0`.
- Final B: `1789574998189-77827-multi-window-0`.

The baseline's 96 shell passes each painted all four editors. Shell spans
previously included child painting and native surface switches/buffer waits;
deferred child work is now outside that span. Thus the shell reduction is not
an end-to-end FPS multiplier. The editor totals measure UI work across all
documents, not native swaps, and must not be summed with overlapping shell
spans. `ps` CPU readings bracket sampling and analysis rather than an exact
eight-second CPU interval. Caret, hover, native events and polling timings cause
run-to-run count variation. Worker/sidecar CPU, large projects and active native
scroll latency are not established by this small idle fixture.

Early diagnostic runs `1789573832717-71981-multi-window-0`,
`1789574196766-73148-multi-window-0`, and
`1789574493543-74756-multi-window-0` are retained but not used in the table:
development checks overlapped those runs and some spans contain long pauses.
They identified the exact 4:1 paint coupling, native GL surface-switch/wait
stacks and secondary caret/hover/polling repaint causes. The final comparison
uses the preserved baseline binary rather than reconstructing an old tree.

Binary SHA-256 (original optimized baseline build and final build):

```text
3e988f7b52d4d07dafa57494f8a0fb47e3c28ef371058c2bb1f1c9dd1fe4b3bd  baseline
808bd595d7ed9ce27d64990010d0445fe965274d84ef858f04e0f7ed7cd49bfc  final
```

Fixture hash `3625fd0a2b8656e8569d3a9df61ac867a2454fd38df1baea54af9b3bf8a38289`
and toolchain manifest hash
`f25c5aace663cb0a8b742081d5c748b1a15069f75fd65d18414be19986f75301`
match across the compared runs; full compiler/sidecar/build provenance is in
their metadata. These local results are not a cross-platform performance claim.

All six initial viewport PNGs were freshly captured and inspected: editor text,
toolbar and preview fallback remain aligned without missing glyphs. Final A
uses `workspace/.tiptoptyp/screenshots/1789574958450-0001-main.png`;
final B uses `workspace/.tiptoptyp/screenshots/1789575000236-0001-main.png`.
These are individual egui framebuffers, not proof of composed native preview
geometry or manual cross-window focus/file-picker behavior.

The final binary also passed the existing `no-window` native smoke profile
(`1789575059389-83419-no-window-0`, six-second warmup/measurement, sampler none):
no UI spans or repaint requests, with process CPU unchanged at 1.04 s. Its fresh
initial framebuffer `1789575061288-0001-main.png` was inspected as well; it does
not visually establish the later hidden-window state.
