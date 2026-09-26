# UI test promotion audit

The existing tests exercise useful contracts at several different layers. We are
promoting user interactions into `scripts/test-desktop-ui.py`, retaining the
lower-level tests that isolate geometry, parsing, renderer workers and state
machines. Removing those would lose edge cases and headless CI coverage.

The desktop journeys use native keyboard/mouse or native Accessibility window
actions. They never call editor commands through the inspection socket. Exact
fixture content is compared by byte length/fingerprint; cursor offsets are
Unicode scalar positions. Temporary scratch tabs are cleared/closed at the end
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

Real open/save dialogs (including `.tex` and arbitrary extensions), restart/session
restoration with a pinned TeX target, tab drag reorder, OS file drag/drop,
completion and diagnostic navigation with actual LSP responses, cross-window
clipboard ownership, and preview controls dragging/resizing need dedicated
fixtures. The current journeys must not be cited as coverage for these cases.

## Execution evidence

Each invocation records the exact build, native actions and assertions in
`.tiptoptyp/desktop-ui-tests/run-*/`. A journey that times out, loses foreground,
uses an unavailable tool or fails readiness is a failure. Pure tests, prepared
builds, observer-only runs and manual review acknowledgements do not turn that
failure into a desktop pass. Consult each run's `result.json` for completed
journeys; do not infer execution from this audit table.

On 2026-09-26, `run-ah3gtlxj` passed all 14 journeys together on macOS 14.6.1
ARM64, with Accessibility and native input enabled. The run records its binary
hash and source diff. Formatting, strict all-target Clippy (default and
`desktop-ui-tests`), the full Rust suite, the 15 xtask tests and the 10 Python
driver-contract tests passed. Fresh viewport captures of the toolbar, settings
highlight and preview controls were inspected. These captures verify their
individual viewport appearance, not native preview composition; the desktop
journeys assert native interaction and read-only renderer state.
