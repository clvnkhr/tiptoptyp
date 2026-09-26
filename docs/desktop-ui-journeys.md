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
```

The runner compiles a separate `desktop-ui-tests` feature build and copies it
into a uniquely identified temporary app bundle. It launches one editor window
with a disposable text document, fresh settings and session storage. The terminal
journey currently requires the system `/bin/zsh`; it isolates zsh startup files
and shell history with `ZDOTDIR`. No user document is opened. The fixture app
and its process group are terminated on success or failure.

Normal builds contain no inspection endpoint. A feature build only starts one
when `TIPTOPTYP_DESKTOP_TEST_DIR` names a fresh private directory (mode 0700).
That directory owns both the settings file and a mode-0600 Unix socket.
The socket only accepts `snapshot\n`; there is no command or state-mutation API.
It requests a repaint and waits for a fresh document UI pass, with a deadline.
It does not write per-frame files or run a background repaint loop.

The input adapter uses AppKit to activate the fixture process and inspect native
windows, and CoreGraphics to post ordinary mouse/key events. The production
build currently does not expose all egui controls through macOS accessibility,
so clicks use named hit rectangles observed from the actual widget responses.
These are screen coordinates including interface zoom, not hard-coded offsets.
The driver rejects missing, disabled, hidden or stale targets. Its observations
cannot invoke application handlers. Startup waits for the document window to
receive focus. Native event edges are separated by 50 ms so move/press/release
are delivered across event-loop turns; outcome assertions still wait on state.

## Current coverage

| Journey | Input and assertions |
| --- | --- |
| panels | Cmd+5, repeated clicks on Terminal/Problems, hide/reopen; stable panel height across 30 fresh frames per sample; app remains responsive |
| find | Cmd+F with Find focused, repeated Replace clicks, one close-button click; expected popup state after every action |
| empty | Cmd+W on the only tab, then Cmd+N; original window remains, new tab has no path and zero source bytes |

These are single-document-window journeys. The runner fails if inspection
unexpectedly switches document owners. Find is a real native child viewport.
The fixture is a text document: this lane **does not exercise either live preview
backend** and records backend identity without claiming rendering readiness.

Still to add: two-document Settings focus handoff; minimize/app-switch/restore;
real save dialogs and TeX save-as; restored Typst → pinned TeX builds; native
preview theme/comfy transitions and their composed on-screen appearance.
The existing `scripts/test-native-windows.py` covers a separate native lifecycle
fixture, not those complete editor journeys.

## Evidence and failure policy

Each run leaves `.tiptoptyp/desktop-ui-tests/run-*/` with:

- `result.json`: pass/failure, completed journeys, platform, exact binary version
  and SHA-256, Git revision/diff summary, and initial backend/settings state.
- `events.jsonl`: ordered native actions, observed targets and bounded state
  snapshots. No document contents are included.
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
permissions above. It uploads evidence even on failure. It is not automatically
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
