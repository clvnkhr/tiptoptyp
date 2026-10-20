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

### 2026-09-27 drawing panel and full Detexify coverage

The original model has no calligraphic Latin classes. The first calligraphy-only
prototype found 122/128 held-out letters in its top four; it was expanded at the
user's request to all 39,494 accepted Detexify samples and 1,123 definitions.
The complete dataset is bundled, with a bounded two-stage matcher and a separate
calligraphic shortlist. There is no background download or extra dependency.

The canvas now fills its section. An independent logical paper preserves ink
through wide/tall resizes, while displayed strokes stay four logical pixels wide.
Predictions are non-selectable text painted over subdued ink. A failing semantic
click reproduction caught the underlying drag surface clearing predictions on
mouse-down; delayed drag recognition fixes that, and clicking still inserts.

Native `drawing` passed in `run-fmmji4fy` after the user unlocked macOS: genuine
three-stroke input, live predictions, actual native size change with identical
retained stroke coordinates, and source insertion. `run-vl18w6zg` additionally
selected the live `\mathcal{A}` prediction and verified Typst `cal(A)` insertion.
No fixture or inference result was injected. These checks are separate from
framebuffer captures and held-out classifier accuracy.

Final validation: 1,327 regular Rust tests, 15 xtask tests, strict all-target
Clippy (also with the desktop UI feature), formatting, and 10 Python harness
contracts pass. The final native run `run-p8wpw4o_` passes with the full dataset,
separate calligraphic shortlist and point-stroke support. The complete 25-image
release gallery was regenerated and validated. Fresh light/dark framebuffers
`1790534349181-0001-main-draw-symbol.png` and
`1790534353575-0001-main-draw-symbol.png` under
`.tiptoptyp/screenshots/drawing-final` were inspected for layout, contrast and
clipping; these are framebuffer evidence, not native composition evidence.

The optimized held-out evaluation finds 342/421 expected commands in the full
set's top eight and 122/128 calligraphic letters in the dedicated top four.
See the dataset README for the non-writer-disjoint split and reproduction.

## 2026-09-27 — Typst handwriting mapping audit

A failing regression demonstrated that `\\mathscr{A}` had no Typst output.
Expanded Detexify mappings from 684 to 1,120 of 1,123 definitions. Added script
styles, upright Greek, IPA, punctuation and symbol aliases; quoted syntax-sensitive
characters and preserved negation marks. Three entries remain unmapped because
their glyph has no settled text equivalent. Original sample and holdout binaries are byte-for-byte
unchanged; mapping lookup/recognition scheduling has no material performance
impact and adds no runtime dependency.

Validation: 1,330 regular Rust tests, 15 xtask tests, strict all-target Clippy and
formatting pass. The opt-in Typst CLI test additionally compiles every mapped
expression with Typst 0.15.1. Semantic tests click new script/IPA/punctuation
predictions and verify insertion in both Typst and TeX. No layout or painting
contract changed, so no framebuffer/gallery regeneration was needed or claimed.

`python3 scripts/test-desktop-ui.py --journey drawing` was attempted; its preflight
stopped before interaction because macOS was locked (`desktop:false, locked:true`;
Accessibility and input permission were both true). Evidence:
`.tiptoptyp/desktop-ui-tests/run-mqmm9ii3/result.json`. This is an unavailable native
check, not a pass; prior-turn native results do not establish this build's result.

## 2026-09-28 — One handwriting engine and language-specific insertion

A failing regression reproduced `$` being offered as the quoted Typst string
`"$"`. The output resolver now prefers escaped ASCII punctuation, a full verified
`#sym.name`, then unquoted Unicode. A Settings preference chooses Unicode instead
of names without exposing raw delimiters. The shared name catalogue is exported
from bundled Typst 0.15.1 and every alias is compiler-verified; deprecated aliases
are excluded. An additional compiler check validates all ASCII punctuation
escapes in both markup and math. TeX retains its own commands and package hints.

Settings selects exactly one recognizer for either document language. Detexify
is the default; Detypify is optional. Both assets remain bundled for offline
switching (4.43 MiB Detypify and 5.03 MiB Detexify; runtime code excluded).
The forced calligraphic list is removed. Baseline work dispatched both engines
and returned up to 20 candidates; selection dispatches one engine and returns
at most eight ranked candidates. Engine switches preserve the ink, invalidate
old generations and wait for the current worker before launching another.
No timing speedup is claimed, no idle repaint loop is added, and output-format
changes do not trigger recognition.

The panel uses normal-darkness ink and compact, right-aligned command rows with
opaque backgrounds. It removes the redundant drawing prompt and result-group
headings. Semantic UI tests cover row order/alignment at two panel sizes,
language-specific click insertion, Clear, settings selection and persistence.
State tests cover engine-exclusive results and rejection of an in-flight result
after switching engines. Invalid JSON engine options name the field and choices.

The native drawing journey passes in `run-mpzyoija`: real three-stroke drawing,
recognition, native resize with retained ink, ranked-result insertion, a Settings
switch to Detypify and Unicode output, insertion from that engine, and Clear.
This uses live inference and native clicks, not injected drawing/model results.
The build was `a23804c2a007-dirty.1790586364` and the fixture binary SHA-256 was
`1a8791cd68be60c40592c60e1b7e2bb787492cbeae616c0533d70b14df7894fc`.

Required validation: 1,334 regular Rust tests and 15 xtask tests pass, alongside
formatting and strict Clippy. Native input evidence is distinct from framebuffer
visual evidence; no composed native-preview claim is made by this audit.

Fresh optimized viewport captures were inspected for both `test.typ` and
`manual-tests/tex-preview.tex`, under `.tiptoptyp/screenshots/drawing-commands`
and `drawing-commands-tex`. Both show dark ink, right-aligned ranked rows and
language-appropriate commands without redundant group labels. The maintained
25-image gallery was regenerated in one session and `--validate-latest` passes;
its drawing scene was also inspected. These are viewport framebuffer captures,
not proof of native child-view composition.

## 2026-09-28 — handwriting row refinements

Result rows now show monospace insertion text followed by the glyph on the right;
Unicode-only output shows only the glyph, following the user’s revised preference. Long labels elide from the left, retaining the rightmost symbol and full insertion tooltip. Backgrounds use 85%
alpha. Math-mode Typst names are the default, with a Settings toggle for full
`#sym` names. All 1,113 catalogue names compile inside Typst math (some existing
aliases emit deprecation warnings). Editing a stroke retains published predictions
until a new generation finishes; Clear still flushes them immediately.

A semantic hover regression failed with a two-pixel row-width change. egui's
button styling derives padding from the state's border width, so overriding only
the drawn border was insufficient. The result overlay now fixes border/expansion
across states and avoids animated scrollbar space. The regression passes at two
panel sizes in both languages; additional tests cover held-stroke predictions,
math/markup insertion and Unicode captions. Recognition work remains bounded and
these presentation preferences do not trigger inference; no timing speedup is
claimed.

Native journey `run-cd3wg77m` failed before drawing when Cmd+N did not create a
scratch tab; it is not interaction coverage. On the corrected build, the complete
native drawing journey passes as `run-0lxl5fsv`, build
`2b856f4e5c07-dirty.1790588295`, SHA-256
`10a40254e08e90d538a2d5f6ac68add9a85ed1ba8ac24ae5d544bc90b729bdd4`.

Final deterministic validation passes: 1,337 Rust tests, 15 xtask tests, formatting
and strict Clippy. A focused left-elision test covers Unicode-safe suffixes and
retaining the final symbol. The revised Unicode-only caption is glyph-only.

Fresh final viewport capture inspected:
`.tiptoptyp/screenshots/drawing-left-elision/1790588987290-0001-main-draw-symbol.png`.
The maintained gallery was regenerated in one session; all 25 PNGs validate and
the drawing scene was inspected. These captures verify the result-row appearance,
not native preview composition. Truncation and hover geometry are covered by
focused deterministic tests.

## 2026-09-28 — Settings code editors and compact explorer controls

Settings JSON, binary environment/configuration, templates, and snippets now share
cached syntax highlighting and the main editor's line-number baseline and wrapped
line mapping. JSON rainbow brackets skip quoted/escaped delimiters. Cmd+F stays in
JSON; typing preserves query focus, and Enter/Next selects the source match.
The native snippet journey exposed Enter losing focus before selection; its fix
is covered by a semantic typing regression and the real JSON-search/edit/save path.
The journey now replaces the snippets property in the complete settings document,
rather than attempting to save an incomplete settings object. Clipboard input is
acknowledged before the fixture restores the previous clipboard.

Templates are persisted editable Typst/TeX sources. Examples and picker previews
are read-only highlighted buffers. TexLab and Badness suppression lists are
provider-specific. Tool help includes concrete arguments/environment examples
and documentation links routed through the app's external-link action. Comfy
background and foreground are independent; the existing shortcut toggles both.

Contents rows reserve the measured line-number width instead of a fixed column.
Git bulk actions share the Changes row and have one detailed tooltip each.
Drawing has icon Clear/Undo/Redo controls, capped at 32 history entries. Both
recognizers return up to 16 candidates using their existing bounded search.
Typst refuses multi-character stand-ins, and bundled Noto Sans covers Latin/IPA
characters including ʟ. Deterministic coverage includes provider isolation,
JSON enums/roundtrips, links, bracket/string handling, history, and Git row geometry.

Performance: syntax/theme results remain cached; gutters share existing linear
row mapping and paint only visible rows. No idle polling or new workers were
introduced. Returning 16 predictions does not expand Detexify's 64-class search.
No timing benchmark is claimed for these changes.

Verification: formatting and all-target clippy with warnings denied passed;
`cargo test --no-fail-fast` passed 1,344 tests, and xtask passed 15 tests.
Final tests ran without overlapping builds after an earlier build-identity
collision exposed shared executable replacement between test runners.

Native macOS journeys passed: drawing (`run-abo3n47a`), window focus
(`run-1biiefns`), snippets through JSON search/edit/save (`run-yxl_1lqi`),
PDFium/Tinymist theme/comfy transitions (`run-lxz58efk`), and Settings search,
JSON validation, typing, maximize/restore and close (`run-hxlefnja`). Evidence
lives in `.tiptoptyp/desktop-ui-tests/`. The click driver permits an older
position only for a non-clicking pointer move, then requires fresh, settled
geometry before posting the click; idle windows no longer fail prematurely.

All 25 maintained gallery images were regenerated and validated. Fresh Settings
JSON capture `run-qy9r1kmm/1790604160580-0003-settings.png` was inspected for
syntax/rainbow colors and line-number alignment. Git capture
`.tiptoptyp/screenshots/agent-review/settings-explorer/1790604281652-0001-main-git-panel.png`
was inspected for the single-row bulk actions. Updated drawing, main Contents,
and Settings gallery frames were also inspected. Framebuffers establish these
egui layouts, not native child-view desktop composition.


## 2026-09-29 — Folding caret, selection wrapping and preview focus

The native folding regression first failed in `run-sfpt7h41`: clicking Collapse
replaced a visible caret elsewhere in the document with the fold's start. A
subsequent reproduction exposed TextEdit moving the caret on mouse-down before
the fold's mouse-up handler. Preserve the pre-press selection; only endpoints
inside the newly hidden range move to the visible end of the header. Expansion
preserves the selection. A brief existing editor attention highlight identifies
the caret. The final native folding run `run-1fsop79b` passed a short fixture and
a 290-line document with a 180-line fold, including distant/hidden carets,
source preservation and scroll stability. Deterministic coverage includes
forward/backward selections and folds ending at EOF.

Typing a delimiter over selected Unicode text previously produced an empty pair;
`live_editor_surrounds_selected_unicode_text` established that failure before the
fix. The TextBuffer adapter now retains the deleted selection only for explicit
surround typing and inserts the pair as one history edit. Existing default-on
auto-close settings control it. Typst, TeX and plain-text editor tests cover all
requested delimiters and undo/redo; disabled pairing, paste, ordinary text and
mixed deletion/typing batches remain ordinary replacement. Native
`selection_wrap` passed in `run-80wsbuud`.

Both previews now display a focus outline and a compact “Preview focused” badge;
Escape returns to the source. Tinymist publishes focus changes and ignores its
keyboard shortcuts without document focus. Native construction keeps keyboard
ownership with the parent; hiding/discarding a focused native viewer releases
that focus. Code-only mode explicitly focuses the editor. That last transition
failed natively in `run-f5l4ti40`: the editor remained unfocused after hiding the
PDFium preview and swallowed typing. The final `preview_keyboard` run
`run-og9690yy` passed both effective backends, compilation/hover/wheel negative
checks, three click/Escape/typing round trips, focused Tinymist inversion, and
switching a focused preview to code-only mode. The user's exact original surprise
focus-transfer sequence was not established; these results do not claim otherwise.

The native runner's target-availability wait now uses the same 500 ms freshness
limit as its click routine. During the larger fixture's close confirmation,
freshly advancing child frames were roughly 170 ms old in the parent snapshot;
the former separate 100 ms wait rejected them although the click contract allowed
them. Clicks still require fresh, settled geometry and verified action outcomes.

Fresh framebuffer `.tiptoptyp/screenshots/agent-review/1790721784500-0002-main-preview-focus.png`
(retained from `run-ad2ww8hb/1790721784500-0002-main.png`) was inspected: the
PDFium outline and top-right focus badge are readable and unclipped. Its initial
frame `1790721774601-0001-main.png` was also inspected. These files are under
`.tiptoptyp/desktop-ui-tests/`; their fresh original captures were written under
the isolated fixture's `.tiptoptyp/screenshots/`. This verifies the egui appearance,
not native Tinymist composition. Tinymist focus was verified by real keyboard
input and the live renderer state, without claiming a composed desktop capture.

Performance: no new timers, workers, idle repaint loops or per-frame disk writes.
Focus messages are event-driven and deduplicated. Selection copies occur only for
explicit delimiter typing over a selection. Folding's attention animation uses
the existing bounded timer. No material steady-state performance change or
cross-platform timing claim is made.

Validation: formatting, all-target Clippy with warnings denied, the complete Rust
test suite, all 15 xtask tests, and all 9 JavaScript adapter tests passed. The
toolbar harness now renders the real editor (including its fonts), so Code's
focus destination exists in the accessibility tree. The nested-fold harness uses
a bounded number of frames while the deliberate caret highlight animates; its
fold geometry assertions are unchanged. Opt-in/ignored tests remain opt-in; native
interaction results above are reported separately from the standard suite.

The existing native background/app-switch journey also passed in `run-bbazewgv`;
preview work did not activate the app while it was in the background.


## 2026-09-30 — Indentation, panel focus borders and Git navigation

Harper reproduced a false spacing warning for first-line indentation and for
indentation in a multiline Typst string. Checks now exclude lints wholly within
leading spaces/tabs, using original scalar coordinates. Inline repeated spaces,
misspellings and the separate Unicode checks remain eligible. Regression tests
exercise Typst, TeX, tabs, CRLF and unchanged typo locations.

The source editor, PDFium and explorer sections share the same hover/focus stroke
selection. Explorer focus follows the actual focused widget's clipped hit area,
including search inputs and blank panel space. Idle sections retain their single
flat separators; interaction borders paint last and take no layout space.
Tinymist receives the same theme strokes through its cached native style update.
The previous preview focus badges are removed from both viewers. JavaScript
coverage checks focus/blur, palette propagation and the absence of a badge or
pointer-intercepting overlay.

Git shortcut navigation leaves focus in source without a popup. Popup arrows
navigate, settle the source selection/scroll, then open the destination diff at
its caret. An intervening document edit cancels a pending popup. Deterministic
coverage checks routing, cancellation and long-document scroll placement; the
new `git_hunks` native journey checks repeated navigation and wraparound against
a real temporary repository without changing user files.

No persistent repaint loop or per-frame service work is added. Harper's extra
indent scan runs once per background check; native styles are sent only when
changed. Popup navigation requests two bounded layout passes.

Native verification is pending: `python3 scripts/test-desktop-ui.py --journey
git_hunks` stopped at preflight in `run-apvan0pd` with Accessibility/input granted
but `desktop: false, locked: true`. No native interaction or visual pass is
claimed for this attempt.

Required formatting, strict Clippy, full Rust suites, xtask tests and the ten
preview JavaScript tests passed. The `desktop-ui-tests` feature also compiles.
A fresh light-mode framebuffer was captured and inspected:
`.tiptoptyp/screenshots/agent-review/1790749556669-0001-main-pdfium-preview.png`.
The idle PDFium/explorer layout and edges are intact; this unfocused capture
is not evidence of active border colors or native Tinymist composition.
Capture command: `cargo run --release -- --ui-theme catppuccin-latte
--ui-snapshot-scene pdfium-preview --ui-screenshot-subdir screenshots/agent-review
--ui-screenshot-exit test.typ`.

## 2026-09-30 — unlocked native rerun and mathematics vocabulary

After unlocking the desktop, all five affected real-app interaction journeys
passed with Accessibility and input posting available:

- `git_hunks`: `run-3jegt80p`, repeated keyboard/popup navigation and wraparound.
- `editing`: `run-ptcnw102`, typing, undo/redo caret, Unicode and comments.
- `diagnostics`: `run-xmu8r0ho`, broken source, Problems navigation and recovery.
- `explorer_scroll`: `run-pg__a39d`, actual explorer scrolling and search.
- `preview_keyboard`: `run-40d4y41g`, repeated PDFium/Tinymist focus transitions,
  passive scrolling, preview shortcuts and return to source typing.

Evidence is retained under `.tiptoptyp/desktop-ui-tests/<run>/result.json`.
These are native interaction passes, not framebuffer or composed-pixel checks.
They supersede the unavailable interaction check recorded above.

Harper now shares a reviewed 467-spelling mathematics supplement between the
Typst and TeX adapters. Regression tests retain typo and repeated-word detection,
curated metadata and explicit spellings; no arbitrary affix exemption is added.
Formatting, strict Clippy, the full Rust suites and xtask tests passed. The
optimized same-fixture worker measurement was 13,073 us curated versus 13,080 us
with the supplement, within local timing noise; metadata is recorded in
`docs/performance-results/math-dictionary-2026-09-30.json`. No layout changes or
additional screenshots are required for this vocabulary change.

## Settings help, editor decorations and compact tooltips (2026-09-30)

Settings now has a Help title-bar button, effective shortcut tables, tool-specific
regional suppression examples and tips. The bundled Badness 0.24.0 was checked
with stdin fixtures: formatting-off preserves a region and lint-skip suppresses
redundant-script-braces. Editor settings add default-on configurable indentation
guides, optional definition highlighting and per-match styling (TODO/FIXME
defaults). Existing wrap controls remain available. JSON validation and settings
merge/roundtrip coverage include the new options.

Native `editor_preferences` passed in `run-vco2xdey`: Help, focused settings
search, toggles/restoration and typing in a custom match field leave source
untouched. The preview-controls journey passed in `run-5dgd4w39`, including real
PDFium/Tinymist transitions, compact tooltip width and drag-release positioning.
The journey initially exposed release overshoot; applying the final pointer
position fixes it. Fresh native viewport captures were inspected for Help,
custom-rule table alignment, editor guides and compact tooltip appearance.
These are viewport framebuffers, not evidence of composed desktop geometry.
The 25-image maintained gallery was regenerated and validated.

Pure tests cover byte-safe literal matches, definitions, visible/bounded guide
work, decoration cache invalidation and tooltip text sizing/caps. Semantic tests
cover settings ownership, persistence and table geometry. TeX definition
highlighting is lexical and does not evaluate macro expansion. Tool-specific
region directives do not disable compilation or every diagnostic provider.
See `help-and-editor-decorations.md` and the performance measurement metadata.

Handoff checks passed: formatting (workspace and xtask), strict all-target
Clippy, the full Rust test suites, and all 15 xtask tests. Native interaction
results above are separate from those deterministic checks.

## Actual indentation levels (2026-09-30)

Fixed the guide renderer's fixed-step ladder: only positive indentation columns
used by nonblank source lines are eligible, including alignment columns that
are not multiples of the tab size. Blank lines do not introduce levels; wrapped
continuations and folded rows remain excluded from painting. The ordered levels
are cached by galley identity and tab width, with no source scan on unchanged
frames. Visible-row and horizontal clipping still bound painting. Rebuilding
levels scans source and sorts the collected columns only on layout changes.

Regression tests establish the former failure with levels 2/5/17 (no intermediate
levels and no whitespace-only level), tab width changes, edit invalidation and
cache reuse. A fresh `actual-indentation` main viewport PNG with a 2/17/19-space
math fixture was inspected: only those three columns are drawn, and the large
alignment jump has no ladder of extra lines. This is framebuffer evidence, not
a composed native-preview geometry check. Formatting, strict Clippy, full Rust
tests and 15 xtask tests passed.

The guide Settings native journey passed again in `run-_rjtqe_6`; the refreshed
25-image gallery validated.

## Block-local indentation guides (2026-09-30)

The previous actual-column filter was still global: a deeply indented line
received guides for shallower columns used anywhere else in the document.
Guides now follow a local indentation stack. Increasing indentation introduces
one level; dedenting removes deeper levels, and unindented code resets the
stack. Wrapped continuations do not introduce levels. This retains irregular
math alignment without importing another block's columns. The per-row guide
list is cached with the galley and tab width; unchanged frames do not scan code.
Regression coverage includes unrelated blocks, irregular jumps, dedents, blank
lines, edits and tab-width invalidation.

Fresh `local-indentation/1790770959727-0001-main.png` was inspected: the earlier
2/4/6/8-space block keeps its own nested guides, while the later math block
shows only 2/32-space guides and drops the alignment guide on dedent. This is
viewport framebuffer evidence. Formatting, strict Clippy, full Rust suites and
15 xtask tests passed.

The native editor-preferences journey passed in `run-n2ewe48c`; the regenerated
25-image gallery validated.

## Syntax-aware math alignment guides (2026-09-30)

Indentation nesting is inappropriate for equation padding: different left-hand
sides can start at different columns while sharing an `&` alignment point.
Typst now uses actual Equation/MathAlignPoint syntax. TeX uses a tolerant math
reader for dollar/double-dollar, bracket/parenthesis delimiters and standard
alignment, matrix/cases and equation environments. Escapes, comments and verbatim
payloads do not create markers. Nested TeX environments restore the parent's
alignment. Inline math leaves ordinary indentation unchanged.

The structural math guide derives from equation rows, not padding before a
marker-only continuation. The alignment guide sits after `&`, with no extra
indentation guide before that marker; glyphs are painted only at clear gaps,
never across unrelated text. Regressions cover varying left-hand-side padding,
marker-only continuations, nested environments, multiple markers, escaping and
inline math. Representative fixtures are retained under
`tests/fixtures/indentation-guides`.

Math parsing is cached by galley, tab width and document kind. The optimized
local extraction measurement added about 0.31 ms for a 600-line Typst fixture
and 0.06 ms for a 600-line TeX fixture on a cache miss; the former semantic
extraction baseline had no parsing work. This is not a full typing benchmark.
Metadata is in `performance-results/math-guide-parsing-2026-09-30.json`.

Final marker placement was inspected in fresh
`ampersand-continuation/1790773131387-0001-main.png`: the second marker has one
guide to its right and no guide immediately to its left. The compact fixture
keeps the continuation visible without soft wrapping. Formatting, strict
Clippy, full Rust suites and 15 xtask tests pass. The final 25-image gallery
was regenerated and validated. Native `editor_preferences` could not run in
`run-sn084ztp`: its preflight reported desktop=false, locked=true, with input
and Accessibility permissions true. This unavailable interaction check is
not a pass; framebuffer evidence is separate from native interaction evidence.

The final TeX framebuffer `tex-after-marker/1790773303641-0001-main.png`
was also inspected and has the alignment guide after each real ampersand.

## Ordinary guides for unaligned math (2026-09-30)

Math blocks with no genuine alignment markers now use ordinary indentation
guides. Escaped ampersands do not activate alignment mode. Nested unaligned
blocks inside aligned environments use ordinary guides locally and restore
the parent's alignment after closing. Pure regressions compare marker-free
Typst/TeX output against ordinary text and retain aligned-equation behavior.
Only guide eligibility/state changes; drawing coordinates and interactions
are unchanged, so deterministic state coverage is sufficient for this change.

The existing cached parse already discovers alignment markers. Selection adds
an empty-list check; unaligned blocks skip the special body-indentation scan.
Nested scopes retain their enclosing indentation stack on cache rebuilds.
Idle frames still reuse the cached per-row guide list, with no additional parsing
or repaint requests. No material performance impact is expected; this statement
is algorithmic, not a new wall-time benchmark.

Formatting (workspace and xtask), strict all-target Clippy, the full Rust
suites and 15 xtask tests passed for this eligibility change.

### Marker-only equation guides and indented comments (2026-09-30)

Reproduced the reported duplicate guide with a failing deterministic test:
marker-only equations incorrectly used continuation padding as their body indent.
They now retain the enclosing indent and one guide after the alignment marker.
The exact reported equation is retained in
`tests/fixtures/indentation-guides/marker-only.typ`. A fresh framebuffer at
`.tiptoptyp/screenshots/agent-review/marker-only-equation/1790775724595-0001-main.png`
was inspected; the extra half-column-offset guide is absent.

Comment toggling inserts the marker after leading spaces/tabs and round trips
Unicode selections and cursors. TeX uses `%`, Typst uses `//`.
The native `editing` journey now checks indented comment round trips and passed
in `run-tr8h4p4z` with Accessibility, input posting and unlocked desktop verified.
Formatting, strict all-target Clippy, full Rust tests and 15 xtask tests passed.
Guide lists remain cached; this removes a fallback calculation and adds no
per-frame work or repaint requests. No material performance impact is expected.


### 2026-09-30: docked hunk editor and read-only hover

The Git editor stays inside the document's code pane. Marker hover reuses the
compact paired diff renderer and the existing non-activating tooltip lifecycle;
clicking retires that tooltip and opens the docked editor. Individual changed
lines can be reverted in a draft, or the current side edited with the shared
syntax-aware code field. Applying checks the original source and is one undo
transaction. Navigation/repository actions are disabled while a draft differs
from its original bytes. Tooltip sizing is cached by content, style, width and
scale; no extra idle repaint loop or per-frame disk work was added.

Native `git_hunks` passed in `run-ly5e16_n`: focus-preserving hover, repeated hunk
navigation, owner resizing, individual line reversions, manual typing, Apply and
Undo. Fresh diagnostic/docked/edit viewport PNGs were inspected for alignment,
clipping, pairing and line numbers. These are separate viewport framebuffers,
not proof of desktop composition. The 25-image gallery was regenerated and
validated. Formatting, strict all-target Clippy, the full Rust suite (1,204 unit
tests passed, 34 intentionally ignored, plus integration tests) and all 15 xtask
tests passed. The reported table fixture and math conversions have focused
parser/state/semantic regressions; native table/math journeys remain a gap.

### 2026-09-30: full-document live diff and Escape sizing

Replaced the docked draft/edit switch with one editable full-document comparison.
The current column is the actual document, sharing its source widget identity,
Undo/Redo history, auto-pairing, autosave and preview flow. All baseline/current
lines remain visible; wrapped pairs align through geometric gaps without adding
characters to the source. Syntax, rainbow brackets and custom decorations remain
on unchanged lines; Git colors override changed lines. Marker clicks and the
previous/next controls move the real cursor through the live comparison.

Reintroduced the old empty Escape render pass to establish a failing regression:
a 600-pixel resizable code pane shrank to its 100-pixel minimum. Escape now paints
the ordinary editor in the same pass and preserves its width and source cursor.
The failing reproduction is retained in the ignored profiling evidence folder.
Deterministic coverage includes complete 180-line navigation, bounded large-file
comparison, exhaustive short repeated-line cases, wrapping/cursor identity,
Unicode/final-newline changes, direct semantic typing and layout-cache reuse.

Formatting, strict all-target Clippy, the full Rust suite (1,208 app unit tests,
35 intentionally ignored, plus core/integration/doc tests) and 15 xtask tests
passed. The desktop-test build also compiled successfully. The fresh framebuffer
`.tiptoptyp/screenshots/agent-review/1790800458813-0001-main-git-chunk.png`
was inspected for paired alignment, syntax, line numbers and complete source.
The 25-image gallery was regenerated and validated; representative light main
and dark popup captures were inspected. Framebuffers do not prove native desktop
composition or input behavior.

The expanded native `git_hunks` journey checks editing beyond the original hunk,
Undo/Redo, resizing, find-focus ownership and Escape divider stability. Both
attempts stopped before interaction because macOS reported
`accessibility=true, input=true, desktop=false, locked=true`.
This remains an unavailable native check, not a pass. Preparation evidence is
`run-nwbbovq0`. The matched optimized algorithm measurement is recorded in
`docs/performance.md`; it measures comparison work, not end-to-end typing latency.

### 2026-09-30: unlocked live-diff native rerun

Native input became available and `git_hunks` exercised the real editor in
`run-fyvb56sh`, `run-ykuuohuu` and `run-32lsfqqq`. The latter run passed marker
hover/click, repeated navigation, owner resizing, line revert, typing outside
the original hunk, Undo/Redo, editing the last line, Escape preserving the pane
width over repeated frames, and reopening the full-document comparison.
The requested and effective preview backend were both PDFium. A fresh root
framebuffer from the first run was inspected for aligned columns and Git colors;
this is separate from native input evidence and not a whole-desktop capture.

The runs corrected two test assumptions: changes within the same three-line
context merge into one hunk, and returning from the end of a large document
requires waiting for newly visible gutter geometry before clicking it. They also
found a real regression: Find's state became visible while the docked diff's early
render path omitted its overlay. The live diff now calls the same Find overlay as
the regular editor. A semantic regression failed on the missing Find controls
before the fix and passes afterward; it checks popup presence, typing/focus,
unchanged source, and toggling closed while the comparison remains open.

Formatting, strict all-target Clippy, 1,209 app unit tests plus core/integration/
doc tests, and 15 xtask tests passed. The build-identity integration check initially
collided with a simultaneous desktop-feature build replacing the app executable;
its isolated rerun passed. The desktop-feature build compiled. No material
performance impact is expected: the existing overlay is now rendered from the
alternate editor path, with no new worker or idle repaint loop.

The final native rerun stopped at preflight because the desktop had locked again
(`accessibility=true, input=true, desktop=false, locked=true`). The post-fix native
Find checks remain pending; these partial native runs are not a full journey pass.

### 2026-09-30: post-fix native verification completed

The full `git_hunks` journey passed in `run-5wxcbd2p` on build
`761274e5ad4f.1790806898`, with Accessibility/input permissions and an unlocked
desktop confirmed. This closes the pending post-fix native Find check above:
Find opens over the live comparison, owns query typing without editing source,
and Cmd+F closes it while returning focus to the comparison. The same run passed
all full-document editing, Undo/Redo, navigation, line-revert, owner-resize and
Escape divider-stability checks. Requested/effective backend was switched and
verified as PDFium before those interactions. Fresh end-of-document framebuffer
`1790806939231-0003-main.png` was inspected for full-document pairing, the added
last line and clipping. Native interaction results and framebuffer evidence
remain separate; no whole-desktop composition capture was performed.

### 2026-10-02: resize presentation, divider grips and keyboard ownership

Source reflow now discards the unaligned paint and presents the corrected scroll
anchor in the same frame. A wrapped 100-line regression requires the correction
pass, checks the caret stays within one point and verifies settled frames use one
pass. The matched optimized presentation probe is recorded in `docs/performance.md`.

Independent main dividers is available under Editor settings, with the previous
fixed-code-width policy retained as the default. Compensation uses the Explorer
width actually drawn on each pass, rather than PanelState's pre-drag saved width.
The native `git_hunks` journey passed in `run-wpqo5bmk`: divider drags in both
policies, full-document diff editing/navigation, Find typing/toggling, Escape,
and a native toolbar tooltip with the effective Tinymist backend all passed.
Temporary menus and Settings retain the docked comparison. The test's concurrent
snapshot sampling did not observe intermediate drag frames on this macOS run;
final native geometry passed, and no claim of continuous desktop frame timing is
made. Earlier failures are retained: a wrong inspection target, a missed Settings
focus transition and Settings dismissing the persistent comparison.

The native `preview_keyboard` journey passed in `run-wkvmez5n` with both PDFium
and Tinymist confirmed as effective backends. Clicking the preview clears the old
editor keyboard owner; preview shortcuts, Escape back to source, typing and Undo
passed. A deterministic negative test also checks an unrelated focused field and
Explorer ownership are retired.

Grips are paint-only, centered on the actual separator rule, with horizontal and
vertical variants. Explorer grips are visible only when the upper section has an
allocated resize handle; collapse/expand visibility and centered geometry have
regression coverage. Fresh light framebuffer
`panel-grips-final/1790932142458-0001-main.png` was inspected for these relationships.
Native Tinymist content reserves four points next to shared grips. Bounds traces
are retained in the native run's `app.log`, for example:

```
ui.preview.bounds available=(803.6,30.0)-(1400.0,826.0) clip=(803.6,30.0)-(1400.0,826.0) egui=(807.6,30.0)-(1400.0,826.0) native=(807.6,30.0)-(1400.0,826.0) viewport=(0.0,0.0)-(1400.0,850.0) zoom=1.000 egui_ppp=2.000 native_ppp=2.000
```

Framebuffer captures do not prove native child composition; no whole-desktop
composition capture was performed. The native host existence and bounds checks
are separate evidence. No worker, idle repaint loop or per-frame disk I/O was
added; divider compensation and grip painting do constant work.

Final formatting and strict all-target Clippy passed. Full tests passed: 1,217
app unit tests plus core, integration and doc tests; 15 xtask tests passed. One
older navigation test was updated to explicitly close the docked comparison,
rather than use the transient-popup dismissal it previously relied on.

The stable gallery was regenerated in one session and all 25 PNGs validated.
The fresh dark `main--catppuccin-mocha.png` was inspected as well: centered
vertical grips, shared horizontal rules, and no grips on collapsed sections.
Resize anchoring explicitly disables animation for its correction; ordinary
scrolling is unchanged. Its regression now uses the default animated scrolling
style and advances the warmup clock to settle the initial cursor reveal.

The post-animation-change native `git_hunks` rerun also passed in
`run-ez2g_lpy`; effective PDFium/Tinymist backend checks and retained geometry
traces accompany the interactions.

### 2026-10-03: audit of the recent docked editor and divider commits

Reviewed `361dcce`, `fd5a6b6`, `761274e`, `1541f07` and `c32e076`, including
full-document alignment, editing/actions, shared highlighting, math conversion,
resize state, native tooltip routing and preview keyboard ownership.

Four confirmed bugs were fixed:

- The full-diff layout cache omitted the language extension. A failing test
  reused Typst highlighting for identical TeX text; the extension now participates
  in the key, with a repeat-hit test confirming correct layouts still reuse it.
- Restoring a line from a baseline without a final newline could join it to its
  neighbor (`oldnext`). Line reverts now preserve logical boundaries, including
  insertion after a current EOF without a newline, while keeping a final EOF
  newline-free when appropriate. The boundary regression failed before the fix.
- Stage/Unstage shortcuts in the live diff targeted the initially clicked hunk,
  even after the caret moved to another hunk. The live view now selects actions
  from the caret. Native `git_hunks` failed before the fix in `run-54vek7jc` and
  passed afterward in `run-b0laciuk`. The extended journey checks the disposable
  Git index contents: only the second hunk is staged, then the same hunk is
  unstaged. Existing navigation, editing, Undo/Redo, Find and native tooltip
  checks also passed, with effective PDFium and Tinymist backends confirmed.
- Math conversion trimmed the newline terminating a TeX `%` comment, putting the
  replacement closing delimiter inside that comment. TeX body bytes are now
  preserved exactly across all offered delimiter choices, without introducing
  blank lines inside math environments. Typst inline conversion also retains a
  newline terminating `//`; syntax diagnostics confirm the result parses. The
  TeX regression failed before the fix.

Hunk formatting repeatedly counted every preceding line for each disjoint hunk.
It now advances old/new offsets over disjoint ranges, making that step linear in
pair count. Exhaustive short-sequence baseline recovery, missing-newline cases,
and a 10,000-line/33-hunk round trip cover the result. Matching optimized
component measurements are recorded in `docs/performance.md`.

The retained native run traces check existing geometry; this audit changes no
panel/native bounds or maintained visual layout. Deterministic syntax/behavior
coverage is used rather than framebuffer captures. No whole-desktop composition
verification is claimed. There is no new worker, idle repaint, profiling hook or
per-frame disk I/O.

The follow-up selection audit established a failing comment regression:
`0..4` in `one\ntwo\nthree` commented only `one`, despite the active caret
being at line two, column zero. Comment toggling now includes that active line,
including a blank endpoint, but excludes a column-zero anchor on a reversed
selection. Indentation and Unicode scalar offsets remain intact; selection
orientation survives commenting and uncommenting. Focused Typst/TeX tests cover
forward/reversed endpoints and blank-line round trips.

The native `editing` journey also verifies upward keyboard and mouse selection
from column zero includes the preceding newline, deletion removes that newline,
Undo restores it, and both selection directions survive comment round trips.
The existing column-zero character selection already behaved correctly. VS Code
uses a distinct whole-line gutter selection (including the starting line's
newline): see `CursorMoveCommands.line` in its source. VS Code's comment command
excludes a final zero-column endpoint, whereas tiptoptyp intentionally includes
it when it is the active caret, as requested. Sources:
https://github.com/microsoft/vscode/blob/main/src/vs/editor/common/cursor/cursorMoveCommands.ts#L156-L208
https://github.com/microsoft/vscode/blob/main/src/vs/editor/contrib/comment/browser/lineCommentCommand.ts#L315-L318

The selection change adds work only when a comment command runs; it has no
material idle or per-frame performance impact. The line-start native probes are
bounded to four visible targets and compiled only for the desktop test feature.

Final follow-up native interaction evidence: `editing` passed in
`run-ae7c2a69`, including a blank final cursor line. The earlier character-drag
and reversed-selection run also passed in `run-u_yp9f0b`. These are native input
results, separate from deterministic Rust tests; no screenshot was necessary.

Handoff checks passed: `cargo fmt --all -- --check`, strict all-target Clippy,
`cargo test --no-fail-fast` (1,225 app tests plus core/integration/doc tests),
and all 15 xtask tests. Evidence logs are retained as
`/private/tmp/ttt-audit-handoff-{clippy,tests,xtask}.log`.

### 2026-10-03: startup initialization

Deferred generic syntax/default-theme database decoding to first use; Typst
keeps its existing parser. Shared the fixed platform CJK fallback bytes across
actual font engines, preserving the selected file, face, tweak and fallback
order. Deterministic tests cover no eager database work, sharing after use,
custom-theme avoidance and borrowed CJK pointers in two font engines.

The native `startup_preview` journey passed in `run-r7aqbkf8`: fresh history,
unbound initial buffer, Cmd+O and first-file preview targeting. First-use JSON
editing/search/validation passed in `run-9zs9wgxi` (`settings_search`), and normal
editing/Unicode/Undo/Redo/selection/comment interactions passed in
`run-o_1hp46h` (`editing`). These are native input results, not screenshot or
compositor verification. No pixels/geometry changed; existing font/glyph and
highlighting coverage provides the proportionate evidence.

Matching optimized startup measurements, limitations and the new external
runner are documented in `docs/performance.md`. Required fmt/strict Clippy,
all app/core/integration/doc tests (1,227 app tests), and all 15 xtask tests
passed. Logs: `/private/tmp/ttt-startup-final-{clippy,tests,xtask}.log`.

### 2026-10-03: startup change audit

Reviewed lazy syntax/theme initialization, shared platform CJK font lifetimes,
and the startup measurement runner. Reproduced and fixed runner cleanup bugs
for failed sampler launch, timed-out sampling and failed quit input; added
bounded forced shutdown, fresh-checkout driver-directory setup, incremental
measurement persistence and counterbalanced comparison order. Eight Python
regressions pass. Production UI code and pixels are unchanged.

The native sampled runner smoke test completed two launches and automatic
shutdown; evidence is `.tiptoptyp/profiles/startup-audit-smoke/`. This exercises
the runner, not an additional editor interaction journey or visual QA claim.

Handoff validation passed: fmt, strict all-target Clippy, full Rust tests (1,227
app tests plus core/integration/doc tests), and all 15 xtask tests. Logs are
`/private/tmp/ttt-startup-audit-{clippy,tests,xtask}.log`.

### 2026-10-03: native TeX editing journeys

Added `tex_editing`, `tex_intelligence` and `tex_save`, included in `all` and
available together as `--journey tex`. The editing journey uses a named TeX file
and a 320-line source, not an untyped scratch buffer. Coverage includes Unicode
carets, undo/redo, comments and selection wrapping, literal command replacement,
real diagnostic providers, Problems navigation, command completion acceptance,
and native Save/manual Save/close/reopen.

The failing native intelligence run `run-obmme923` exposed Harper treating
`\[...\]` identifiers as prose while missing `\text{...}` inside equations.
The deterministic delimiter/environment regression also failed before repair
(`/private/tmp/ttt-tex-boundary-before.log`). A source-coordinate-preserving
adapter now shares the existing TeX math scanner, retains prose-command bodies,
and emits non-prose tokens for equation gaps. The tokens avoid the additional
false whitespace warning in ordinary `with $x$.` prose. Four focused Rust tests
cover delimiters/environments, spacing, Unicode offsets, nested groups, escaped
commands and comments; Typst's parser is unchanged.

Passing individual native evidence: `tex_editing` in `run-kj18oro8` (large source),
`tex_intelligence` in `run-898ymo_t`, `tex_save` in `run-1f8ktqbh`, and the existing
`dialogs` journey in `run-k0a4ugvo`. The intelligence run observed stale Harper
results in seven snapshots. Badness refreshed before a stale snapshot was
observed; its retention assertion is conditional on a pending revision and does
not establish a slow-provider interval in this run. Initial completion probes
outside a complete TeX document returned empty results from TexLab itself; the
final fixture includes a document and a locally defined command, avoiding an
external TeX package database prerequisite.

The Save harness previously assumed native dialogs used the launch workspace;
a failed TeX save landed in Documents. The exact generated fixture was removed,
and both Save journeys now use native Go to Folder before confirming. A runner
contract test protects that ordering. The existing Open fixture also used a
literal `\n` TeX command; it now contains a real newline. Feature-enabled strict
Clippy uncovered a helper placed after a test module; moving it before the
module fixes that check without changing behavior.

There is no pixel/layout or preview-composition change, so no screenshot was
required. Added TeX scanning runs in the existing bounded writing worker after
its debounce, with no new jobs, idle repaint loop or editor-thread parsing.
These functional runs do not establish a typing-latency or performance gain.

The combined `--journey tex` run passed all three journeys in one native app
session (`run-5aqehg8i`), including settings restoration between journeys.
Handoff checks passed: fmt, strict all-target Clippy (default and
`desktop-ui-tests` feature), full Rust tests (1,231 app tests plus core,
integration and docs), 15 xtask tests, and 11 runner contract tests. Logs are
`/private/tmp/ttt-tex-final-{clippy,tests}.log`,
`/private/tmp/ttt-tex-feature-clippy-final.log`, `/private/tmp/ttt-tex-xtask.log`
and `/private/tmp/ttt-tex-combined-native.log`.

## 2026-10-03: preview work versus a saved PDF

The engine gear now refreshes live preview. A separate fixed-size PDF action
creates the user-facing file; Cmd+R retains that action, and Export PDF retains
its destination dialog. Its icon/color distinguishes no confirmed save, a
successful save, stale output, pending creation and failure. The tooltip shows
the destination and explains that live preview does not write it. The same
controls apply to Typst and TeX: Tinymist streams preview without PDF bytes;
PDFium's Typst builds and Tectonic's TeX builds produce private preview PDFs.

PDF status records only durable writes observed by this window, per tab. It
does not claim to inspect files left by a previous app session or another
process. Source edits and new canonical artifact generations make a previous
receipt stale. Tab activation rebases service keys without discarding the
receipt; unchanged Tinymist refresh does not manufacture staleness.

The failing reproduction in `/private/tmp/ttt-pdf-owner-before.log` showed that
changing the pinned preview during queued output could write the new preview
into the previous destination. PDF requests and destination dialogs now carry
the preview tab and document epoch. Preview changes cancel incompatible work;
ordinary editor-tab changes preserve it. Reactivating its source requests a
build for the rebased revision. Build failures clear the pending write and
retain any previous saved file, allowing an explicit retry.

Native `pdf_output` passed (`run-xytpnra5`): confirmed effective Tinymist,
preview-only output absence, actual PDF bytes from button/Cmd+R, unchanged
bytes and modification time during stale preview work, stable saved/failure
states over 30 fresh frames, failed-build preservation/recovery, and PDFium
cache work without rewriting the adjacent file. Native `preview_keyboard`
also passed (`run-frclsngm`) with both backends. Logs:
`/private/tmp/ttt-pdf-native.log` and
`/private/tmp/ttt-pdf-preview-keyboard-native.log`.
Native `tabs` passed (`run-2nxthu0b`), preserving both buffers/carets and the
pinned preview; its log is `/private/tmp/ttt-pdf-tabs-native.log`.

Fresh main framebuffers were captured during the PDF journey and the saved
state was inspected: engine and green PDF check fit in the toolbar without
clipping. These root framebuffers do not contain Tinymist's native child and
are evidence for the toolbar only, not composed preview pixels.

Required checks passed: fmt, strict all-target Clippy (default and
`desktop-ui-tests`), full Rust tests (1,247 app tests plus core/integration/docs),
15 xtask tests and 11 runner contracts. Logs are
`/private/tmp/ttt-pdf-{clippy,all-tests,feature-clippy,xtask}.log`.
Tracking uses one bounded receipt per tab with constant-time comparisons and
no per-frame file I/O, additional worker, or idle repaint timer. No unrelated
performance benchmark was needed.

## 2026-10-04: terminal off control

The terminal action strip has a matching power icon below Restart. Off stops
the shell and its foreground command through the existing worker cancellation
path, keeps the last output available for selection/copy, and clears the cursor
and IME composition. Switching or reopening panels cannot start another shell;
Restart explicitly resets this state. Off is disabled while already off.

Native `panels` passed (`run-ul1sptrq`): ordinary terminal input wrote each
shell's PID, Off reaped both the original and restarted shell, and repeated
panel transitions retained Off without changing the document. The first run
(`run-abpb4fn4`) failed before Terminal opened; its trace recorded unrelated
pointer movement during the tab click. Both runs remain in the evidence folder.
The fresh terminal framebuffer was inspected for the two square controls,
alignment and retained output. The icon inventory was also freshly captured.

Required checks passed: fmt, strict all-target Clippy (default and
`desktop-ui-tests`), full Rust tests (1,249 app tests plus core/integration/docs),
15 xtask tests and 11 runner contracts. Logs are
`/private/tmp/ttt-terminal-off-{clippy,tests,feature-clippy,xtask}.log` and
`/private/tmp/ttt-terminal-off-native-rerun.log`. Off adds no background worker,
idle repaint timer or per-frame disk access; cancellation/reaping never blocks
the UI. No unrelated performance benchmark was needed.


## 2026-10-07: finish terminal links and focus-routed Find

Work was completed directly in `/Users/calvinkhor/Documents/GitHub/mytypst`,
on `codex/finish-terminal-find`, from master `5539652`. The user's `todo.typ`
edits were committed separately as `c3b9cd0`; their descriptions were preserved.
This review covers the unfinished terminal feature, not the broader recent-PR audit.

The existing Escape regression failed before the fix: egui's default text-field
filter cleared focus before the bar could consume Escape. The bar now retains
Escape, while the terminal grid keeps it whenever the search field is unfocused.
Enter/Shift+Enter steps once, focused Cmd+F toggles Find, and terminal Cmd+G/
Shift+Cmd+G does not reach source Find. Find/Replace, clipboard, Select All and
rebound/disabled editing shortcuts stay owned by the terminal query. Switching
bottom-panel tabs releases both terminal widget focus IDs.

The first native run (`run-wium893_`) reproduced a focus loss after clicking the
match arrows, despite a passing basic kittest. Arrow clicks now queue one field
focus request for the next pass; a late-response-read regression exercises the
click-away case. A subsequent run (`run-46ibcy5t`) caught an overly strict journey
assertion: opening the bar resizes the terminal and legitimately queues another
background snapshot. The journey now requires a completed result initially,
then checks that displayed matches, query, source and focus remain stable even
if another refresh becomes pending. It does not hide a changing result/focus.

Search runs on the existing terminal worker and retains at most 4,096 physical
rows / 512 KiB and 8,192 matches. Absolute rows survive truncation and soft wraps;
Unicode lowercase expansion and Greek final sigma retain original character
spans. Match painting visits only visible hits. Output/resize revisions are
separate from scrolling, focus and search requests, so stable results request
no further work or search repaints. Generation tests reject obsolete replies;
throttle/disconnect tests bound requests and pending lifetimes. On EOF the shell
is reaped, while its emulator waits without polling so retained output remains
searchable, scrollable and resizable. These are deterministic performance
invariants, not a measured GUI speed or cross-platform guarantee.

Link fixes preserve single-digit line/column suffixes, both HTTP schemes in
source order, balanced URL parentheses and IPv6 brackets. Home-relative paths
resolve correctly. Command-click bypasses selection and TUI mouse reporting
through the complete press/drag/release gesture. Links split across physical
rows remain a documented limitation; relative paths use the pane's starting
working directory.

Verification:

- `cargo fmt --all -- --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed; the same check with
  `--features desktop-ui-tests` passed.
- `cargo test --no-fail-fast` passed: 1,519 tests, 43 opt-in/ignored tests.
- `cargo test --manifest-path xtask/Cargo.toml` passed: 15 tests.
- The desktop runner's 11 Python contract tests passed.
- Native `terminal_find` passed in `.tiptoptyp/desktop-ui-tests/run-4x0yuu2n`:
  real shell scrollback, mixed-case matches, exact stepping/wrap, arrow focus,
  Cmd+A query edits, grid versus field Escape, Cmd+F toggle/refocus, one-click
  close and separate queries in two panes. The source fingerprint stayed fixed.
- Native `panels` passed in `.tiptoptyp/desktop-ui-tests/run-d8c032gv`:
  panel transitions, real shell typing, Off/reaping, restart, independent panes
  and normal mouse tracking in vim.

Both native runs checked the actual executable hash/build and passed desktop,
Accessibility and input-posting preflight on macOS 14.6.1, Apple Silicon, 2x scale.
Their result JSON and event logs retain the hashes, fixture and run metadata.
The normal debug linker emitted a large-unwind-table warning; the
strict Clippy checks remained clean.

The fresh app-generated Find framebuffer (selected visible match, `3/3`) was
inspected for field/button alignment, clipping and highlight placement. It is
retained at `.tiptoptyp/screenshots/agent-review/terminal-find-20261007.png`, copied
unchanged from `run-4x0yuu2n/1791372029582-0003-main.png`. This is viewport pixel
evidence, not a claim about native-child desktop composition; this journey uses
a plain-text document with no native preview surface.

## 7 October: two-day change audit

Reviewed all 29 commits from `e1e0280^` through `104af1c` (5–7 October),
including terminal mouse reporting, panes, palettes, alerts, links and Find;
math/context menus; backend-dependent build buttons and disk output; cached
editor formatting and probes; document/viewport ownership and frame telemetry;
Intel build tooling, font packaging, arXiv fixtures and screenshot contracts.
The audit ran in `/Users/calvinkhor/Documents/GitHub/mytypst` on
`codex/audit-october-5-7`; user TODO descriptions were left untouched.

Confirmed defects and corrections:

- The compiler gear for Typst CLI and TeX refreshed only a private preview even
  though its separate PDF action was hidden. It now uses the existing durable
  PDF transaction to save the adjacent file. Automatic previews remain private.
  Manual builds work while automatic updates are paused and leave them paused.
- Cmd+Shift+R was consumed by the less specific Cmd+R binding. Both build actions
  now use the existing shortcut dispatcher, which selects the most specific
  binding before consuming an event. The native PDF journey exposed this bug.
- Typst display math was classified by newline presence. The conversion menu
  now uses the parsed equation mode: `$ x $` is display math and `$x$` is inline.
  Conversion flyout state belongs to its popup generation, preventing stale
  choices from appearing on a reopened menu's first frame.
- auto-miTeX completion returned `None` in projected dollar math without a
  backslash, incorrectly permitting fallback Typst suggestions. Projection
  ownership is checked before the completion shortcut, including empty results.
- Hidden terminal bells did not wake the app. New alerts now request one
  coalesced repaint without rendering hidden output. Held terminal mouse drags
  release on pane hiding or native focus loss, retaining the last encoded
  geometry. Palette notifications reach every started pane, including hidden
  programs, with deduplication and bounded retry only for mailbox backpressure.
- The redraw monitor treated extended typing/dragging as an idle redraw problem
  and observed only the root while reporting through another active document.
  Each document host now observes its own actual paints; input/held keys reset
  the no-input interval. Notices describe sustained redraw rather than asserting
  the app is stuck. Development statistics restart their cadence with a burst.
- Typst sometimes reports `.tiptoptyp/source-*/project/...` paths relative to
  its working directory. Problems now maps those generated mirrors to original
  source paths by components, including included files with identical basenames
  and working-directory aliases. Mapping survives mirror cleanup and retains
  source coordinates. Diagnostic/preview navigation reuses an untitled backing's
  document owner and rejects unowned generated copies; ordinary user config
  files remain openable. Projected Unicode locations remap exactly once.
- The Intel script verified a relative target directory against the caller
  rather than the repository. Build and verification now share an absolute
  explicit target directory; extra arguments are rejected. The opt-in arXiv
  suite checks installed engines and refuses a vacuous pass with none installed.
- Hack was incorrectly shipped with an OFL notice. Its exact upstream v3.003
  MIT/Bitstream Vera/public-domain notice now matches all four fonts' embedded
  metadata. A shared font registry feeds aggregate notice generation and package
  verification; tests reject missing/changed Hack notices and incomplete
  manifests. Regenerated aggregate notices also include formerly omitted
  bundled terminal notices. Two tracked Python bytecode files were removed and
  future bytecode caches are ignored.

Architecture and performance review:

The worker-per-terminal design, bounded Find dumps/caches, owner-local document
windows, cached syntax formats, guide walk guards and revision/source-bound PDF
receipts remain appropriate. The fixes reuse those structures, the common
shortcut dispatcher and the private-workspace namespace rather than adding
parallel ownership systems. Ordinary diagnostic paths bypass generated-source
root scans; custom working directories canonicalize once per watch context.
Terminal color notifications do no worker work for unchanged palettes; hidden
plain output still causes no repaint/render. Frame telemetry keeps counters,
requests no repaint and retains no document/input contents. Regression tests
check these bounded-work rules. No before/after GUI performance benchmark was
run and no speed or cross-platform performance claim is made.

Failing reproductions were established before fixes. Local logs are retained at
`/private/tmp/ttt-audit-{math,build,terminal,completion,monitor,menu,pause}-before.log`
and `/private/tmp/ttt-audit-diagnostic-*-before.log`;
`/private/tmp/ttt-audit-diagnostic-before.log` retains real Typst 0.15.1 output.
Compiler/type errors during test preparation were not counted as behavioral
reproductions. Native failures and successful reruns are listed below.

Limitations: this audit does not establish an Intel/universal distributable.
The Intel script still compiles the executable only; matching sidecars and
PDFium are needed for an Intel package. No installer, fresh Intel cross-build
or Rosetta runtime was run, and the installed app was not altered. Large opt-in
arXiv multi-engine builds were reviewed/typechecked, not executed; the native
TeX fixture uses Tectonic. This audit changes interaction/state contracts rather
than pixel appearance, so no fresh framebuffer/gallery was requested and no
composed desktop visual verification is claimed.

Verification after the fixes:

- `cargo fmt --all -- --check` and the xtask formatting check passed.
- Strict Clippy passed for ordinary, `profiling` and `desktop-ui-tests` builds.
- `cargo test --no-fail-fast`: 1,541 passed, 43 opt-in/ignored.
  `cargo test --features profiling --no-fail-fast`: 1,549 passed, 43 ignored.
- All 17 xtask tests and 22 Python runner/script contract tests passed. Font
  package resource coverage is included in the Rust suite. Notices regenerated
  successfully through `xtask generate-notices`; Intel script shell syntax
  and three fake-compiler contract tests passed.
- The debug linker still reports its pre-existing large unwind-table warning;
  strict Clippy is clean. No release/installer validation is inferred from
  passing debug tests.

Native interaction reruns on macOS 14.6.1 ARM64, 2x scale, with Accessibility
and input posting enabled:

| Journey | Evidence directory | Relevant assertion |
| --- | --- | --- |
| `diagnostics` | `run-eaxqo2eh` | Real Typst compiler error navigates within original source tab; path/tab identity stays stable, no generated-copy tab; undo restores source. |
| `pdf_output` | `run-_zke2qw_` | Tinymist preview-only refresh, explicit PDF output, failed-write retention, CLI gear saves fresh bytes, manual build while paused and later edits stay paused. |
| `tex_save` | `run-fbfgedxc` | TeX gear saves adjacent PDF, no separate PDF button, paused gear builds unsaved source without saving that source or resuming automatic updates. |
| `context_menu` | `run-73wstv3e` | Opens its own fixture, inline/display conversion including same-line display syntax, selection/Copy, menu closure and return to prior tab. |
| `panels` | `run-mh4gz3da` | Real shells, Off/reaping/restart, independent panes, vim mouse reporting and repeated panel transitions. |
| `terminal_find` | `run-16bwbivd` | Real scrollback, matching/stepping, field versus shell Escape, repeated toggle/close, query ownership across panes. |
| `focus` | `run-epo_um3k` | Multiple document windows, Settings ownership and native minimize/app-switch/focus restoration. |
| `tex_editing` | `run-2p9rt21a` | Real TeX editing, Unicode/undo, long-source edits, comments, bracket wrapping and Find/Replace; disabled autosave leaves source disk unchanged. |

Directories are under `.tiptoptyp/desktop-ui-tests/`; their `result.json` and
action logs retain build/source metadata, executable hashes and actual input
evidence. All eight successful runs used the same binary SHA-256:
`90c65372e76017a70c586a711bdf84d119c669cfb82d41ffd6d0ce8de1beb8a2`.
These are separate focused runs, not a full aggregate `all` suite. Terminal
bell/palette backpressure and generated included-file/Unicode edge cases are
covered by deterministic tests, not claimed as native journey assertions.

`run-v5wceiqw` retained the failing native pause-shortcut reproduction before
its fix. `run-kab_s9d_` exposed an invalid multiline input adapter use, and
`run-mdywooaz` exposed the diagnostic fixture's invalid readiness setup; these
were harness failures, not product regressions. The corrected context journey
prepares its fixture itself for standalone and combined execution; diagnostics
starts valid, verifies its effective renderer, then deliberately introduces
the error. No failed/observation-only run is counted as an interaction pass.


## October 7: Explorer deadlock and newest todos

The installed app froze immediately after a real Cmd+1 press. Its main thread
changed from an idle AppKit run loop to a render stack blocked in a condition
wait. A separate failing `git_hunks` journey also stopped during an independent
Explorer divider drag (`run-fh42y93z`). The divider compensation wrote panel
memory inside `Context::data_mut`, then computed a viewport-scoped panel ID
inside that callback. Computing the ID reads the same Context: the write lock
cannot be acquired recursively. IDs now resolve before entering the callback.
The audit also removed read-lock reentry in terminal Find, editor focus and
desktop observations; those callbacks now inspect only their provided data.

Before the Mac locked, the new `explorer_toggle` journey passed in
`run-qwgh8ggt`: Tinymist and PDFium, both divider policies, before/after actual
drags, 48 native Cmd+1 transitions, and subsequent typing/undo. This verifies
interaction with the new build; the installed `/Applications` app was neither
reinstalled nor replaced by the fixture.

Divider grips now shorten to the allocated divider span and clip to their
containing panel. Their centering, separator mask, interaction colors and hit
areas are preserved. Failing geometry coverage preceded this change; normal
and extremely short spans have deterministic clipping tests. This adds only
constant-size paint work.

PDF page appearance was already configurable. Images now have a separate,
default-off dark preference, following the same page Light/Dark/Follow choice.
An image toolbar contrast icon and Appearance checkbox edit that one preference.
Original files and alpha remain unchanged. Decoding/inversion run in the image
worker, never on the UI thread; an original-source cache is bounded to 16 MiB,
keys include canonical target/file identity/change metadata, and a source is
cached only when unchanged throughout decoding. Stale valid pixels stay visible
while refreshes finish. Current errors without usable pixels show a failure
message instead of an indefinite loading label.

Terminal theme initialization was already correct. Two separate protocol bugs
were reproduced: changing palette within the same dark/light mode did not
notify opted-in clients, and a 258-answer palette query consumed one writer
mailbox slot per answer. Replies now coalesce per PTY packet with a 64 KiB bound;
unchanged palettes publish no new grid or notification. This corrects the host
protocol; a named TUI theme can still choose its own colors. The native theme
journey checks receipts from a private real PTY rather than assuming a particular
OpenCode screen appearance. See the [color-update protocol](https://contour-terminal.org/vt-extensions/color-palette-update-notifications/)
and [OpenCode v2 theme settings](https://opencode.ai/v2/docs/themes/).

Selection preservation and direct Typst math-conversion actions now use the
same context-menu path in ordinary and docked diff editors. A review also found
Typst font/link actions leaking into TeX comments/verbatim text; language guards
and regressions cover that case. Parsing occurs only on a context-menu event.
The Intel compile script already existed; an actual x86_64 build and Rosetta
version smoke test passed. That is executable compilation evidence, not an
Intel distributable or PDFium runtime test. Runtime packaging must select Intel
helpers and an Intel PDFium bundle.

Follow-up inspection found two separate Explorer transitions beyond the lock:
independent dividers compensated drags but not a closed Explorer's zero width,
and closing first hid contents before removing the empty panel on the following
frame. The latter matches the reported choppy disappearance; it does not establish
the user's estimated half-second duration. Closing is now a single state/layout
transition, and compensation runs before presenting the editor. A persisted
narrow preview retains its physical edge when Explorer closes/reopens; an actual
viewport shrink still respects available bounds. Tests reuse retained egui Panel
state across open/closed/idle frames, including a forty-point preview.

Near-maximized bottom panels exposed a second geometry issue: child widgets can
inflate the side-panel response beyond its visible allocation. Grips now use the
intersection with the original main-region allocation for centering and clipping.
The geometry regression reproduces a twelve-point visible allocation with a
sixty-four-point child response. This changes paint/observation bounds, not resize
hit areas or native preview placement.

The checked-in `explorer_toggle` journey samples observations concurrently with
native key-down/up and across settled frames, rejects a fresh empty closed-panel
frame, and rejects independent preview-edge movement. Its focused fixture adds
1,800 long source rows; `--release` selects the optimized build and records the
profile. The first debug large-source attempt (`run-gcvirjc3`) exceeded the initial
inspection socket deadline before any interaction; it is not counted as a pass.
Earlier short-divider setup attempts (`run-w47b72b9`, missing observation height;
`run-s3krmkw9`, inflated response geometry) also do not count as passes.

The terminal theme probe source is maintained in
`scripts/desktop_ui_terminal_theme.py`; only its disposable executable instance
and receipt files live in the private temporary fixture. Successful focused
native runs before this final divider follow-up: `terminal_theme` in
`run-d1zt413v`, `images` in `run-a35137in`, `context_menu` in `run-jmr113py`,
and `git_hunks` in `run-ip5x70yl`. Fresh original/dark image framebuffers from
`run-a35137in` were inspected. They verify the framebuffer appearance, not
composed native child-view placement. The theme journey's earlier Settings-cleanup
failure (`run-k0z0c4_r`) is not counted as an interaction pass.

A final image-cache review also removed a redundant full-raster copy for uncached
large images in original-color mode. A deterministic pointer/capacity regression
checks that rasters above the sixteen-MiB cache bound move their pixel allocation.
Cached/shared originals remain immutable. No unrelated latency benchmark or
cross-platform performance guarantee is implied.

The first optimized large-source follow-up (`run-nos3zpqa`) completed all forty-eight
Explorer transitions and 708 sampled transition frames without either regression,
but its separate short-grip setup did not resize the bottom panel; the whole run
is therefore not counted as a pass. The native drag adapter now posts a pointer
approach before mouse-down, so hit testing can retire the previous widget owner.
This follows the same native input-edge separation as the existing click adapter.

Final optimized native verification passed: `explorer_toggle` in `run-blildqe_` with
forty-eight transitions, 712 sampled transition frames, both effective preview
backends/divider policies, typing/undo, and a nearly full-height bottom panel.
`git_hunks` passed in `run-hsc17gwb`. Native targets now read the actual resize
widget response; the bottom hit point uses its intersection with the panel,
clear of the source's inclusive boundary. The previous pointer-approach-only
attempt (`run-x9bpeumf`) completed its transitions but failed short-grip setup;
that full run is not counted as a pass.
The intervening locked-desktop attempt is unavailable interaction coverage.

Fresh `run-blildqe_` main framebuffers were inspected, including the short main
strip above Activity. Grips fit the visible strip and remain centered across its
rules. These are viewport pixels, not a composed native-preview screenshot.
The retained app log includes `ui.preview.bounds` rectangles and scale metadata.
Serial required verification passed: formatting, strict all-target Clippy,
1,589 tests (44 ignored opt-in checks), and 17 xtask tests. An earlier concurrent
native build overwrote the debug executable while build-identity tests ran;
that tooling race failed the earlier suite and was resolved by serial builds.
The final Intel build and Rosetta version check passed again.

The maintained gallery was regenerated in one app session and all twenty-five
PNG files passed decoding/contract validation. Fresh gallery inspection sheets
and the regenerated icon contact sheet were inspected; the new contrast icon
stays within its reference box at 1x/4x. The checked-in contact sheet is
`docs/icons/contact-sheet.png`. The final native-test feature also passes strict
all-target Clippy. Runner failure-detector tests pass (15), as do Intel script
contracts (3). GPUI blur/translucency item 30 remains open; existing PDF page
appearance and Intel compile-script work were audited rather than duplicated.


## Follow-up audit of the latest Explorer and image changes

Audited `8d4a28d` and the preceding terminal-rendering change `c8c93ba`.
The terminal worker, bounded replies, wakeup/throttle races, palette propagation,
and retained rows had no further concrete runtime bug identified.

Reproduced and fixed the following gaps before handoff:

- Closing Explorer while Split was hidden changed the independent divider from
  900 to 600 on returning to Split. Compensation now uses the last actual split
  editor rectangle, rather than an Explorer-width sample overwritten by hidden
  views. Retained egui panel tests cover close/reopen, hidden resize, and viewport
  shrink/grow. The obsolete per-frame temporary width read/write is removed.
- A viewport shrinking exactly to the retained divider could leave zero preview
  width. Retention now requires positive preview space; otherwise normal panel
  limits reserve room for the preview.
- Hidden Explorer sections could keep current or pending keyboard ownership and
  swallow Cmd+F. Closing Explorer, explicitly focusing the source editor, and
  claiming native preview focus now retire both forms of ownership while keeping
  search queries. Normal and docked editor regressions cover Code followed by Find.
- The source image cache retained up to its sixteen-MiB bound after closing or
  leaving an image. Cancellation now queues a bounded worker cache-release event,
  without a result, repaint, or timer. A coalesced replacement whose metadata
  lookup fails also releases the old buffer. Weak-reference regressions check
  actual pixel-buffer release; theme toggles continue to reuse the source cache.
- The image appearance preference had no direct search target or highlight.
  Settings now indexes and anchors the checkbox itself, including plural `images`.

The native journey harness now serializes concurrent evidence records, rejects
non-finite independent-divider observations, and returns to form mode before a
Settings-section search. Copy checks no longer expose unexpected clipboard text
in failures or retain successfully copied fixture data. Copy/paste adapter
restoration is acknowledged and cleanup has bounded terminate/kill/reap paths.
Deterministic contracts reproduce the evidence-log race, invalid-geometry false
pass, JSON/form search gap, and adapter timeout leak. These changes add no default
runtime profiling or idle repaints. No latency benchmark or cross-platform
performance guarantee is implied by this audit.

The expanded optimized native journey then exposed an additional real regression:
Tinymist's Preview-only child view covered Explorer's resize edge. Its drag input
was posted at a hovered resize target, but Explorer stayed at width 320; retained
`ui.preview.bounds` lines showed the native child starting at that same x=320.
`run-xh5qtj2k` failed and is not counted as a pass. Shared clearance selection now
uses the actual rendered split placement and visible neighboring panels, including
forced image/PDF split views. Popout previews inherit neither Explorer nor bottom
panel clearance from the document window. Deterministic tests reproduced both the
missing Preview-only clearance and the unintended popout bottom inset before fixes.

The same ownership review found that a designated source preview alongside an
active image/PDF tab could be rendered both in the main window and in its
popout. Content classification now explicitly accounts for detached ownership:
the main window retains the active source/asset while the detached window alone
renders the designated preview. A regression covers every view mode and document
kind; the image journey checks repeated frames during popout and restoration.

The first final native attempt (`run-x_t4frfe`) stopped at preflight because the
Mac was locked. It is unavailable interaction coverage, not a pass.

Final deterministic verification passed serially: formatting, strict all-target
Clippy (normal and desktop-ui-tests feature), 1,604 Rust tests (44 opt-in tests
ignored), 17 xtask tests, and 20 Python harness contracts. The optimized native
feature build also completed. The pending native command is:

```sh
caffeinate -di python3 scripts/test-desktop-ui.py --journey explorer_toggle --release --trace-preview --capture-review --review-preview
```

The desktop remains locked despite available Accessibility/input permissions.
No native interaction pass or fresh visual verification is claimed for this
follow-up until an unlocked session runs Explorer, images, git_hunks and
context_menu. The user-owned todo.typ/todo.pdf edits remain untouched.


## Newest table and panel todos, 2026-10-08

Items 24-30 and the table workbench steps 31a-f are implemented. Table workbench
31g has checked-in Typst/TeX fixtures and native journeys; its native verification
is still open. Original todo descriptions were preserved, and pre-existing
checkbox/PDF edits were excluded from this implementation commit.

- Preview Find targets the focused source or asset viewer and opens/focuses its
  controls, including a detached viewer. Controls, popouts and queued menu actions
  carry document ownership; obsolete child input cannot mutate another viewer.
  Held dragging uses actual pointer displacement and does not accumulate motion
  while the pointer remains stationary.
- Terminal padding is removed at its top, bottom and right. Problems has shared
  hover/focus outlines and exclusive keyboard ownership. Sticky editor rows retain
  the editor outline; current/error/warning backgrounds fill the viewport width.
- Inline/display math conversion queues one formatting request by default, with
  a Settings toggle. Changed documents and obsolete formatter replies are rejected.
- Typst tables support independently collapsible inspectors, exact option
  expressions, highlighted cells, literal fill/stroke/alignment/inset painting,
  selectable rules and column-header dragging. Writing column ratios is opt-in.
  Dynamic cells, computed options and unsupported partial structures retain a
  highlighted exact-source editor rather than being guessed or discarded.
- TeX tabular/tabularx/longtable grids preserve compatible column specifications,
  packages, placement, row heights, rules and comments. Unsupported structures
  use source mode. Invalid structural drafts disable Apply but remain repairable
  in Source. Apply checks the document key and exact original range, then performs
  one undoable edit; Cancel discards the draft. Repeated Apply cannot add newlines.

Failing deterministic regressions were established for preview ownership,
obsolete child commands, Problems commands, child-window commands and TeX Apply
stability before their fixes. A final independent read-only review found no
additional actionable defects.

Verification on macOS 14.6.1, Apple Silicon:

- Formatting and strict all-target Clippy pass for the normal build; strict
  Clippy also passes with `desktop-ui-tests`.
- Normal full suite: 1,674 passed, zero failed, 44 opt-in tests ignored.
  Feature full suite: 1,679 passed, zero failed, 44 ignored.
- 17 xtask tests, 29 Python harness contracts and two opt-in real PDF/compiler
  integrations pass. Both Typst and all three TeX manual fixtures compile.
- The optimized gallery captured and decoded all 25 expected PNGs in one session.
  Fresh table workbench normal/narrow and sticky-context PNGs were also captured
  under `.tiptoptyp/screenshots/agent-review` and inspected, together with the
  fresh terminal/Problems gallery images. These are viewport framebuffers;
  they do not prove native composed-window placement.
- Table parsing/rule validation is cached, TeX grids render visible rows only,
  stationary dragging adds no repaint loop, and math formatting issues one
  request per conversion. Deterministic tests cover those bounded-work rules.
  No comparative latency measurement or cross-platform speed claim is made.

The required real-app input check remains unavailable. The command
`python3 scripts/test-desktop-ui.py --journey tables --release` stopped before
input at preflight: Accessibility and input permissions were available, but the
Mac was locked and the desktop unavailable. Its explicit failed/unavailable
receipt is `.tiptoptyp/desktop-ui-tests/run-xr0z30ma/result.json`. A subsequent
privileged preflight reported the same condition. No native pass is claimed.
Pending unlocked runs are `tables`, `preview_find` and `problems_focus` (use
`--release`; add `--trace-preview` for the preview journeys). Keep this branch
unmerged until those interaction checks and the previous audit's pending native
checks are completed.


## Audit of the newest table/panel implementation, 2026-10-08

The follow-up audit reproduced and fixed these defects from 1025e4e:

- Typst grid cell text could close its own content/table and introduce a second
  table; style fields could inject unrelated options or cell spans. Apply now
  validates the complete cell wrapper and exactly one named option. Incomplete
  drafts remain available for repair; nested markup, lambdas and dictionaries
  retain their valid expressions. Both negative regressions failed before fixes.
- TeX `\tabularnewline` lost its terminating trivia, concatenating the command
  with the next cell and preventing valid grid Apply. Row endings now retain
  whitespace and comments, including optional heights; repeated Apply is stable.
- An oversized TeX row allocated a String for every top-level alignment delimiter
  before rejecting its width. It now stops at the first extra delimiter and
  preserves the complete exact source fallback. The matched optimized allocation
  measurement is checked in at
  `docs/performance-results/tex-table-row-audit-2026-10-08.json`: 876,573 to 80,333
  bytes for the same 40,033-byte fixture. This measures allocation, not latency.
- The Terminal command requested egui focus while Tinymist retained native
  keyboard ownership. Terminal and Problems now share the native parent/keyboard
  handoff, clearing preview and Explorer ownership. Native-menu and physical-key
  semantic regressions cover the change without modifying the source.
- Closing a pinned source left its controls action able to zoom an unrelated PDF.
  Invalid owners/actions are now retired before selecting the next closed default.
  A real tab-switch/close regression covers this, alongside positive first-PDF
  shortcut and toolbar checks.
- Eight terminal pane buttons expanded a user's 120-point panel to 307 points.
  The action strip now scrolls/clips inside its own column. Regressions retain the
  height across twelve frames and reach the last pane/new action with real wheel
  events. The normal-height terminal gallery fixture now contains eight panes.

An independent review additionally caught two native-journey setup defects. The
first backend closed the last terminal, leaving the second backend unable to
claim input; preparation now creates an isolated pane before focusing the preview.
A repeated-backend harness regression failed before the fix and also covers an
initially-off terminal. The standalone Problems/focus journey is now subject to
the same `/bin/zsh`/ZDOTDIR startup isolation guard as the other terminal journeys.
Its preview-to-Terminal transition types a physical `t` and writes an isolated
shell receipt without clicking the terminal grid after the shortcut.

Formatting, strict all-target Clippy (normal and desktop-ui-tests builds),
1,683 normal Rust tests, 17 xtask tests and 30 Python harness contracts pass;
44 opt-in Rust tests remain ignored. Apply-only validation and the bounded
terminal action strip introduce no workers or idle repaint loop.

The optimized gallery freshly captured and decoded all 25 expected PNGs in one
session. The eight-pane terminal gallery image and a separate fresh capture under
`.tiptoptyp/screenshots/agent-review` were inspected: the action strip stays
clipped inside the chosen panel height without overlapping the terminal grid.
These are viewport framebuffers, not proof of native composed-window placement.

The native command
`python3 scripts/test-desktop-ui.py --journey problems_focus --release`
stopped before interaction at preflight because the Mac was locked. Its explicit
unavailable result is `.tiptoptyp/desktop-ui-tests/run-u2kufq4l/result.json`;
Accessibility/input permissions were available. Native interaction and composed
window placement are still unverified, and the branch remains unmerged.


## Unlocked native follow-up, 2026-10-08

The first unlocked Problems run (`run-xv29mxpq`) passed PDFium ownership and
physical terminal typing, but exposed a journey preparation error on Tinymist:
Cmd+5 correctly reopened the remembered Terminal tab after the preceding handoff.
The journey now prepares Problems and closes it before focusing the preview.
A failing harness contract covers all three remembered tabs and both backend
phases; no compensating click is permitted after the tested shortcut.

Native table editing (`run-urj0r2ot`) passed Typst cell Cancel/Apply/Undo/Redo but
failed source repair. Native preview Find (`run-bn9k4aku`, reproduced for diagnosis
as `run-541oqqd3`) retained only the final seven-character chunk of a 23-character
query. Same-pass semantic preview and table regressions reproduced those lost
prefixes; prepending older parent commands corrected that ordering but was not a
complete native fix. The next optimized run (`run-d0bka9j2`) still lost the prefix.
An alternating child/parent regression exposed the second cause: TextEdit had
already painted Cmd+A, but its retained child InputState let the parent consume
and queue it again. A following parent pass repeated the synthetic command too.
Popup edit input must be normalized on its owning child pass, and injected
commands must retire after that field paints. Table draft observations expose
only byte length, a fingerprint and cached validity, with no contents in logs.

The focused review also reproduced reversed text/shortcut ordering in Settings,
root Explorer search commands arriving after typing, and retired preview controls
retaining queued field commands. Current physical shortcuts keep their positions
among text events; older menu commands precede subsequent native input. Preview
controls clear queued input when closed or retargeted. These changes introduce no
workers, timers, idle repaints, or default-runtime diagnostic logging.

The Explorer run `run-lfosy238` reached the affected native composition review
boundary, then failed because the computer-use observation tool stalled beyond
its three-minute acknowledgement window. It is not an interaction pass or visual
verification. Composed-window visual review remains unavailable through that
failed observation. Subsequent automatic native runs retain geometry traces and
report interaction results separately from framebuffer captures.

The Problems-only content outline was corrected to one layout-neutral outline
around the entire bottom panel, including the tab row, shared by Problems,
Terminal and Activity. A failing header-hover regression established the missing
boundary before the fix. The all-tab regression checks outer bounds, repeated
header focus, stable panel height, no source typing and no nested Problems
outline. Activity and empty Terminal now have noninteractive keyboard owners;
controls keep their own hit targets. This adds one visible-panel stroke query,
no workers, timers, idle repaint loop or panel padding. The native Problems
journey also checks Activity ownership and can capture all three focused tabs.

Terminal's raw input consumer also intercepted the host screenshot shortcut. A
failing regression established this for the grid and its Find field. The host
capture binding now runs first, including rebound and disabled variants, without
changing terminal input or source contents. Table-specific cell selection chords
are handled before generic editing normalization, retaining their existing grid
behavior; alternating child/parent regressions verify that editing commands retire
after their field paints.

The expanded native runs found two additional preparation errors. Native Open
selected the TeX fixture's directory rather than its file; it now selects the full
file path while retaining the original document owner and exact-path assertions.
Git/context-menu math conversion expected raw output with the default formatting
preference enabled. Those exact-source journeys now disable the preference through
Settings, verify its effective value, and restore it in a `finally` block. The
source fingerprints and single-Undo assertions remain strict. The 34 Python
contracts cover both preference defaults and failure restoration.

Native Explorer transitions (`run-b2rmjepc`) reproduced a real PDFium interaction
conflict: in Preview-only mode, a full-width page took the exact shared Explorer
resize boundary, leaving the width unchanged after a 20-point drag. PDFium now
clips only its page interaction to exclude the actual current-pass resize widgets.
The page's paint rectangle, focus bounds, coordinate mapping, scroll layout and
fit-width calculation remain unchanged. Hidden, disabled, fixed and detached
handles reserve nothing. The three handle IDs are queried once per visible PDF
view, then reused for its resident pages; there is no widget re-registration,
allocation proportional to document length, or new repaint loop. All 16 divider
tests pass, including Explorer/editor/bottom exact-edge resizing, ordinary page
clicks, stale-handle removal, widget/bounds preservation and Tab/Shift-Tab order.

Optional startup review capture also had no established editing first responder
(`run-5728lx4m`). Its preparation now explicitly claims and verifies source
keyboard focus; per-state bottom-panel captures preserve their tested owner.
A Settings preparation attempt (`run-nm6dhf7o`) produced no query after a posted
click, but had no intervening observation to classify when focus was lost. The
search geometry remained stable and the next native run passed. The harness now
requires a fresh, enabled, focused search target after its single click before
sending keyboard input. Negative contracts reject unowned, disabled and stale
targets. There are no compensating clicks or relaxed source/backend assertions;
this receipt does not establish a separate Settings architecture bug.

Final deterministic validation: formatting, strict all-target Clippy for normal
and desktop-ui-tests builds, 1,696 Rust test executions, 17 xtask tests and 40
Python harness contracts pass. Forty-four opt-in Rust tests remain ignored.
The optimized gallery captured all 25 expected PNGs in one app session, and its
final validation decoded all 25. The updated terminal gallery image was inspected.
Fresh focused Problems, Activity and Terminal framebuffers from `run-91cetuzr`
were inspected and retained under `.tiptoptyp/screenshots/agent-review`; their
outline encloses tabs and content, with stable height and no nested list outline.

The final optimized native binary is
`tiptoptyp Dev 0.1.0 (209ffcf22f9e-dirty.1791455266)`, SHA-256
`5dcdfab016840adfce579cf8f804aeed33da169c1604e53a1c7c936b768219a5`.
The runs verified the running build and effective backend and passed real input:

| Journey | Successful receipt |
| --- | --- |
| Problems / Activity / Terminal focus | `run-91cetuzr` |
| Preview Find | `run-8mgnla5p` |
| Typst and TeX tables | `run-ltw8gvze` |
| Explorer transitions / dividers | `run-zkwkg65e` |
| Images | `run-jjeu2489` |
| Git hunks and diff editing | `run-231r0ch3` |
| Code context menu | `run-plhfulp2` |

Problems/focus, Preview Find and Explorer verify both effective PDFium and
Tinymist. The focus journey includes actual isolated shell typing after the
preview-to-Terminal shortcut. Explorer retains 1,239 `ui.preview.bounds` traces,
including `available=(873.6,30.0)-(1400.0,860.0)`, clipped/native
`(877.6,30.0)-(1400.0,860.0)`, viewport `(0.0,0.0)-(1400.0,884.0)` and
`egui_ppp=native_ppp=2.000` in its final restored state.

These automatic native input results and viewport framebuffers are separate
evidence. Tinymist's composed desktop placement was not visually certified:
its native child is absent from the root framebuffer, and the earlier whole-window
observation failed. Geometry traces and interaction receipts do not replace that
visual check.

The stricter final Explorer repeat `run-35chakzt` also passes all transitions,
including focused Settings search readiness, effective PDFium and Tinymist,
independent dividers, and closing the nearly maximized bottom panel. It used
`tiptoptyp Dev 0.1.0 (209ffcf22f9e-dirty.1791456583)`, SHA-256
`7b9347ae4859ed322fc3708aab4226d00bb1be9735530d7f25738b26442990d9`;
the app source was unchanged from the seven successful runs above.

The preceding repeat `run-io77tklx` passed the backend/divider transitions but
failed its final bottom-panel close click. The root lost native focus after the
posted click; the close target did not overlap a resize handle or preview, and
no tooltip child viewport was created. The desktop remained unlocked, but that
receipt did not identify the foreground application, so the cause remains
unclassified. An AX-focused window alone does not establish that its process is
frontmost. The passing repeat does not erase this failure or prove its cause.
`run-48ecbpth` was unavailable at locked-desktop preflight and is not a pass.

Failed native interactions now retain bounded, read-only foreground PID/bundle,
lock/permission state, and native-window observations before cleanup. Collection
never activates the application or retries input, and cannot replace the original
failure. Two negative contracts establish these properties, bringing the Python
contract suite to 40 passing tests. The final repeat compiled and exercised the
updated Swift observer as well as the unchanged optimized app binary.


## Local-definition uses — 2026-10-09

The former definition-site decoration did not match the intended behavior.
Exact-position regressions first failed for Typst and TeX: declarations were
marked instead of uses. Settings now exposes **Highlight uses of local
definitions** (`highlight_local_uses`), with matching search, Help and JSON
behavior. Uses follow local Typst scopes, parameters, loop/destructuring bindings
and set/show customizations, or lexical TeX macro/environment declarations.
Declaration targets, comments, literal code, unrelated member names and argument
labels retain ordinary syntax highlighting. Imported definitions and dynamic
TeX expansion are outside this source-local feature.

The editor reuses its incremental Typst syntax tree. Decorated jobs and byte
ranges remain cached across unchanged frames; code-mode offsets participate in
cache invalidation. Tests cover identical bytes switched between code/markup,
Unicode positions, namespace shadowing, global TeX declarations, malformed
literal delimiters and non-TeX generic files. The full document diff shares the
same setting and complete-source highlighting path. Partial Typst declarations
keep their targets unmarked while valid unfinished expressions retain their
uses. Import aliases shadow earlier local names even when explicit imported
items accompany the alias; exact-range tests cover both cases.

Final verification: 1,775 Rust tests passed (44 opt-in tests ignored), 17 xtask
tests and 49 Python UI-harness contracts passed; formatting and strict Clippy
passed for ordinary and desktop-test builds. Native `editor_preferences` passed
in `run-fdrc8k1y` using real macOS clicks, keys and scrolling, including the new
setting and unchanged document source. No framebuffer was required or captured;
this is behavioral evidence, not a visual-composition claim.

The optimized, interleaved source-change scanner comparison and limitations are
recorded in `performance-results/local-use-highlights-2026-10-09.json`. Cached
frames perform no additional scan; the measurement excludes layout and native
rendering. User `todo.typ` and `todo.pdf` contents were preserved.

## Buffered Tinymist mode acceptance (9 October 2026)

The separate experimental backend preserves the original Tinymist default.
The actual app journey `partial_preview` passed in
`.tiptoptyp/desktop-ui-tests/run-sy9mviyc`, using debug build
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791556459)`, SHA256
`1f95c4d6ce77b34db7bcdcc786b9a449d9f1fb4933ae575f5bed442be030be9a`.
Accessibility/input permissions were available and the desktop was unlocked.
This is native-input/read-only-state evidence, distinct from the optimized
standalone frontend performance comparison.

The 48-page fixture checked actual DOM partial mode and visible ink after
End, Outline return, four wheel transitions, zoom/fit, and reopening partial
mode. Populated pages stayed between 4 and 6 while 1–2 pages were visible; no
fallback canvases appeared. Switching to the original mode required a fresh
ready DOM with partial disabled and all 48 pages populated. The existing
three-page full-mode browser integration also passed
(`/private/tmp/ttt-partial-original-e2e.log`).

The first native attempt exposed an observation bug: readiness tested the
first page's ink bounding box, even when that offscreen page was intentionally
empty. The corrected probe checks initialized, visible page ink. Three Node
regressions cover offscreen first-page placeholders, visible missing ink and
uninitialized/loading modules. The initial failing app receipt remains
`run-7zlqmyu_`. The platform's synthetic Home event did not navigate the
WebKit scroller (`run-val6xlch`); the acceptance journey uses the actual
Outline control to return to the beginning. No Home behavior change is claimed.

The new mode changes SVG contents/scheduling, not native placement. Opt-in
app tracing retained, for example:

```text
ui.preview.bounds available=(838.5,30.0)-(1400.0,860.0) clip=(838.5,30.0)-(1400.0,860.0) egui=(842.5,30.0)-(1400.0,860.0) native=(842.5,30.0)-(1400.0,860.0) viewport=(0.0,0.0)-(1400.0,884.0) zoom=1.000 egui_ppp=2.000 native_ppp=2.000
```

Fresh standalone Chromium and WebKit PNGs were inspected for rendered ink.
They are not composed desktop captures, and no composed visual pass is claimed.
Required Rust checks passed: 1779 tests with 44 intentionally ignored, 17 xtask
tests, formatting and strict ordinary/desktop-feature Clippy. Adapter 23 Node
tests, observation 3 Node tests and 52 Python contract tests also passed.
The profiling-enabled Rust suite, strict profiling-feature Clippy and xtask
formatting check passed as well. See
[Buffered Tinymist preview](preview-partial-rendering.md) for the architecture,
reproduction commands and measurement receipt.


## Tinymist follow-up performance and shared full-mode optimizations (9 October 2026)

The optional partial mode now reuses one geometry snapshot per request and
acknowledged SVG ink for geometry-only changes with the same page buffer. A
normal patch remains mandatory for source updates, force requests, missing
associations and a changed buffered range. Exact-position regressions cover
far-page Ctrl-wheel zoom, fixed CSS page gaps and source repagination. The
normal patch fallback originally shifted the pointer's source point from
200 to 216.36 source units on page 18; the shared page-local restoration fixes
both paths. Final page-42 browser checks force the ordinary fallback and retain
visible ink with first/second-frame errors under 0.51 CSS pixels.

The exact black-on-white palette bypass and retained-link handler deduplication
also apply to full SVG document previews. Before the fix, initializing a
retained link 101 times attached 101 mousemove/mouseleave handler pairs; after,
it attaches one. New/replaced links, exceptions and unrelated preview modes
retain their required behavior. Full rendering bounds, source FIFO and Canvas
fallback are unchanged. Near-identity and dark/tinted/inverted palettes must
still have exactly one transfer filter. The native palette assertion now
recognizes the identity bypass in either mode, with negatives for tinted ink,
tinted paper and inversion.

Final adapter `06c0dba75e657f3806efaa7bc47274b0437e207291b3c7f47085bb515cdcbb33`
and stricter browser probe `de605d06540e9fbbe6de7b742704e84523b9fb772c02e13371e94e0d9bbea21e`
passed separate 48-page full and partial browser acceptance. Receipts:
`.tiptoptyp/profiles/partial-optimize-2026-10-09/strict-final-full-acceptance.json`
and `strict-final-partial-acceptance.json`. The checks exercise links/history,
source changes during scrolling, first-frame ink, resize/zoom, idle work, five
palette transitions, page clipping and decoded paper/colored-ink pixels.
Fresh dark and restored-white PNGs were inspected in both modes under
`.tiptoptyp/screenshots/agent-review/partial-optimize`. These are standalone
Chromium viewport captures, not composed native editor evidence. No native
bounds or maintained gallery contract changed.

Matched browser pairs record partial p95 gaps 33.4→16.8 ms and full gaps
199.9→183.3 ms. The native partial pair records 114→78 ms using the same optimized
Swift binary, backing scale 2, fixture/workload/reset and low power mode off.
It measured adapter `c7ba6f…` before the shared full-mode ports, not final
`06c0db…`. Total native workload time and rescale wall spans increased; do not
claim a CPU/overall-work reduction. Full sample ranges overlap substantially.
See `performance-results/tinymist-partial-followup-2026-10-09.json` for preserved
metadata, all sample summaries, selected raw receipts and interpretation.

The probe audit found that the installed Playwright poller treated async
predicates as truthy Promises, allowing unresolved false readiness to pass.
Source readiness and frame stability now use explicit bounded awaited RAF
observations. Older browser results remain separate from the selected matched
probe-`94320b…` pairs. A later review found that the first stability
observation did not need to contain fitted ink; both observations must now be
valid rendered page-18 states at equal viewport dimensions. Before-only
scale/page/dummy/size negatives cover this gap. Selected timing receipts still
assert matching final physical start states; they do not prove the strengthened
first-observation contract. Final acceptance was rerun with the stricter probe. Native instrumentation uses a single bounded, cancellable
RAF sampler; a stale in-flight callback cannot restart it. Idle checks now
reject rescale, rerender or viewport work. Probe source and binary hashes
must agree before reuse; the optimized build and Swift version are recorded.
A failed 90-second compile in `20261009T155415Z-98akhxri` was retained; build cleanup now terminates
remaining process-group descendants on timeout/interruption, with bounded
waits and a 240-second compile limit. Fresh compilation and reuse passed.

The real-app follow-up was attempted twice and did not pass. `run-6lg2dzjl`
failed while opening the fixture before preview checks: the native Open button
was unavailable, although the desktop was unlocked and permissions available.
Its cause remains unclassified; no production Open fix is claimed. The journey
now establishes source-editor keyboard ownership before issuing Open.
`run-cpohohk5` then failed initial document-window registration after the Mac
locked; failure evidence identifies `com.apple.loginwindow` and locked state.
The final read-only preflight still reports `accessibility=true`, `input=true`,
`desktop=false`, `locked=true`. The pending command is
`python3 scripts/test-desktop-ui.py --journey partial_preview`; this unavailable
interaction check is not a pass. Earlier `run-sy9mviyc` covers the initial mode,
not these follow-up changes, and is retained separately.

Final ordinary verification passed formatting, strict all-target Clippy,
1779 Rust tests (44 intentionally ignored), 17 xtask tests, 48 Node contracts,
53 Python UI-harness contracts, browser probe self-tests and native runner
comparability/phase/binary-reuse/build-cleanup self-tests. Final strict profiling
Clippy, the profiling-enabled suite (1787 passed; 44 intentionally ignored) and
xtask formatting also passed. User `todo.typ` and
`todo.pdf` hashes are unchanged by this work.


## Buffered Tinymist becomes the Typst default (9 October 2026)

`PreviewPreference::default()` now chooses `InteractivePartial`; AppSettings
uses that same default. Fresh/no-storage profiles and a missing preview-choice
field load the buffered viewer. Explicit stored full-Tinymist or PDFium choices
remain selected. The complete viewer stays available as a separate Settings
option. TeX and opened PDFs retain their PDFium routing. Built-in Help and
current backend documentation describe the new policy. Low-level service
options and deliberate full/PDFium QA fixtures keep their explicit choices.

Focused persistence tests cover absent storage, an empty store, omitted JSON
fields, and every explicit backend through both persistence and Settings JSON.
The semantic Settings selector still starts from explicit full mode to verify
that selecting partial emits an update. Toolbar tests cover the default partial
viewer, explicit full viewer and TeX under both Tinymist preferences; the opened
PDF test now starts beside the default partial Typst preview. No renderer,
worker, cache or repaint algorithm changed; no new performance benchmark is
needed for this selection-policy change.

The real macOS `partial_preview` journey passed in
`.tiptoptyp/desktop-ui-tests/run-nxohoeg1`, debug build
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791574389)`, SHA256
`742f6c7c87c9c71f5ddca729d4dc1c17bcb0e1ae4d15d9206a80f943e9961832`.
Accessibility/input permissions were available and the desktop was unlocked.
The journey used a fresh isolated settings profile, observed the actual partial
DOM before any backend change, then used native clicks/keys/wheel navigation
on the 48-page fixture. Initial/End/Outline/wheel/zoom/fit/reopened-partial
checks all retained visible ink, 4–6 populated pages and zero canvases. Switching
to full required actual full mode with all 48 pages populated. It restored the
initial session afterward. This passes the previously unavailable follow-up
native interaction check, without erasing the earlier unclassified Open and
locked-desktop failures. It is not a composed visual or performance claim.
No screenshots or placement changes were needed for the default-setting change.

Verification passed formatting, strict all-target Clippy, 1780 Rust tests
(44 intentionally ignored), 17 xtask tests and 54 Python UI-harness contracts.
The harness negatives reject wrong requested/effective backends and stale,
loading or actual-full DOM observations when checking fresh default adoption.
User `todo.typ` and `todo.pdf` contents were preserved.


## Fractional-page partial zoom audit (9 October 2026)

The reported tiny-only zoom symptom did not reproduce with Cmd+= / Cmd+- on the
installed app's mathematics document, or with repeated buttons in standalone
WebKit. Inspection confirmed that the installed app embedded adapter `06c0db…`.
A separate, reproducible partial-mode bug was found: compiled ISO paper sizes
are fractional, while the SVG exporter ceil-rounds paper dimensions. This
invalidated the geometry reuse checks on every zoom. The new deterministic
fractional-page regression failed before the fix and passes afterwards. Layout
now follows SVG dimensions; WASM render-window bounds preserve compiled units.

The browser probe now checks accumulated zoom against actual SVG scale and
requires a covered small zoom to retain ink. Its A4 transform extends the known
marker stripe to match the taller paper; pixel checks require marker color only
where that known stripe intersects the viewport. Blank paper and a horizontally
panned-off stripe are not reported as missing ink. Both full and partial lanes
passed sustained zoom, refill, source changes, navigation, and partial pointer
anchoring with the fixed adapter `f2895d…`. Fresh standalone browser captures
were inspected; they do not prove composed editor appearance.

The matched Chromium workload retained the same probe/fixture/sidecar hashes,
viewport, warmup and five fixed sample starts. Partial zoom-sequence patches
fell from 19 to 2; the timed resize/scroll/zoom workload's median renders fell
from 59 to 16. Median p95 frame gap remained about 16.8 ms. This is reduced
renderer work, not evidence of smoother native pinch gestures. The checked-in
[receipt](performance-results/tinymist-fractional-zoom-2026-10-09.json) records
all samples and limits, including the additional untimed after-only pointer
acceptance. Baseline was intentionally measure-only because the reuse contract
fails. Physical trackpad pinch input has not been reproduced by the harness.

Formatting, strict all-targets Clippy, 1,780 Rust tests (44 ignored), 17 xtask
tests, 50 JavaScript contract tests, and 55 Python harness checks passed.
The expanded real-app `partial_preview` journey verifies twelve native button
steps, four native keyboard zoom steps and fit against fresh visible-paper
geometry on 48 fractional A4 pages; its execution receipt is recorded below.


The optimized native run passed: `.tiptoptyp/desktop-ui-tests/run-m8tiph7t`,
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791580715)`, binary SHA-256
`471471ec2e79969809fe1ee0a18d4e06d1323962f885c48fb1bf9f2c983f1ffd`.
Accessibility/input/desktop checks were all available and the desktop was
unlocked. The run confirmed the actual partial default, all repeated native
zoom steps against paper scale, fit, distant coverage, full mode with 48/48
populated pages, and reopened partial mode. The isolated profile/session was
restored and the process exited successfully. This is native interaction
coverage, not a composed-window screenshot or physical pinch test.


## Sustained zoom and dense mathematical fixture (9 October 2026)

The user identified trackpad pinch as the original input and confirmed that the
preceding fractional-page fix resolves it. Physical pinch remains distinct from
the harness's synthetic Ctrl-wheel events.

A failing deterministic performance test recorded two physical layouts for
each covered partial zoom frame and repeated cursor searches through unchanged
ink. Covered zoom now applies scale, paper positions and pointer anchoring in one
layout, and cursor discovery occurs after patches. The first draft deferred zoom
before anchoring a normal fallback fetch; matched measurements exposed extra
patches. A second failing regression required the first fetch to target the
anchored destination. That ordering is fixed and all pointer/coverage/source
queue invariants pass. Detached cursor decorations force a normal patch and are
rediscovered; clearing cursor paths flushes the cached list.

A full-viewer performance regression recorded 1,000 paper-transform reads on a
1,000-page synthetic document. Full page tracking now binary-searches the first
intersecting paper and visits overlapping papers only. Tests compare it with a
complete reference scan across unequal heights, distant pages and zoom ratios
from 0.1 to 10. Full rendering's source/patch/canvas behavior is unchanged.

The new opt-in `tinymist-zoom-stress.typ` fixture compiles to 48 pages, including
672 dense matrices and nested estimates, with unequal fractional paper widths.
Its initial partial SVG has 52,636 nodes and some buffered positions exceed
130,000; the earlier fixture had about 6,700 initially. Browser/native stress
probes preserve coverage/ink/first-frame/idle checks and use a separately recorded
320,000-node cap; the fast fixture retains its original 30,000-node cap. Both
before and after adapters pass those contracts.

Matched optimized WebKit runs (same native binary, three warmed samples, 900×700
points at 2× backing scale) reduced the ten-step zoom phase from a median 5,062 ms
to 2,905 ms and its rescale calls from 22 to 12. The full workload median fell
from 9,173 ms to 7,119 ms; p95 RAF gap fell from 508 ms to 288 ms. Worst zoom
steps remain around 0.5 seconds. Five matched Chromium samples have essentially
unchanged combined p95 gaps around 217 ms. Inclusive wall spans are not CPU time.
The checked-in [receipt](performance-results/tinymist-zoom-stress-2026-10-09.json)
retains the measurements and their limits. Fresh standalone captures were
inspected for paper/ink coverage; they do not prove composed editor appearance.

The initial dense-full native check lost page ink after keyboard zoom. Separate
standalone full-mode runs with both the preserved pre-optimization adapter and
the current adapter exceeded the same 125-second probe budget. A two-second
native CPU sample of the baseline WebKit process found SVG geometry/compositing
work during scrolling; the process was CPU-bound with roughly 4 GB resident
memory. Dense full rendering remains a failed capacity check, not a pass. The
stress journey now explicitly uses the dense fixture for partial mode, the
standard 48-page fixture for full zoom/last-page smoke checks, then returns to
the dense partial entry. Both failure receipts remain in the measurement record.

The larger journey also exposed test-driver traversal of the unrelated WebKit
accessibility tree when pressing a native Open button. The driver now skips
web areas and matches native button roles. Two-fixture cleanup explicitly closes
fixture tab identities: an MRU transition to an original tab can no longer make
the harness close that original tab. A deterministic regression covers that
selection sequence and restores the original active/preview/backend identities.

The final optimized native journey passed in
`.tiptoptyp/desktop-ui-tests/run-8ehs8c0k`, version
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791584816)`, binary SHA-256
`ec20b2be8148923c05793aae52ace2d3b116eddb019a36381a0fb4316e00898f`.
Accessibility/input/desktop checks passed with the desktop unlocked. It checked
26 fresh partial coverage states, twelve native button zoom steps, four partial
keyboard zoom steps, fit, distant refills, three full-viewer keyboard zoom steps
on the standard fixture, full last-page tracking, and dense partial restoration.
The original tab, preview entry and backend were restored. This is native input
and read-only geometry evidence, not a composed-window screenshot or physical
pinch automation.

Required formatting, strict all-target Clippy, the full Rust suite (1,780 passed,
44 ignored), and xtask tests (17 passed) passed. Adapter/renderer probe tests
(53), desktop harness contracts (56), both browser-free fixture-bound checks and
the native probe self-tests passed. The full-renderer browser integration passed
with its obsolete identity-filter expectation corrected; exact white/black uses
no SVG filter while tinted/inverted palettes retain the filter and pixel checks.
No Rust profiling instrumentation changed in this task. The steady idle contract
remains unchanged; heavy active rendering measurements above are separate from
idle, cold startup and last-input-to-final-paint latency.

The user's cancellation idea is recorded in the partial-rendering design notes.
Queued viewport inputs already coalesce to the newest request; the pinned
synchronous SVG patch cannot be interrupted by an event that has not run yet.
A future interruptible scheduler should retain visible ink and ordered source
patches, cancel obsolete viewport work at safe checkpoints, publish only the
latest refill, and measure the last gesture input to the final correct frame.


## Idle viewport image for Tinymist zoom — 10 October 2026

Smooth Tinymist zoom is default-on for macOS and selectable in Preview settings.
After 150 ms of settled idle, one bounded viewport capture is decoded. Zoom
reuses that exact image element, cancels pending idle timers and rejects earlier
in-flight captures, including gestures which return to their starting geometry.
It never captures or decodes on demand. The native capture is capped below four
million pixels; TIFF-to-PNG encoding runs on one background worker per live view.
Source, palette, scroll and view changes invalidate old pixels. Disabled and
unsupported paths retain normal zoom without extra paint frames or image work.

The native journey exposed two additional bugs before handoff. Full Tinymist did
not schedule an initial idle capture after its asynchronous final completion;
now the cache observes that completion without a polling loop. Native End could
remain at the first page after full zoom/fit; both viewers now handle Home/End
explicitly while preserving modified keys and text-input ownership. Keyboard
appearance inversion also invalidates cached pixels immediately.

The final optimized real-app `partial_zoom_stress` journey passed in
`.tiptoptyp/desktop-ui-tests/run-9f4_62e7`, version
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791619904)`, binary SHA-256
`70aa83b8bff53b4a1d4ae640d8ede78251db809bcea76a2dd6e7e35a9dd2edce`.
It checked 26 fresh coverage states, settings toggle/release/reenable, and new
keyboard cache hits/handoffs separately in the partial and full viewers. The
actual native cache images were 1,618×2,408 pixels. The run restored the original
session and retained preview bounds traces. An earlier locked run is recorded
as unavailable, not passed; a temporary keep-awake assertion protected final
runs without changing the Mac's permanent sleep settings.

Matched optimized standalone WebKit runs used the dense 48-page fixture,
900×700 points at 2× backing scale, three-second warmup and three measured samples
per setting. Median ten-step zoom was 4,052 ms disabled versus 3,520 ms enabled;
all 30 enabled inputs used the existing image. The final sharp DOM/RAF handoff
was 415 ms disabled versus 447 ms enabled, and worst zoom RAF gaps remained near
0.67 seconds. This does not claim faster final rendering, physical pinch input
or final GPU-presentation latency. The Swift probe uses Cocoa PNG encoding;
the real app's Rust worker was verified by the separate native journey.

A fresh cached-transition PNG under
`.tiptoptyp/screenshots/agent-review/partial-render-native/20261010T081856Z-camzvied`
was inspected for page edges, clipping and retained ink. It is a standalone
viewport visual scene, not proof of composed editor pixels. The
[checked-in receipt](performance-results/tinymist-zoom-cache-2026-10-10.json)
retains current code hashes, warmup/workload metadata, all measured samples,
rejected experiments, native cache states and geometry trace examples.

Required formatting, strict all-target Clippy, the full Rust suite (1,785 passed,
44 ignored), xtask tests (17 passed), focused JavaScript tests (63 passed), and
desktop harness contracts (56 passed) passed. The macOS-only checkbox assertion
is gated for other platforms. No Rust profiling instrumentation or maintained
framebuffer-gallery contract changed.


## Native isolation of cached Tinymist zoom — 10 October 2026

The earlier DOM-image overlay shared Tinymist's WebContent JavaScript thread.
Its roughly 0.67-second browser-frame stalls explain the reported choppiness;
a background PNG encoder did not isolate gesture transforms. That implementation
is replaced by a native retained CGImage in a clipped Core Animation layer.
AppKit gesture/key input transforms the layer on the app's main thread, separate
from WebContent, and the compositor scales it without per-input pixel resampling,
PNG/base64 transfer or browser decoding. Capture still starts only after 150 ms
of settled idle, with one in-flight capture and fewer than four million pixels.

Vector rendering keeps one request in flight and one latest replacement. Pointer
anchors compose across intermediate inputs, including a round trip with residual
pan. Exact revision/scale/token acknowledgements retire the image only after the
latest sharp geometry is ready. Source/palette invalidation retires pixels without
abandoning the final requested zoom; hiding/disabling aborts it. Bounds changes
scale the native image without adding duplicate renderer requests. Cold-cache
zoom uses the bounded renderer path. Local monitors retain the exact owner window
and focused WebKit responder; sibling settings/editor keys pass through.

The optimized native component regression deliberately blocked WebContent for
800 ms and posted four owned AppKit Cmd+= events during that block. Three matched
runs per lane used the same binary, 640×420 point viewport and three-second warmup,
without concurrent builds or test workloads. Browser-path input responses were
517–699 ms (median 608 ms); native layer-transform submissions were 0.11–0.44 ms
(median 0.16 ms). All twelve native inputs reused the idle image while WebContent
was blocked. Each run coalesced four inputs into two vector requests, reached
1.1^4 scale and completed handoff. Negative responder ownership and hide/release
checks passed. These are submission timings, not GPU presentation or dense-file
final-render latency; vector completion is mocked in this isolation fixture.

The final optimized real-app `partial_zoom_stress` journey passed in
`.tiptoptyp/desktop-ui-tests/run-bimbqtl8`, version
`tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791654976)`, SHA-256
`75d56ccd704545982f2441c07d59542303c0a0eb5b54444c355eac2ce15d4793`.
The effective partial and full backends were verified. It checked 26 fresh
partial coverage states, twelve native button zooms, four partial and three full
native keyboard zooms, native raster hits in both modes, latest sharp coverage,
fit, distant refill, settings disable/reenable and original-session restoration.
The dense 48-page fixture exercises partial rendering; the standard 48-page
fixture exercises full rendering. The native rasters were 1,618×2,408 pixels.
Relevant `ui.preview.bounds` traces and input/renderer receipts are retained.

Three fresh native-layer PNGs under
`.tiptoptyp/screenshots/agent-review/zoom-isolation/20261010T193803Z` were inspected
for upright retained ink, scaled text and viewport clipping. They render the owned
CALayer during the blocked interval, not the composed editor window. The separate
Cua app-observation call stalled until the temporary app had exited, then returned
file-not-found; whole-window visual composition is unverified. Physical trackpad
pinch was not automated. A preliminary locked component run failed handoff and
had incorrect child bounds; it is excluded from the final matched measurements.

Formatting, strict all-target Clippy, the full Rust suite (1,789 passed, 44 ignored),
xtask (17), JavaScript regressions (64), desktop harness contracts (56), and native
probe self-tests passed. The external cached test runner stalled; only the owned
processes were stopped and the complete suite was rerun with the repository's
standard runner. No Rust profiling instrumentation or maintained theme-gallery
contract changed. The old Swift bitmap-provider copy was removed so it cannot
misrepresent production native input isolation.

The [measurement receipt](performance-results/tinymist-zoom-native-2026-10-10.json)
contains all six successful component runs, source/build hashes, power/environment
metadata, the native journey states and bounds traces, and unavailable checks.


## Buffered native zoom, opaque boundaries and divider feedback — 10 October 2026

Cached gestures now defer Tinymist vector work until native pinch end/cancel or
120 ms of quiet; a one-second watchdog handles a missing pinch-end event. One
native timer, one vector flight and one latest replacement bound the work. The
same path scales the image during divider changes and restores the source point
at the viewport top. Identical bounds do no work.

Idle capture includes half a viewport on each horizontal side and a full viewport
above/below, with page backgrounds and referenced glyph/palette definitions. A
passive offscreen WKWebView renders the inert copy without moving the live SVG,
activating the app or taking focus. Scripts/external resources are disabled and
its data store is ephemeral. One capture and the existing four-million-pixel cap
remain; the larger buffer can reduce density. An opaque sRGB clip background
blocks the live PDF beneath exposed edges. A native regression first exposed
GenericRGB's 238→241 color mismatch; using sRGB removes that seam.

The old unconstrained photo transform moved a zoomed-out first page down 100 px
in the regression fixture. The new transform clamps to SVG scroll limits and
centers below-fit paper. Full mode gets an absolute scroll target; partial mode
gets page-local/gap-fraction anchors so fixed-size gaps do not shift its final
content point. Tests cover end limits, gutters, moving anchors and mixed
zoom/resize coalescing. Cached page metadata is reused, not allocated on every
viewport observation.

Final formatting, strict all-target Clippy, Rust (1,798 passed, 44 ignored),
xtask (17), JavaScript (76) and harness contracts (56) passed. Actual bundled
Tinymist frontend checks passed for full and partial mode, including bounded
nonempty paper copies, glyph dependencies, unchanged live source/scroll geometry,
and light/dark palette propagation. The isolated native regression passed with
activation prohibited: all four owned zoom keys moved pixels during an 800 ms
WebContent stall, sent one final vector request and handed off. Zoom-out opacity,
horizontal midpoint, four child-width changes, one final resize request, retained
keyboard ownership and release passed. The fixture mocks vector completion;
its debug submission timings are not a release/dense-document speed comparison.

Fresh browser-buffer and isolated native clip-layer PNGs under
`.tiptoptyp/screenshots/agent-review/zoom-buffer` were inspected for retained ink,
clipping, opacity and horizontal alignment. The isolated clip's root coordinate
system differs from its host; it does not prove screen-space vertical placement
or the composed editor transition. No maintained framebuffer/gallery contract
changed.

The release baseline completed, but both deferred comparison attempts lost
foreground input ownership to Codex; the missing events never advanced native
revision. Those runs are excluded. The user then declined an idle interval, so
final whole-app `partial_zoom_stress` (including newly added divider drags),
composed native bounds/visual confirmation, physical pinch and final paired timing
remain unavailable. No 10× or final-render speedup is claimed. The
[buffer/defer receipt](performance-results/tinymist-zoom-buffer-2026-10-10.json)
retains source hashes, raw component observations, checks and exclusions.

Final review also found and reproduced a cold-start resize race: the JS helper
could claim a resize before the native cache was initialized, then wait for an
acknowledgement that never existed. It now claims resizing only with an admitted
image or an already active native request. Uninitialized/cold views retain normal
Tinymist sizing. The isolated native fixture also receives real browser resize
events, and deterministic coverage verifies startup and active ownership.


## Cached zoom at document edges and image resource bounds — 10 October 2026

The real pinned Tinymist frontend reproduced a 235-pixel partial-layout height
error at half zoom near the end of the 48-page dense fixture. The cached transform
scaled the fixed 10-pixel page gaps. A failing bottom-edge geometry regression
preceded the fix; the new model scales page ink separately from gaps and computes
the same bounds and page-local/gap-fraction anchor as the actual frontend. Resizing
also preserves current horizontal scroll instead of restoring capture-time scroll.

Native image layers now submit only visible intersections and normalized texture
rectangles. Partial page slices share one immutable CGImage; offscreen slices are
hidden and pages outside the bounded capture do not allocate layers. A 10,000-page
regression allocates only three slices. Per-input forced Core Animation flushes
were removed. After capture, the reused passive WKWebView loads a blank document
and shrinks to 1×1; navigation identity rejects callbacks from an older/parked load.
Retained image pixels remain usable through subsequent zoom and recapture.

Formatting, strict all-target Clippy, Rust (1,804 passed, 44 ignored), xtask (17),
JavaScript (76) and desktop harness contracts (56) passed. The headless real
full/partial edge probe passed all 24 cases. The latest native own-window regression
passed mock opacity/centering/resize checks and actual full/partial Tinymist checks,
asserting the effective backend, a parked 1×1 capture view, image updates during
an 800 ms WebContent stall, one final vector request and sharp handoff. No global
input or application activation was used. Fresh native clip PNGs under
`.tiptoptyp/screenshots/agent-review/zoom-edge` were inspected for retained ink,
clipping and opaque boundaries. Their isolated root coordinates cannot establish
composed editor placement or screen-space vertical centering.

Three matched release submission runs per lane show no clear CPU improvement
(about 10–11 µs/input), and do not measure presented frames, GPU cost, RSS savings
or dense-document final-render latency. Raw samples, build/source hashes, commands
and unavailable checks are in the
[edge/resource receipt](performance-results/tinymist-zoom-edges-2026-10-10.json).
The full editor `partial_zoom_stress` journey, composed-window bounds observation
and physical trackpad pinch remain unavailable because the user declined an idle
foreground interval. The implementation changes image geometry/cropping, not the
outer native preview rectangle.

After all checks, both the main Cargo target and the separate xtask target were
cleaned at the user's request (32.8 GiB and 78.9 MiB reported by Cargo). The final
optimized app/probes and raw evidence remain under
`.tiptoptyp/profiles/zoom-edge-final`; screenshots remain under `.tiptoptyp/screenshots`.


## Native zoom cache continuity and atomic refresh — 11 October 2026

A failing deterministic reproduction showed that sharp handoff discarded valid
pixels during idle recapture. The native layer now hides and retains those pixels
and their transform. A detached replacement cannot overwrite the admitted image's
geometry. Native acceptance is acknowledged before JavaScript promotes metadata;
source changes and input races reject late results without retiring usable pixels.
There is at most one capture and one prepared replacement. Retained pixels are
used only while they cover the visible paper; an uncovered zoom-out immediately
uses the live renderer instead of covering missing ink with an opaque proxy.

Formatting, strict all-target Clippy, opt-in native Clippy, Rust (1,806 passed,
44 ignored), xtask (17), JavaScript (82), and desktop harness contracts (56) passed.
Optimized own-window tests passed the delayed-admission race on the mock and actual
full/partial Tinymist frontends, verifying the effective backend. They also verify
updates during an 800 ms WebContent stall, one final vector request, refresh
recovery, immediate uncovered-paper fallback, covered zoom-out opacity and four
child-width changes. Sibling keyboard ownership and hidden/disabled cache release
passed. The first mock opacity run exposed a stale test assumption that a proxy
should remain even outside its captured paper; fallback and covered opacity now
have separate assertions.

Fresh isolated clip PNGs under `.tiptoptyp/screenshots/agent-review/zoom-reuse`
and `zoom-edge/native-{full,partial}-zoom-reuse-native-final.png` were inspected
for retained ink, centered mock paper and opaque gutters. They do not prove
composed-editor placement or screen-space vertical centering. No outer preview
bounds or maintained gallery contract changed.

Three matched interleaved release runs retained native feedback for the next
gesture (42–64 µs submission) where the baseline had no image and waited about
830 ms for sharp output during a forced stall. Final render latency was unchanged.
This is a component timing, not a GPU/frame-rate or whole-app smoothness guarantee.
Transient raster retention can reach eight million pixels during refresh, plus
capture/GPU overhead; no RSS savings are claimed. The full editor
`partial_zoom_stress` journey and physical pinch/composed-window checks remain
unavailable because the user declined an idle foreground interval. Own-window
probes prohibit activation and never post global input. Raw evidence and exclusions
are in the [cache-reuse receipt](performance-results/tinymist-zoom-reuse-2026-10-11.json).

The previous task's cache clean required an optimized rebuild. A stalled local
compiler-cache build was bypassed only for these checks; the final compiler and
documentation compiler match. No additional target clean or installed-app change
was made during this task.


## Installed editor hidden by native blur (2026-10-11)

Reproduced the grey editor in the installed release and established a failing
native regression before fixing it. An `NSVisualEffectView` added as the lowest
child still covered the parent CGL output. The higher Tinymist child remained
visible, which made the window look like a preview-only app. Previous framebuffer
captures did not include this native overlay and therefore missed the bug.

Removed the blur overlay while preserving ordinary translucency. Native tests
enforce no decoration children over CGL output, unchanged content/window identity,
resize behavior and sibling-window isolation. The renderer was initially treated
as incompatible with soft blur. The later sibling-placement investigation disproved
that broad claim; the old explanatory tooltip was removed. The fix adds no worker,
per-frame native mutation or idle repaint.

Formatting, strict all-target Clippy, all Rust suites (including 1,667 application
tests) and 17 xtask tests passed. The opt-in native property check passed. The
native lifecycle fixture initially timed out without application activation; its
bundled rerun completed with one canceled close, a committed close and no failure.
The real-app layout journey passed with Tinymist partial actually active and
ready, with Accessibility/input permissions available. Its build receipt and
`ui.preview.bounds` traces are retained in
`.tiptoptyp/desktop-ui-tests/run-rx_w0a52`.

A fresh release framebuffer was inspected at
`.tiptoptyp/screenshots/agent-review/blur-recovery/1791709418350-0001-main.png`.
A separate CUA whole-window observation verified that the corrected release
interface was visible in native composition; that isolated main scene does not
establish Tinymist-specific pixel composition. The bundle passed package and
signature verification. Logs, receipt and limitations are retained beside the
framebuffer in `review.json`.

### Preview buffer preparation — 11 October 2026

The live WebContent renderer no longer deep-copies and traverses dense SVG trees
to prepare its idle zoom buffer. The native matched probe measures 289→50 ms for
three dense partial pages (79,176 buffered nodes), and 31→7 ms in standard full
mode. Canonical output equivalence preserves paper geometry, ink and dependencies.
See `docs/performance-results/tinymist-buffer-preparation-2026-10-11.json` for
source/binary hashes, raw samples, environment and limitations. These are
preparation spans, not final vector-render or GPU-frame measurements.

The real `partial_zoom_stress` journey passed in
`.tiptoptyp/desktop-ui-tests/run-zj2nocly`, with the effective backends verified as
Tinymist partial and Tinymist full, real input permissions enabled and the desktop
unlocked. Both native zoom bursts sent one final vector request; cached divider
drags and far-page coverage passed. The initial run
`run-fb766wuw` exposed a harness input-owner error: it sent End after dragging a
root divider while preview keyboard focus was false. The journey now explicitly
clicks and checks preview focus before its viewer-only key. No production focus
behavior or renderer timing assertion was weakened. Native bounds traces are
retained in `app.log`; the inspected fresh browser buffer raster proves isolated
serialization/raster appearance only, not composed-editor pixels.

All required checks passed with four Rust test workers, plus the profiling-feature
checks, 88 deterministic JavaScript tests and 56 desktop harness tests. The first
default-concurrency Rust run hit seven existing fake-tool startup deadlines; the
Tinymist fixtures passed serially and both entire suites passed with bounded
concurrency. Fixture timeouts and test selection were unchanged.

### Automatic TeX package imports — 11 October 2026

A shared import planner now handles TeX table Apply (both grid and Source), drawn
symbols, math-environment conversion, completions/snippets and new templates.
It adds the required packages and tabularray libraries within the same document
transaction as the generated code. Existing imports/options/comments remain
intact; Cancel and failed/stale validation leave the document untouched. Undo
restores both the preamble and body, with Unicode cursors and linked snippet
fields remapped together. Existing packages are not removed automatically.

Sixteen focused regressions cover all supported table environments, column
providers, rules/colours, library deduplication, literal/comment exclusion,
package loading order, preamble configuration, built-in versus packaged symbols,
line endings, cursor/tabstop mapping and atomic Undo. The new native table case
starts with a missing siunitx import, checks Cancel/Apply/Undo/Redo and reopens
and reapplies without duplicating it. The final optimized `tables` journey
passed on macOS 14.6.1/Apple Silicon with Accessibility and input permission:
`.tiptoptyp/desktop-ui-tests/run-en41ar8p/result.json`, build
`1de1cf7896a4-dirty.1791721435`, executable SHA-256
`bc55a7e8c758c13b27d2e153e18e42aef5813cbd124bc7420036b8f5789e9976`.
This is native interaction evidence, not a pixel/composition claim. No maintained
visual contract changed, so no framebuffer or gallery recapture was required.

The audit caught and fixed imports preceding external preambles, array dependencies
following arydshln, and template imports following configuration commands that
needed them. The symbol transaction regression also caught a context-lock
reentry in focus restoration; editor identity is now read before acquiring the
focus write lock. Formatting, strict all-target clippy, all 1,875 Rust tests
(with four workers, 50 existing opt-in tests ignored), 17 xtask tests and 56
Python harness contract tests passed. Final logs are retained in
`/private/tmp/ttt-imports-{full-tests,xtask-tests,native}-final.log`.

Import work runs only on an explicit generation/apply action: there is no idle
polling, per-frame parsing, filesystem access or background package installation.
The manager is lexical, not a TeX evaluator. A fragment with no preamble reports
required imports in its main document instead of inserting invalid body-level
imports. Imported preamble/class internals and dynamic package names are not
expanded; user options remain authoritative.


## Source snippet editing and completion providers — 2026-10-11

Custom snippets now use a dedicated, searchable source form with per-language examples, language/prefix/description controls, cached Typst/TeX highlighting, line numbers, rainbow brackets and explicit Save/Revert. Invalid drafts never replace live snippets. Raw math dollars, backslashes, quotes and line breaks are preserved; only explicit numbered `${1:name}`, `${1}` and `${0}` fields are interpreted. Escaped TeX dollars before braces remain literal. A failing boundary test first demonstrated that `\${1}` was incorrectly consumed; the adapter now respects escaped dollars. Literal opening braces in placeholder defaults also exposed and fixed a shared LSP delimiter-parser bug.

Completion candidates carry provider metadata through filtering/rebasing independently of their insertion format. Rows show a language-server link, custom-snippet document, built-in code brackets or a font A, with concise provider tooltips and accessible labels. A mixed-provider semantic test covers hover stability, acceptance and dismissal.

The native `snippets` journey uses the source form for both languages, verifies scoping, literal `$x$`, `\${2}` and doubled backslashes, then checks linked editing and undo/redo. The `completion` journey checks both embedded built-in TeX and real Tinymist responses. Its initial server probe failed because the standalone fixture was plain text and had no server session; the fixture is now Typst and the journey waits for actual editor LSP readiness.

Final optimized build: `1de1cf7896a4-dirty.1791724404`, SHA-256 `6f73d4d679868a52d56d9636e297d1a943e8fe557e987005e8e63216c6c715fc`. Native input checks passed on macOS 14.6.1 / Apple Silicon, with Accessibility and input permissions available: `.tiptoptyp/desktop-ui-tests/run-islwuixo` (snippets, including the escaped-dollar boundary) and `run-vcio6m20` (completion). Fresh affected viewport captures were inspected and retained under `.tiptoptyp/screenshots/agent-review/snippets`. These are viewport framebuffers, not proof of composed native preview geometry; this change does not modify native preview bounds or the stable theme gallery.

All required checks passed: formatting, clippy with warnings denied (normal and desktop-ui-tests builds), 1,879 enabled Rust tests (50 existing opt-in ignores), 17 xtask tests and 56 runner contract checks. No performance benchmark was needed: provider metadata adds no background work; only the selected snippet is rendered, highlighting is cached, and validation runs on edits without idle polling or repaint loops. Unrelated working-tree changes, including todo source/output, were preserved.


### 2026-10-11: Font names in completion

Completion rows no longer create explanatory hover tooltips. Installed-font rows show only the family name, rendered using that family's primary face; the font badge, detail label and separate sample footer are removed. Provider symbols remain on other completion rows. The Settings help reflects this behavior.

Font names reuse the existing worker-owned rasterization and private atlases. The list paints visible rows plus overscan and reuses sixteen completion cache slots; font browsing does not replace the application's font definitions. Tests verify cached texture reuse, bounded slots, keyboard scrolling to either end of a long list, and stable bounds on hover. A fresh native image exposed a small overflow with two results: the actual themed button was 21 points tall against a 20-point virtual row. The regression fails before reducing vertical button padding and passes afterward; provider glyph reservation also uses the supporting text size.

Verification: fmt, default and desktop-feature strict Clippy, 1,881 Rust tests (50 existing opt-in ignored), 17 xtask tests, and 56 Python harness contract tests passed. The final real macOS completion journey passed in `.tiptoptyp/desktop-ui-tests/run-j7vqnpy6`, with Accessibility/input permission and an unlocked desktop. Its final build was debug `tiptoptyp Dev 0.1.0 (1de1cf7896a4-dirty.1791739511)`, SHA-256 `4d16c11c3f7c69bfa78ddf4378d88d4708db814e57668d88ef53b6cc283dd012`. This is interaction evidence, not a performance benchmark.

Inspected the fresh viewport framebuffer `.tiptoptyp/screenshots/agent-review/font-completion/run-j7vqnpy6/1791739549815-0004-main.png`: Times and Times New Roman show their serif faces, the list has no footer or redundant scrollbar, and row text is aligned and unclipped. This verifies the egui popup, not native-child preview composition. The maintained gallery was regenerated in one production app session and all 25 PNGs decoded and validated.


## Native blur restored in normal builds (2026-10-11)

Soft blur is enabled by default and has an Appearance checkbox. The material is a
sibling below the existing renderer, with behind-window blending; neither the
renderer nor its input owner is replaced. Each viewport owns one cached material.
AppKit handles resizing; a bounded parent check repairs fullscreen reparenting.
Theme changes reuse the view, and disabling blur removes it on the main thread.
No CPU image blur, additional worker or idle repaint loop is introduced. Native
compositor cost was not benchmarked.

The real-editor `background` journey passed with both Tinymist and PDFium actually
ready: three off/on cycles per backend, Light/Dark, separate Settings ownership,
Find typing/close, native resize and unchanged source. Native lifecycle checks
passed fullscreen/return (including shadow restoration), minimize/restore,
Settings hide/reopen, focus handoff and canceled/committed document close. The
property check passed stable content/responder identity and repeated attach/remove
cleanup. Required formatting, strict default/native-feature Clippy, 1,882 Rust
tests (50 opt-in tests ignored), 17 xtask tests and 56 Python contracts passed.

The 25-image release gallery was freshly regenerated and validated. Main light,
main dark and Settings framebuffers were inspected. A fresh CUA composed Tinymist
window remained visible with the new underlay; framebuffer images alone exclude
that material and native child. The strength of blur for desktop content behind
the window and Intel hardware remain unmeasured. Optional-review timeouts and a
retained-query harness mistake are recorded as failed attempts, not passes.

The production bundle passed package/signature checks and replaced the closed
installed app. Its previous bundle is preserved for rollback. Build hashes,
geometry traces, successful native evidence and limitations are retained in
`.tiptoptyp/profiles/native-blur-restored/review.json`; the passing journey is
`.tiptoptyp/desktop-ui-tests/run-iykerwyp`, and the native lifecycle report is
`.tiptoptyp/native-window-tests/run-qfpuiu6s/result.txt`.


### Partial-render exception recovery follow-up — 12 October 2026

A read-only audit found an existing failure-path gap outside the current successful
zoom/divider fixes. `flushPartial` clears coverage after a renderer exception
(`src/preview_navigation.js:348`), while the snapshot acknowledgement requires a
ready frame (`src/preview_zoom_snapshot.js:79`). The native `Pipeline::dispatch`
then refuses replacement work while the previous flight remains occupied
(`src/preview_zoom_snapshot.rs:185`). Further zoom input alone may therefore wait
for scroll/reload or another renderer recovery. This was identified from source;
no native exception-injection reproduction or recovery verification was run.

Follow-up: report a failed render with its token and revision, release only the
matching native flight, hide/retire its feedback, and allow subsequent input to
recover. Reject obsolete failure callbacks and keep recovery bounded; do not add
an automatic retry or idle repaint loop. The present anchor and retained-image
changes cover successful rendering and do not introduce this exception path.


### Delayed preview resize acknowledgement — 12 October 2026

The unlocked final-release journey in `run-x9cq1ixd` failed after widening the
partial preview: native request 54 remained in flight with the cached image
visible. The desktop was unlocked and the tested app remained foreground.
A deterministic contract reproduced the missing ready acknowledgement when a
second DOM resize arrived between sharp DOM mutation and its paint callback.
The snapshot helper now preserves an active flight's acknowledgement and treats
an already acknowledged viewport size as a no-op. Two focused contracts cover
notifications both before and after the handoff; newer native requests retain
their existing stale-callback cancellation.

The corrected release journey `run-ggn_p9d3` passed three narrow/widen cycles in
each actual Tinymist mode, native keyboard bursts and source-preserving cleanup.
All 12 divider anchors were within 0.43 CSS pixels. Matched native before/after
probes used the same optimized flags, viewport, mode-specific fixture and warmup;
the corrected probes passed with forced duplicate resize notifications and
captured/sharp ink within 0.85 CSS pixels. The original baseline still failed
full-mode alignment and cache retention. The repository transition receipt
retains hashes, native reports, 3,175 final `ui.preview.bounds` lines and the
failed attempts. Fresh isolated native-layer PNGs were inspected; composed
desktop pixels and physical trackpad presentation remain unverified.

A separate attempt, `run-e8z9eyub`, timed out reopening the preview controls after
scrolling despite a fresh stable hit target, foreground app and granted input.
The identical build passed that path on the next complete journey. This remains
an unexplained intermittent click failure, not an excluded permission failure
or an automatic retry inside the test. Investigate native event/focus ownership
if it recurs; no speculative popup fix was included in the resize change.

### Preview regression audit — 19 October 2026

Reviewed the recent preview changes, particularly `54a3c8c` (off-thread raster
preparation and capture deferral) and `2582d0b` (serial resize acknowledgements).
The bounded worker is retained. Capture deferral also blocked the initial ready
notification; three failing contracts established this for editor feedback,
outstanding pixels and reveal beacons. Readiness now belongs to the live renderer
and remains independent of warming. Four bounded-work contracts also require no
repeated DOM reads after readiness has been reported while warming is blocked.

The older "Preview render exceptions" follow-up is resolved: queued layout and
partial-patch failures previously retained the native serial slot. Three failing
contracts now require the original error to remain observable and the next zoom
to recover. Failure callbacks carry a WebView owner and revision, so an obsolete
callback cannot cancel a replacement preview with the same revision. There is no
automatic retry loop. Native transition coverage injects the same queued failure
into the pinned frontend and checks subsequent zoom input. Native pinch phase
coverage includes ending outside the preview; physical trackpad input remains a
separate manual check.

Evidence for this audit is retained under
`.tiptoptyp/preview-zoom-regression-2026-10-19/`. A comparison run using the
preserved pre-worker build failed the divider handoff; the first current-build
stress attempt instead failed its Home/top-edge step after passing cached zoom
and a 20-input burst. Neither attempt is counted as a passing journey. Small
owned-WebView full/partial probes and an isolated session with the 19-page user
document passed repeated divider drags before the new changes. These observations
separate cold-session success from failure recovery rather than asserting that a
fresh preview proves an earlier stuck session was healthy.

The open installed process also had a different loaded Mach-O UUID from the
executable currently on disk. Reading `--version` from the file incorrectly
identified that live process as current. A normal quit/relaunch restored a normal
preview. For an existing non-inspection build, verify the loaded image identity
(e.g. the app entry in a native CPU sample against `dwarfdump --uuid`) or restart
it; do not substitute an on-disk version check for the running build. Feature
journeys already report their compiled build identity from the live process.

The first new owned-WebView run recovered from the injected exception but failed
its overall gate: WebKit concealed the deliberate exception's Error object, and
a page-18-specific observer extrapolated a negative page fraction when extra
recovery transitions moved the viewport into page 17. The observer now follows
the actual page or fixed gap under the viewport. Recomputing the retained failed
receipt independently gave partial-mode source errors of 0.25 and 0.29 CSS pixels;
its alleged 1.72-pixel error was not a renderer defect. The unnecessary production
rounding adjustment was removed. Strict geometry tolerances remain unchanged.
The test scopes its expected exception to the synchronously armed injection,
requires exactly one observable intended error, and still rejects unexpected
errors. Original failed receipts remain under `verified-serial/` and
`handoff-serial/` rather than being overwritten.

Final checks passed: 244 JavaScript contracts, 151 Python contracts, 2,336 default
Rust tests, 2,344 profiling Rust tests and 32 packaging-task tests, plus formatting
and strict default/profiling/desktop-UI lint checks. Both pinned frontend probes
passed all 13 transitions, including exactly one deliberately injected renderer
error per mode and recovery afterward. Fresh isolated native-layer PNGs were
inspected; they are not composed desktop evidence.

The real-app `partial_dividers` journey (`run-2edrh6mp`) passed twelve native
divider drags across both modes, preserving source position and releasing the
cached overlay. Its 1,046 preview-bounds trace lines are retained. The earlier
`partial_cache_edges` attempt (`run-vp_y855n`) passed its divider steps but failed
full-mode Control-wheel input; it is not counted as a passing journey. Read-only
preflight subsequently found macOS Accessibility scroll-to-zoom enabled. The
harness now marks global Control-wheel unavailable before posting input under
that setting, and divider coverage has its own independent journey. No OS
preferences were changed. Physical trackpad input remains a manual check.

The checked-in performance receipt records the optimized native probe metadata
and scope. These are functional recovery checks and four callback-latency
samples, not a dense-document presentation benchmark. The installed app has not
been replaced by this patch.

### Retained screenshot cursor follow-up — 19 October 2026

The next report described positional mismatch, followed by intermittent loss of
both pinch input and cached divider feedback. The installed process's loaded
Mach-O UUID matched the installed `94124c8` binary; the observed editor used
Tinymist. This was not dismissed as an old executable.

The stronger native regression defers recapture through 24 alternating cursor
zooms. It projects the original live source point independently of the proxy's
reported anchor. Before the fix, full-mode drift exceeded the existing 1.1-pixel
tolerance, and both modes lost ownership of an outside pinch release once the
viewport left the retained raster. Reconciliation now uses the completed SVG's
actual position; losing buffer coverage only removes the render deadline, not
the gesture's ownership.

The final `confirmed/` probes passed 35 transitions per mode, including all
24 cursor zooms each. Maximum original vertical source error was 0.940 CSS pixels
in full mode and 0.950 in partial mode. An intermediate `reconciled/` run fixed
the drift but still failed the outside release. A subsequent `final/` attempt
passed all cursor zooms but failed downstream coverage assertions because that
phase reused the deliberately exhausted buffer before idle warming caught up.
The divider phase now waits for a fresh, matching idle raster after recapture is
resumed. Earlier failed receipts remain preserved.

The real-app divider journey `run-m2dbyp2k` passed twelve native drags in both
modes after the owned native probes had exited. Its 823 `ui.preview.bounds` lines
are retained. An earlier passing journey overlapped an owned probe; that run is
not used as the final interaction evidence. Fresh full/partial isolated native
PNGs were inspected; they are not proof of composed desktop geometry. Required
formatting, strict default/desktop-UI lint, 2,337 default Rust tests, 32 xtask
tests and 245 JavaScript contracts passed.

Evidence is under `.tiptoptyp/preview-cursor-anchor-2026-10-19/` with a checked-in
cursor-anchor receipt. Reconciliation is constant work at handoff and adds five
numbers to the existing acknowledgement. It adds no input-time layout queries
or screenshot work. Callback samples are not presentation timing measurements.

The complete intermittent loss of input has not been reproduced in the disposable
diagnostic app, which has been updated with the fixes and left open for a
physical pinch check. That report remains unresolved until an actual failing
state can be inspected. The installed app was not overwritten.

Manual follow-up: the user tried the updated `preview-zoom-stress.typ` diagnostic
window and confirmed that both pinch and divider work. Live inspection verified
Tinymist partial, 143 native zoom inputs, 219 cached-image updates, five completed
handoffs and zero render failures. The controller was initialized and cached,
with no active overlay, flight, pending request or owned pinch afterward. This
adds physical interaction evidence for that partial-mode session; full-mode
physical pinch and universal absence of the intermittent failure are not claimed.


### Partial preview liveness follow-up (20 October 2026)

The installed app was verified as `ce3787ad9656.1792446246` with a matching live
arm64 Mach-O UUID. Its embedded preview scripts and those in the previous
instrumented diagnostic match the checked-in baseline. The toolbar label
`Tinymist` intentionally covers both renderers; the earlier inference that this
label proved full mode was incorrect. The user clarified that the reported
failure used partial mode, with only pinch and divider interaction. Separate
manual tests in the instrumented app did not reproduce the failure in either
full or partial mode; actual partial mode was verified through the inspector.

Two related failures were reproduced before fixing them. The owned real-frontend
probe timed out rewarming after a no-op page action in both modes: native pixels
were gone while browser cache metadata remained valid. A deterministic partial
viewport test also stranded a zoom handoff when a brief buffered-edge excursion
returned before its scheduled prefetch. The temporary rendering flag blocked a
paint observation, then cleared without a completion notification. Both paint
phases are covered. Cache retirement now exchanges only matching image/admission
tokens; a no-work partial pass publishes completion once. Neither fix adds an
idle loop, a new worker, input-time screenshot work, or a parallel vector render.

The final optimized native rerun passed 36 full and 37 partial transition steps,
including the no-op recovery and an injected redundant partial pass over actual
Tinymist. Each mode retained one raster through 24 alternating native pinch-phase
zooms, stayed responsive during the existing 800 ms WebContent stall, and
submitted one final vector request. A prior passing run overlapped compilation;
only the subsequent `verified/` run is used for the paired callback samples.
Those four samples per mode are not GPU frame-rate or perceived-smoothness proof.

The composed-editor `partial_dividers` journey `run-colkfrsx` passed twelve native
drags across partial/full modes and retained 719 bounds traces. Fresh before/after
isolated native PNGs were inspected; no composed desktop pixel verification is
claimed. Required formatting, strict default/desktop-UI lint, 2,337 default Rust
tests (70 ignored), 32 xtask tests (one ignored), 243 JavaScript tests and 151
native-journey contract tests passed. The debug test link emitted the existing
large unwind-table warning; no Rust test failed.

Raw evidence and setup failures are retained under
`.tiptoptyp/preview-liveness-2026-10-20/`, with a checked-in liveness receipt.
The installed app was not overwritten. The original intermittent installed-app
failure remains unobserved directly; these are fixes for reproduced failure
paths, rather than a claim that every physical-input failure has been eliminated.


The fixed `tiptoptyp Partial Preview Probe` was then configured with copied user
preferences in a fresh isolated profile, with autosave disabled. It passed six
more native divider drags and twelve alternating native preview zoom-button
actions on the original document. Source fingerprint/dirty checks remained
unchanged. The final actual renderer was partial, with warm native pixels and
no active overlay, owned pinch, flight or pending request. This app is left open;
these button checks are not a physical trackpad pinch test. Its first launch
refused reuse of the old settings store, and an early activation ran before the
new window registered; those setup failures are preserved separately from the
successful interaction evidence.
