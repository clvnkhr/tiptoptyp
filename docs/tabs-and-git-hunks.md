# Document tabs and Git hunk actions (todo 133 / 115)

Tabs occupy the existing title-bar filename area. Click a name to select it,
the path-drawn × (or middle-click the name) to close it, and the eye to choose
the Typst preview source. The open eye with its filled pupil identifies the
preview tab independently of the active editor tab; other tabs have a closed
eye, disabled for non-Typst documents. The active background encloses the name
and both controls. The first tab is selected initially; closing it
selects the first survivor. Overflow scrolls horizontally, and keyboard tab
switches reveal the active tab. Double-click still renames / saves an untitled
document. Reopening the same canonical path selects the existing tab.

Closing the last tab leaves the window and its workspace open. The empty view
keeps Explorer available and offers New document / Open file; document-only
commands are disabled. New/Open reuse that window. Closing a dirty last tab
still follows the normal save/discard/cancel workflow. The window close button
and Close window shortcut continue to close the window itself.

PDF/image tabs have their own viewer in the editor pane when a Typst tab owns
the preview. The Typst output stays on the right; asset page navigation, zoom,
links and loading cannot replace or navigate it. Without a Typst preview,
the asset fills the content area. Returning to a source tab restores the
selected Code/Split/Preview mode; viewing an asset does not change that setting.

Default shortcuts (configurable in Settings):

| Action | macOS | Other platforms |
| --- | --- | --- |
| Close tab | Cmd+W | Ctrl+W |
| Close window | Cmd+Shift+W | Ctrl+Shift+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Stage hunk | Cmd+Option+S | Ctrl+Alt+S |
| Unstage hunk | Cmd+Option+U | Ctrl+Alt+U |
| Revert hunk | Cmd+Option+R | Ctrl+Alt+R |
| Previous / next hunk | Cmd+Option+Up / Down | Ctrl+Alt+Up / Down |

The window-close chord is consumed before the less-specific tab-close chord.
When Settings or a popup has focus, Cmd/Ctrl+W closes that child window instead
of closing its owner's document tab.

## Ownership and performance

Inactive tabs own buffers, saved baselines, undo/redo, cursor state, folding,
scroll identities, and auto-save deadlines—not another `EditorApp`, compiler,
watcher, or Tinymist process. A window still owns one service set. Switching
reattaches the buffer to a monotonic document epoch/revision without replacing
its history, so late hover, formatting, save, and Git results cannot cross tabs.
Git markers are cleared immediately on a switch, before the new scan arrives.

Tinymist retains unsaved open tab documents, including the chosen preview
entry. Session restarts reopen the parked buffers once. Project indexing gets
open-buffer overrides when a debounced indexing request is built, not during
every frame. The native CLI fallback retains its existing limitation: the
preview entry uses unsaved text, while imported files are read from disk.

Auto-save of parked tabs uses the earliest pending deadline; an idle tab
strip neither scans files nor starts background workers. Auto-save verifies
the disk fingerprint and pauses on external edits. Dirty background tabs
participate in window and process close. Cancel retains every buffer, and
changed documents invalidate previous close approvals. Save As cannot
overwrite a file already open in another tab. The preview choice is now
window-local tab state, not the removed workspace-global `preview_files`
setting. Tabs themselves are session-local; this does not add session restore.

## Hunk actions

The existing gutter popup provides Previous, Next, Stage, Unstage, and Revert
buttons with the effective shortcut labels. Navigation wraps through changes
and reveals the source line. Action shortcuts use the selected popup hunk,
or the hunk under the caret when no popup is selected.

Gutter hunks still compare HEAD with the canonical editor buffer, including
unsaved text. Stage/Unstage run as exclusive background operations and update
only the index using a checked patch. They do not save the working file.
Partial staging inside the selected hunk is supported; unrelated staged
hunks survive. A staged edit crossing the selected boundary is rejected
instead of broadening the operation. Conflicted files and changed HEAD
baselines are also rejected. New files, unborn repositories, Unicode/quoted
paths, CRLF, and missing final newlines have regression coverage.

Revert restores the selected HEAD change in the editor as one Undo-able edit;
it does not directly write the saved file or alter the index. Normal auto-save
still applies. In MiTeX mode, canonical changes pass through the checked
projection before committing displayed text; unrepresentable edits fail
without changing the buffer. Ordinary tabs bypass that translation.

## Verification

Deterministic tests cover independent buffer/history/cursor state, preview
selection, named tab controls, dirty multi-tab close/cancel, revoked close
approvals, parked auto-save conflicts, and a 51-tab idle strip without added
service work. Git tests cover partial staging, preservation of unrelated
index/worktree content, unsafe overlap rejection, and projected revert/undo.
The `tabs` and `git-chunk` capture scenes exercise the actual controls; the
maintained gallery includes the updated title strip and File menu.

All required checks passed: formatting, strict all-target Clippy, the full
Rust test suite, and the xtask tests. Strict Clippy and the full suite also
passed with the profiling feature enabled. Navigation wrapping and staging
new executable files have additional focused regression tests.

Fresh viewport captures inspected in both Catppuccin Latte and Mocha:

- `.tiptoptyp/screenshots/tabs-hunks-final/1789587857306-0001-main-tabs.png`
- `.tiptoptyp/screenshots/tabs-hunks-final/1789587857948-0002-main-tabs.png`
- `.tiptoptyp/screenshots/tabs-hunks-final/1789587858048-0003-popup-git-chunk.png`
- `.tiptoptyp/screenshots/tabs-hunks-final/1789587858102-0004-popup-git-chunk.png`

The 68-image maintained gallery was regenerated in one session, reviewed,
and passed `scripts/capture-theme-gallery.sh --validate-latest`. The tab
captures show a dirty second tab while the first tab remains the rendered
preview entry. Hunk controls wrap within the popup without clipping.
Viewport captures do not prove composed native child-window geometry or
cross-platform interaction latency; native preview bounds were not changed.

## Idle performance comparison (2026-09-16)

Apple M2 Max, macOS 14.6.1 (23G93), arm64 Rust 1.96.0. Both binaries used
the optimized `profiling` profile with frame pointers. Same isolated 812-byte
fixture, Catppuccin Latte `main` scene, 2800×1770 framebuffer, eight-second
warmup and eight-second idle measurement; native `sample` at 1 ms. Captures
were inspected to confirm the same workload. No builds, tests, or other
agent profiling sessions ran during a measured workload; the user's existing
app remained open, so this is not an isolated-machine benchmark.

Commands:

```sh
cargo xtask profile --scenario main --warmup 8 --seconds 8 --binary /tmp/tiptoptyp-tabs-baseline
cargo xtask profile --scenario main --warmup 8 --seconds 8
cargo xtask profile --scenario main --warmup 8 --seconds 8 --skip-build
```

The preserved pre-tab binary has SHA-256
`808bd595d7ed9ce27d64990010d0445fe965274d84ef858f04e0f7ed7cd49bfc`;
the final binary has SHA-256
`40a460485199539f7d68b36ae27c02badd807f22a479579b5855c9601a81a136`.
The baseline was copied from the optimized, frame-pointer profiling build
before this task; its skipped-build metadata alone does not prove provenance.
Fixture hash for every run:
`3625fd0a2b8656e8569d3a9df61ac867a2454fd38df1baea54af9b3bf8a38289`.

Artifacts are retained under `.tiptoptyp/profiles/`:

| Run directory | Version | Editor passes | Editor UI total / mean | Shell UI total | Supplementary process CPU delta |
| --- | --- | ---: | ---: | ---: | ---: |
| `1789586323370-88487-main-0` | Before | 121 | 73.35 ms / 606 µs | 84.43 ms | 0.21 s |
| `1789588471209-18686-main-0` | After | 70 | 67.76 ms / 968 µs | 74.14 ms | 0.17 s |
| `1789588491216-19068-main-0` | Before repeat | 105 | 71.17 ms / 678 µs | 80.28 ms | 0.19 s |
| `1789588510788-14334-main-0` | After repeat | 122 | 69.13 ms / 567 µs | 82.79 ms | 0.20 s |

Native main-thread samples were predominantly event waits (about 98% in
the final three runs). Seven or eight existing worker jobs ran per interval;
no highlighting rebuilds were recorded. This shows no material idle CPU
increase in these local runs, not faster interaction or a general speedup.
Pass counts and per-pass means vary with event timing. UI totals are inclusive
wall time, not CPU, and must not be added together. Supplementary CPU deltas
bracket sampler attachment/analysis rather than exactly eight seconds.

An exploratory pre-shortcut-fix run is also retained at
`1789588033141-5754-main-0` (27 passes, 34.83 ms editor UI); it is not the
final-binary comparison. Many-tab behavior is guarded structurally by the
51-tab no-extra-service test. Active switching, large-document auto-save,
large-repository Git latency, cold startup, and cross-platform performance
were not benchmarked here. Tabs still require memory for each open buffer,
and rendering the tab strip scales with the number of tabs.

## Empty workspace and asset panes (todo 178)

The empty workspace invalidates pending document/asset results, stops Tinymist,
pauses compilation and clears document/index deadlines. It retains the workspace
and its normal Explorer/Git refreshes; it does not create an untitled tab until
New is selected or a file successfully opens.

The asset pane uses the existing background asset loader and a separate raster
controller, not another compiler, Tinymist session or worker. Only the active
asset's pages are retained; revisiting an asset reloads it. Ready Typst pages
survive asset switches without requesting another compile. The shared renderer
also keeps both horizontal page margins inside a fitted viewport; zoomed pages
can still scroll horizontally. No per-frame disk I/O or new repaint timer was
introduced. A visible second raster naturally adds texture memory and painting
work; this is not a claim that viewing two documents costs the same as one.

Regression tests cover last-tab close without a window-close command, New/Open
and failed opens in the empty workspace, disabled document commands, absence of
empty-document service work, independent asset/PDF results and page destinations,
late-result rejection, pane routing in all source view modes, and fitted-page
margin geometry. A semantic UI test closes the actual tab control and creates
a new document through the empty view.

All required checks passed: formatting, strict all-target Clippy, full Rust
tests (765 application tests passed, eight opt-in tests ignored), and 13 xtask
tests. The maintained 68-image gallery was freshly regenerated in one session
and passed `--validate-latest`; the changed light/dark main layouts and File
menus were inspected. Six final viewport captures were inspected under
`.tiptoptyp/screenshots/workspace-assets-final/`:

- `1789590642326-0001-main-tabs-image.png` (Latte)
- `1789590642877-0002-main-tabs-image.png` (Mocha)
- `1789590643590-0003-main-tabs-pdf.png` (Latte)
- `1789590644298-0004-main-tabs-pdf.png` (Mocha)
- `1789590644377-0005-main-empty-workspace.png` (Latte)
- `1789590644439-0006-main-empty-workspace.png` (Mocha)

These show the selected asset on the left, the pinned Typst output on the
right, uncut horizontal page margins, and the empty workspace retaining its
Explorer and New/Open controls. QA captures use the raster preview: native
child-view bounds code was unchanged, and composed native-webview geometry
was not visually verified. `TIPTOPTYP_UI_TRACE=1` was enabled for the targeted
batch, but no native `ui.preview.bounds` lines are emitted on that raster path.

### Idle overhead check

Same Apple M2 Max/macOS 14.6.1/Rust 1.96.0 environment and optimized
frame-pointer profiling profile as above. The `main` workload used the same
812-byte fixture (SHA-256 `3625fd0a2b8656e8569d3a9df61ac867a2454fd38df1baea54af9b3bf8a38289`),
Catppuccin Latte, 2800×1770 framebuffer, eight-second warmup and eight-second
idle interval with 1 ms native sampling. No builds, tests or other agent
profiling runs overlapped either measurement. The user's app remained open;
desktop focus was not controlled, and the initial after capture shows an
editor focus outline absent from the baseline. Both captures were inspected.

```sh
cargo xtask profile --scenario main --warmup 8 --seconds 8 --binary /tmp/tiptoptyp-empty-workspace-baseline
cargo xtask profile --scenario main --warmup 8 --seconds 8
```

The preserved baseline came from the prior optimized profiling build:
`40a460485199539f7d68b36ae27c02badd807f22a479579b5855c9601a81a136`.
The rebuilt after binary is
`648c419326e9607ff8c8118175ace94e8094bd1dddd43911ddbe85743c509c47`.
Metadata and samples remain in `.tiptoptyp/profiles/`:

| Run | Editor passes | Editor UI total / mean | Shell UI total | Supplementary process CPU delta |
| --- | ---: | ---: | ---: | ---: |
| Before `1789590481727-27267-main-0` | 41 | 46.59 ms / 1136 µs | 50.23 ms | 0.14 s |
| After `1789590728629-29161-main-0` | 29 | 32.65 ms / 1126 µs | 34.64 ms | 0.08 s |

Each run recorded eight worker jobs, no highlighting rebuilds, and over 96%
of main-thread samples in the event wait. These runs did not show extra idle
work, but event/focus variation prevents treating the lower totals as a
speedup. Inclusive UI wall-time totals overlap; they are not additive CPU
time. Process CPU deltas include sampler setup/analysis. This checks ordinary
single-document overhead, not active asset loading, two-pane scrolling,
large-PDF memory use or cross-platform performance. Empty-workspace service
shutdown and unchanged-preview compile suppression have deterministic tests.

The earlier baseline attempt `1789589324799-20133-main-0` exited before its
measurement interval completed. Its artifacts are retained but excluded from
the comparison; the successful retry above is the baseline.

## Tab controls and editor gutter (todo 177 / 179)

The tab eye reuses the Explorer's rounded vector geometry; its pupil is a
filled dot. The closed-eye lid and lashes and the close X are paths, with
shared vertical centers independent of font metrics or bitmap scaling.
Preview and close remain separate named keyboard/accessibility buttons;
clicking the eye does not activate that tab's editor. The selected background
is painted around the entire tab, including both controls.

Line numbers now use the configured editor font and its normal editor weight,
not the smaller annotation font. Their glyph baselines align with the first
visual row of each logical source line, including taller fallback-font rows.
Sticky headers use exactly the same placement helper. Hidden folded rows
remain unpainted. The gap after a number is three points, with a separate
eight-point fold lane before it and the unchanged Git lane. Gutter sizing
uses ten cached digit metrics and the digit count, so proportional editor
fonts cannot make a wide number overlap the folding arrow.

These are bounded paint/layout changes with no new worker, timer, parsing
pass or disk I/O. Number text still uses egui's text-layout cache and only
visible numbers are painted. No material performance impact is expected;
no unrelated benchmark was run. Geometry tests cover both controls inside
the highlight, icon centerlines, normal/tall/blank text baselines, proportional
digits and lane spacing. Semantic tests cover the named tab actions. The
[keyboard coverage notes](keyboard-shortcuts.md) document todo 176.

Verification: formatting, strict all-target Clippy, all Rust suites (773
application tests passed, eight opt-in tests ignored), and 13 xtask tests pass.
The maintained gallery was regenerated in one session and all 68 PNGs passed
`--validate-latest`; light/dark main and File-menu captures were inspected.
Six additional fresh viewport framebuffers were inspected under
`.tiptoptyp/screenshots/tabs-shortcuts-gutter/`:

- `1789592462024-0001-main-tabs.png` and `1789592462759-0002-main-tabs.png`
- `1789592462958-0003-main-folding.png` and `1789592463092-0004-main-folding.png`
- `1789592463188-0005-main-sticky-context.png` and `1789592463292-0006-main-sticky-context.png`

Each pair is Latte then Mocha. The targeted batch wrote all six images but
remained alive on closing its dirty QA fixture; its owned process was then
terminated without saving. The user's running app was left alone. The gallery
session itself exited normally. These are viewport observations, not native
composed-desktop verification; native preview bounds were not changed.
