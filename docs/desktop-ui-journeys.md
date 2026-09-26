# Real desktop UI journeys

This lane launches the actual macOS editor, posts native mouse and keyboard
input, and checks observations from the running document window. It complements
`egui_kittest`; it does not replace focused deterministic tests or prove native
visual composition from state alone.

## Run

Use a logged-in macOS desktop with Rust, Swift and the repository's fetched
PDFium/toolchain dependencies. Grant Accessibility/input-posting permission to
the launching host (for example Terminal or Codex) in System Settings → Privacy
& Security. The runner checks permission without prompting or silently skipping.
Do not type or switch apps during a run: the driver refuses input when the
fixture process is not foreground.

```sh
python3 scripts/test-desktop-ui.py
python3 scripts/test-desktop-ui.py --journey panels
python3 scripts/test-desktop-ui.py --journey find
python3 scripts/test-desktop-ui.py --journey empty
python3 scripts/test-desktop-ui.py --journey focus
python3 scripts/test-desktop-ui.py --journey preview
```

The runner compiles a separate `desktop-ui-tests` feature build and copies it
into a uniquely identified temporary app bundle. It launches one editor window
with a disposable document, fresh settings and session storage. Preview/controls/tabs/layout/background/all runs
use a three-page Typst document and require working Tinymist, Typst and PDFium tools. The terminal
journey currently requires the system `/bin/zsh`; it isolates zsh startup files
and shell history with `ZDOTDIR`. No user document is opened. The fixture app
and its process group are terminated on success or failure.

Multiline fixture input uses an acknowledged native paste. The adapter holds the
previous clipboard representations in memory, waits for the runner to observe
the expected source fingerprint, then restores them unless another process has
changed the clipboard meanwhile. Clipboard contents are never written to logs.

Normal builds contain no inspection endpoint. A feature build only starts one
when `TIPTOPTYP_DESKTOP_TEST_DIR` names a fresh private directory (mode 0700).
That directory owns both the settings file and a mode-0600 Unix socket.
The socket only accepts `snapshot\n`; there is no command or state-mutation API.
It requests an egui repaint and one native redraw per existing viewport, then
waits for a fresh document UI pass with a deadline. The native wakeup avoids
losing an observation to repaint coalescing during a finishing theme-change pass.
Document state and hit targets are published atomically so a following frame
cannot overwrite half of an observation. It does not write per-frame files or
run a background repaint loop.

The input adapter uses AppKit to activate the fixture process and inspect native
windows, and CoreGraphics to post ordinary mouse/key events. The production
build currently does not expose all egui controls through macOS accessibility,
so clicks use named hit rectangles observed from the actual widget responses.
These are screen coordinates including interface zoom, not hard-coded offsets.
The driver rejects missing, disabled, hidden or stale targets. It posts a native
move, then waits under a deadline for fresh, settled geometry before its single
click. A search-scroll animation may finish during this readiness wait; an
already posted click is never retried. AppKit/WKWebView
can suppress passive root-view hover, so hover is diagnostic rather than a click
prerequisite; every action must still produce its asserted result. Its observations
cannot invoke application handlers. Startup waits for the native document window to register its title and window
attributes, then activates it and waits for focus. Native event edges are separated by 50 ms so move/press/release
are delivered across event-loop turns; outcome assertions still wait on state.

## Current coverage

See [the promotion audit](desktop-ui-audit.md) for the original tests, the portions
now exercised through the real app, and the lower-level coverage retained.

| Journey | Input and assertions |
| --- | --- |
| panels | Cmd+5, repeated clicks on Terminal/Problems, hide/reopen; stable panel height across 30 fresh frames per sample; app remains responsive |
| find | Cmd+F with Find focused, repeated Replace clicks, one close-button click; expected popup state and stable measured native height after every action |
| empty | Cmd+W on the only tab, then Cmd+N; original window remains, new tab has no path and zero source bytes |
| editing | Typing and Unicode replacement, undo/redo caret, acknowledged multiline paste, comment round trip |
| search | Query focus, next/previous, case/regex toggles, replace one/all, undo and Escape |
| tabs | Previous/next with separate unsaved buffers and carets; original preview retained |
| layout | Code/preview/split, explorer hide/maximize/restore, editor setting shortcuts, panel maximize/Activity/close |
| folding | Repeated shortcut and gutter collapse/expand without losing headers or source |
| closing | Dirty-close Cancel/Escape preserve text; explicit Discard closes only the requested tab |
| controls | Both engines: stable outline popup size, app-switch hide/restore, outline/history/pages/zoom/fit/search, minimize/reopen, detach/return |
| settings_search | Search destinations highlight individually, then expire |
| background | Switch to Tinymist with its pane hidden; frontend finishes loading; reveal reuses it and paints pages |
| focus | Two document windows; Settings opened/closed from alternating owners; native focus returns to each owner; minimize/restore, Finder app switch, close second window once |
| preview | Pin a real Typst fixture; switch PDFium → Tinymist through Settings; require the selected renderer to become ready; light/dark × comfy transitions; applied palette and live Tinymist DOM checks |

Find is a real native child viewport. The focus journey uses native Accessibility
window actions and verifies native focus independently of the inspection socket.
Other journeys reject unexpected document-owner changes.

Preview checks compare the effective page palette with each renderer's applied
palette. Tinymist additionally requires a rendered page, exactly one palette
filter in its ancestor chain, the expected paper color, and matching transfer
coefficients. This detects missing/double transformations and stale DOM state.
It does **not** prove final screen pixels: an on-screen compositor bug may still
pass DOM checks. No document content is included in DOM observations.

For independent composed-window inspection, run:

```sh
python3 scripts/test-desktop-ui.py --journey preview --review-preview
```

This pauses at each renderer's dark/comfy state for up to three minutes and
prints the exact isolated app path and a resume-file path. Inspect that app's
whole window using the approved computer-use tool (not another running editor).
Record the observation in the printed resume file to continue. The file is an
operator acknowledgement, not an automated visual assertion. Manual viewport
captures can be taken with Cmd+Shift+F12 and retained before fixture cleanup;
Tinymist's native child is not composited into that framebuffer capture.

Still to add: real save dialogs and TeX save-as; restored Typst → pinned TeX
builds; automated assertions against composed on-screen pixels.
The existing `scripts/test-native-windows.py` covers a separate native lifecycle
fixture, not those complete editor journeys.

Use `--capture-review` to retain the initial toolbar/root framebuffer and, with
`settings_search`, `controls` or `all`, the affected settings/popup framebuffers for separate inspection. This is opt-in, never a visual
pass by itself. Use `--trace-preview` to retain native bounds/property traces.
`--review-failure` retains a failed fixture for up to three minutes and prints its
exact app path; writing the printed resume file allows cleanup, without changing
the failed result.

Hidden Tinymist preloading checks document/container readiness. WebKit may defer
animation-frame painting while its NSView is hidden; the background journey
separately requires visible pages after reveal without a document reload.

## Evidence and failure policy

Each run leaves `.tiptoptyp/desktop-ui-tests/run-*/` with:

- `result.json`: pass/failure, completed journeys, platform, exact binary version
  and SHA-256, Git revision/diff summary, and initial backend/settings state.
- `events.jsonl`: ordered native actions, observed targets and bounded state
  snapshots. No document contents are included. Preview runs also record read-only DOM
  palette/filter observations.
- `app.log`: app stdout/stderr, including startup failures.
- The isolated persisted settings file, when the app wrote one.

No journey is retried into a pass. Polling waits for explicit conditions under a
deadline; missing permissions, stale observations, premature app exit and timeout
are failures. Frame stability checks advance the real UI instead of sleeping for
an assumed animation duration. These are functional checks, not timing benchmarks.

```sh
# Build without claiming desktop execution:
python3 scripts/test-desktop-ui.py --prepare-only
# Real startup/inspection protocol only, without synthetic input permission:
python3 scripts/test-desktop-ui.py --observe-only
# Headless checks for the runner's drift detector:
python3 scripts/test_desktop_ui_contract.py
```

`--prepare-only` and `--observe-only` deliberately leave `passed: false`.
Observation success is recorded separately as `observation_passed: true`.
Neither counts as an interaction-suite pass. The drift detector checks include
the previously observed 3-point-per-frame growth and invalid/missing geometry.

## Integration and next steps

The manually dispatched Desktop UI workflow targets a dedicated, logged-in
self-hosted macOS runner labelled `desktop-ui`. Its host must already have the
permissions above, and must remain unused while native input is running. It uploads evidence even on failure. It is not automatically
run against arbitrary pull-request code on a personal desktop.

Before merging a change to a covered interaction, run that journey and report
its result separately from Rust checks. Where native execution is unavailable,
state the missing capability explicitly; a headless or observation-only pass is
not a substitute. For preview work, first assert the requested and effective
backend and the displayed artifact identity. For pixel/composition changes,
retain the separate visual evidence required by `AGENTS.md`.

Extend this suite one demonstrated regression at a time: reproduce the old
failure, exercise the real input path, then assert the visible outcome and its
stability. A broad list of scenarios with no native execution is not coverage.
