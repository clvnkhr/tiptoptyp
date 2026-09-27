# UI test promotion audit

The existing tests exercise useful contracts at several different layers. We are
promoting user interactions into `scripts/test-desktop-ui.py`, retaining the
lower-level tests that isolate geometry, parsing, renderer workers and state
machines. Removing those would lose edge cases and headless CI coverage.

The desktop journeys use native keyboard/mouse or native Accessibility window
actions. They never call editor commands through the inspection socket. Exact
fixture content is compared by byte length/fingerprint; cursor offsets are
Unicode scalar positions. Temporary scratch tabs are closed at the end
of each journey, so the combined run also detects leaked state between journeys.

## Promoted interaction contracts

These mappings describe the portions exercised through the complete application,
not a claim that one desktop journey replaces every assertion of its source test.
The retained source tests remain the fast regression layer.

| Existing test / source | Desktop journey and real-input assertion |
| --- | --- |
| `app/tests.rs::live_editor_redo_restores_the_cursor_after_inserted_or_replaced_text` | `editing`: insert, undo, move caret, redo, type again; Unicode replacement and caret |
| `app/tests.rs::toggle_line_comments_handles_selected_lines_and_round_trips` | `editing`: comment/uncomment a real multiline selection |
| `app/command_tests.rs::command_select_all_obeys_source_or_find_focus` | `search`: Cmd+A replaces the query without changing source |
| `app/command_tests.rs::deferred_find_select_all_is_delivered_once_to_query_without_editing_source` | `search`: query replacement through the native child window |
| `app/command_tests.rs::find_shortcuts_refocus_resume_and_only_close_when_focused` | `find`, `search`: focused Cmd+F toggle, Escape, repeated Replace/close clicks |
| `app/tests.rs::native_find_child_routes_query_and_escape_to_its_owner` | `search`: real query input and Escape return to editor |
| `app/tests.rs::native_find_edit_commands_do_not_target_the_still_focused_source_widget` | `search`: source fingerprint remains unchanged while query is edited |
| Find/Replace widget and search-session contracts | `search`: Enter/Shift+Enter, case/regex toggles, replace one/all and one-step undo |
| `app/tabs_tests.rs::tabs_preserve_unsaved_sources_undo_cursor_and_first_preview` | `tabs`: previous/next shortcuts preserve each buffer and caret, pinned preview survives |
| `app/tabs_tests.rs::new_tabs_preserve_the_designated_preview_identity` | `tabs`: scratch tabs do not replace the original preview |
| `app/tabs_tests.rs::last_tab_closes_to_an_empty_workspace_and_new_reuses_the_window` | `empty`: original window survives, Cmd+N is empty |
| `app/tabs_tests.rs::empty_workspace_new_button_and_close_tab_workflow_are_reusable` | `empty`: keyboard path through close-last/new workflow (button-specific semantics retained) |
| Plain launch without a saved history target | `startup_preview`: no-argument launch starts with the synthetic editing buffer, then opening the first `.typ` rebinds the preview target to that file |
| Native file dialog routing | `dialogs`: Open selects a real `.tex` file through the macOS panel; Save As accepts an arbitrary extension |
| Tab strip reorder behavior | `tab_drag`: native drag reorders scratch tabs and each tab remains selectable |
| `app/tabs_tests.rs::closing_checks_dirty_background_tabs_without_discarding_on_cancel` | `closing`: active dirty tab cancellation preserves content; background-tab cases retained |
| `app/tabs_tests.rs::all_dirty_tabs_must_be_approved_and_later_edits_revoke_window_close` | `closing`: repeated close asks again; Cancel/Escape preserve text; Discard closes only requested tab |
| `app/tests.rs::folding_nested_typst_controls_stay_on_headers_after_click` | `folding`: three real shortcut cycles plus gutter collapse/expand clicks preserve header identities and source; exact gutter geometry remains a fast test |
| `app/tests.rs::explorer_reopen_restores_the_last_open_width` | `layout`: real explorer hide/reopen path; exact resize geometry remains lower-level |
| `app/tests.rs::explorer_maximize_hides_siblings_and_restores_collapsed_states_and_sizes` | `layout`: Tags/Files maximize/restore clicks; detailed sibling bounds remain lower-level |
| `app/terminal_panel.rs::panel_maximize_restores_large_resized_height_and_terminal_shortcut_focus` | `layout`: maximize button, restore shortcut, measured original height |
| `app/terminal_panel.rs::problems_content_growth_does_not_resize_the_bottom_panel` | `panels`: repeated real-frame height checks around reopen/switch |
| `app/terminal_panel.rs::switching_away_from_a_focused_terminal_releases_focus_without_locking` | `panels`: repeated native Terminal/Problems clicks and continued response |
| `app/terminal_panel.rs::panel_tabs_switch_semantically_and_close_without_mutating_source` | `panels`, `layout`: Problems/Terminal/Activity switches and close, unchanged source |
| Editor settings shortcut contracts | `layout`: line wrapping, line numbers and sticky-context toggles restore their original values |
| `app/tests.rs::settings_is_root_owned_and_secondary_requests_never_create_a_viewport` | `focus`: one Settings window reached from alternating document owners |
| Native lifecycle fixture focus/close contracts | `focus`: Settings close returns to most recent owner, minimize/restore, app switch, native close |
| `app/preview_controls/e2e.rs::e2e_pdf_controls_navigate_search_edit_query_and_restore_history` | `controls`: real PDFium and Tinymist controls, outline/back/forward/pages/zoom/fit/search/minimize/pop-out/close |
| Preview control persistence | `controls`: native drag moves the compact toolbar and its position remains stable while the outline opens |
| `scripts/test-preview-e2e.cjs` navigation and history scenarios | `controls`: same user-facing actions through the embedded viewer and native controls, rather than JS action calls |
| `app/tests.rs::native_preview_load_invalidates_queued_palette_even_when_preview_is_hidden` | `preview`: renderer switches followed by theme/comfy transitions and actual applied state |
| `scripts/test-native-preview-palette.py` / `test-preview-palette.swift` | `preview`: production app/backend identity and live DOM palette; optional composed-window review |

## Bugs exposed while promoting coverage

- Find's native child omitted several search shortcuts, and macOS delivered
  modified C/X as Copy/Cut events. Both routes now honor the explicit bindings.
- PDFium reported the first intersecting page even when only a sliver remained
  visible. At the document end this made Previous skip a page; the displayed
  page now follows the largest visible portion, with a separate scroll anchor.
- Preview-controls measurements depended on the preceding native viewport
  height. Content measurement now has an independent vertical constraint.
- Cmd+M/native restore left egui’s minimized flag stale; once child windows
  closed, the restored document stopped rendering. Both native renderers now
  clear that stale flag when native focus confirms restoration. The focus
  journey requires fresh editor frames after its last child closes.
- Tinymist search at column zero reached the source-span boundary but did not
  navigate. Search now targets just after the first matched Unicode scalar;
  the real renderer journey searches for text at the start of a later line.
- Expanded folding markers used pre-edit offsets for one frame. All markers now
  remap in the same layout pass; unchanged galleys reuse their cached projection.
- Clicking a toolbar control retained its native tooltip over the newly opened
  preview popup, intercepting Outline clicks. Control activation now retires
  its tooltip and suppresses reopening until the pointer leaves. The controls
  journey deliberately waits for the tooltip before opening the popup.
- Shared fixed modeless popups now suspend while another application is active
  and return without discarding their open state.
- A plain launch previously designated the synthetic `untitled.typ` buffer as
  the preview owner, so the first file opened from a no-history launch did not
  replace the startup preview. The launch now leaves that fallback unbound;
  the first opened source becomes the preview owner, covered by the native
  `startup_preview` journey.
- Native child views and settings can leave the process inactive while the
  document reports an accessibility focus. The driver now raises the owning
  document before input and the app explicitly activates itself for user-focus
  requests; scratch journeys assert this precondition.

Additional requested journeys cover settings-search highlighting and loading the
Tinymist frontend while its pane is hidden. Native activation guards remain in
place: creating a hidden viewer must not activate a background application.

## What should remain fast and deterministic

- Geometry and hit regions: wrapping, clipping, panel bounds, popup placement,
  scale conversions, alpha channels, icon contours, native child positioning.
- Document/session behavior: stale response rejection, revision ownership,
  parked saves, atomic writes, diagnostics retention, undo grouping and invalid
  completion/formatting payloads.
- Native host fixture internals: style masks, retained ownership, lifecycle
  cancellation counts and alpha-capable GL configuration. The complete editor
  journeys cover user outcomes, not every low-level host flag.
- Renderer integration: malformed PDFs, worker replacement, allocation/cache
  invariants, search after artifact replacement, Tinymist protocol events and
  browser-only navigation details. Keep the standalone viewer/worker tests.
- Gallery/image contracts: naming, fresh files, dimensions and decoder checks.
  Native actions and DOM state cannot replace pixel evidence.

## Runtime cost

The inspection endpoint, target collection and observation redraws exist only in
opt-in desktop-test builds. Normal idle windows gain no polling loop. Expanded
fold markers reuse the existing galley cache between edits; remapping occurs in
the edit's layout pass. Settings highlighting schedules one delayed repaint.
Hidden preview loading deliberately moves frontend startup earlier; the journey
asserts that revealing it reuses the same renderer generation. These are bounded
work/reuse checks, not a wall-time performance benchmark or speed claim.

## Remaining candidates

Restart/session restoration with a pinned TeX target, OS file drag/drop,
completion and diagnostic navigation backed by an external LSP process,
cross-window clipboard ownership, and preview-control resizing still need
dedicated fixtures. The current completion and diagnostics journeys exercise
the app's native response paths (MiTeX completion and compiler diagnostics),
but do not claim external TexLab or remote-toolchain coverage.

## Execution evidence

Each invocation records the exact build, native actions and assertions in
`.tiptoptyp/desktop-ui-tests/run-*/`. A journey that times out, loses foreground,
uses an unavailable tool or fails readiness is a failure. Pure tests, prepared
builds, observer-only runs and manual review acknowledgements do not turn that
failure into a desktop pass. Consult each run's `result.json` for completed
journeys; do not infer execution from this audit table.

On 2026-09-26, `run-kqidr5wg` passed all 18 aggregate journeys together on
macOS 14.6.1 ARM64, with Accessibility and native input enabled. The separate
`run-xq4hbovu` launch passed the no-history `startup_preview` journey. Each run
records the exact binary hash, source diff, native actions and read-only state.
Visual captures remain evidence for individual viewport surfaces, not native
preview composition; the desktop journeys assert native interaction and
read-only renderer state.

## 27 September: diagnostics and Settings

- Editing retains each provider's previous diagnostics until a successful
  replacement, including an empty result. Regression tests cover writing jobs,
  TeX edits/reconfiguration/disable, and projected miTeX source. Infrastructure
  failures do not masquerade as empty diagnostic responses.
- JSON Settings tests cover invalid syntax, unknown fields, invalid ranges and
  enum choices, actual saved changes, and refreshing an unmodified draft after
  form changes. Invalid choices report the field and allowed values. Provider
  configuration and command syntax have focused tests; command validation is
  cached by draft to avoid idle filesystem checks.
- Completion no longer changes selection on hover or repeatedly scrolls to the
  selection. The compact menu removes the incomplete-results footer. Semantic
  coverage checks hover, click acceptance and outside dismissal; the native
  journey verifies a real completion response and Escape dismissal.
- `run-5eu1hwrw` reproduced the original focused-Find accessibility-resize failure
  (`-25200`) with forwarding disabled. `run-9ipln4c2` passed repeated popup
  open/close, two owner resizes, owner minimization/restoration, and Replace sizing.
- `run-rhn7_n94` passed native Settings search/typing isolation, invalid JSON with
  Save disabled, reload/save, independent maximize/restore, Cmd+F and Cmd+W.
  `run-ix7uzfka` passed completion; `run-tqlxe122` passed multi-window focus and
  minimize/app-switch restoration. These are macOS 14.6.1 ARM64 interaction runs,
  not assertions about a user's personal Hammerspoon scripts.
- Fresh inspected framebuffers under `.tiptoptyp/screenshots/agent-review/`:
  `1790518100210-0001-main-unicode-completion.png` and
  `1790518105657-0001-settings-settings-window.png`. These verify the compact
  menu and title-bar JSON button, not composed native preview geometry.

The changes add no idle repaint loop or concurrent checker. Language detection
runs in the existing debounced writing worker, with its existing document-size
limit. Retention keeps one bounded last result rather than accumulating results.
This is an architectural performance review, not a latency benchmark.

Remaining limitations: Auto language selection reads literal top-level Typst
settings, not computed/imported language values or locally mixed-language prose.
External tool configuration keys are checked by the receiving tool. An arbitrary
Hammerspoon helper may use a different API from the AX size/minimize paths tested.

The installed Hammerspoon configuration contained no window-resize bindings.
Rectangle was running and owned Cmd+Option+Return (Almost Maximize) and
Cmd+Option+, (bottom-left sixth). The optional `rectangle` journey passed both
actual configured shortcuts in `run-lqn4xh24`, retaining Find focus and a working
close button. These settings were read, not changed. Rectangle's
[accessibility implementation](https://github.com/rxhanson/Rectangle/blob/main/Rectangle/AccessibilityElement.swift)
uses the size/position attributes forwarded by the popup bridge.

The gallery exposed a separate capture issue: an inactive application correctly
hid its modeless preview controls, so a queued framebuffer capture waited forever.
The 69-second and 264-second runs both stopped after seven images; validating
existing files alone was not accepted as a fresh-gallery pass. Automated
close-after-capture requests now keep only their pending target paintable during
app inactivity. Explicitly hidden targets remain hidden, and ordinary popup
hide-on-app-switch behavior is unchanged. Deterministic visibility tests cover
all three cases; this exception does not count as native interaction evidence.

After the capture fix, the complete 24-image release gallery passed in one
session and `--validate-latest` passed. The refreshed Settings and preview-controls
PNGs were inspected. Native `controls` also passed in `run-k674s3vy`, including
ordinary app-switch visibility. Final checks: formatting, Clippy with warnings
as errors, 1,312 Rust tests (32 intentionally ignored), 15 xtask tests, and 10
Python harness contract tests passed.

### 2026-09-27 new todos

- Preview controls: screen-coordinate drag anchor replaces moving-window-relative deltas; the whole spare header is draggable and minimize uses a minus icon. `controls` now requires a 48×24 drag to produce the same window movement within three pixels, on both PDFium and Tinymist. Native journey passed in `.tiptoptyp/desktop-ui-tests/run-ml0pqabi`.
- Changing preview entry retires compiler errors from the previous entry. Switching editor tabs synchronously retires previous editor-provider results and admits new checks. TeX → Typst reinitializes the retained Tinymist service without discarding the preview surface, avoiding dependence on a later keystroke.
- Snippet input has an editor-level transaction regression covering mirrored replacement and one-step undo/redo. The `snippets` native journey configures a custom snippet through Settings JSON, accepts it, types, and exercises undo/redo.
- Native snippet runs were unavailable after macOS locked the desktop (`CGSSessionScreenIsLocked = 1`), despite granted input/Accessibility permissions. The native driver's preflight now reports that condition. Startup activation failures from those runs are not interaction passes.

Deterministic validation: 1,322 Rust tests passed (32 intentionally ignored),
15 xtask tests, strict all-target Clippy, formatting, and 10 Python harness
contract tests. The separately invoked pinned TexLab/Badness/tex-fmt test also
passes with a projected miTeX source and verifies that formatting preserves
surrounding Typst prose. Offline inference recognizes the dagger fixture.
The model preparation script verified eight seeded tensors against upstream
ONNX Runtime with zero output difference.

Performance scope: no handwriting model initialization occurs at startup; it
loads once on a worker on the first drawing. Jobs are bounded to one per window
and 4,096 points, and generation checks reject stale results. Normal completion
requests bypass snippet processing when the custom list is empty. Linked fields
scan the document only on an actual edit during a snippet session. GUI latency
and cold/warm release inference timings remain unmeasured; a locked desktop is
not a valid interactive performance baseline.

Fresh framebuffers inspected: `1790524802860-0001-main-draw-symbol.png` and
`1790524826006-0001-preview-controls.png` under `.tiptoptyp/screenshots/agent-review`.
The 25-image gallery was regenerated in one release session and validated.
These establish canvas/control layout, not composed native desktop placement.
The locked-session controls trace produced no `ui.preview.bounds` records;
native drag movement evidence remains the earlier successful controls journey.

The release executable is 52,946,272 bytes on macOS ARM64. The previously
available release was 33,297,552 bytes (a different dirty revision); the roughly
19.6 MB increase is indicative, not a controlled size benchmark. The embedded
model plus Rust ONNX runtime are the material new cost.
