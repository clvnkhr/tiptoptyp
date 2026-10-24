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
python3 scripts/test-desktop-ui.py --journey settings_links
python3 scripts/test-desktop-ui.py --journey tool_defaults --capture-review
python3 scripts/test-desktop-ui.py --journey indentation
python3 scripts/test-desktop-ui.py --journey problems_focus
python3 scripts/test-desktop-ui.py --journey activity --capture-review
python3 scripts/test-desktop-ui.py --journey preview_find
python3 scripts/test-desktop-ui.py --journey tables
python3 scripts/test-desktop-ui.py --journey arxiv_tables
python3 scripts/test-desktop-ui.py --journey math_grids --capture-review
python3 scripts/test-desktop-ui.py --journey terminal_find
python3 scripts/test-desktop-ui.py --journey command_palette --capture-review
python3 scripts/test-desktop-ui.py --journey overleaf_connection
python3 scripts/test-desktop-ui.py --journey file_versions --release --capture-review --trace-preview
python3 scripts/test-desktop-ui.py --journey preview_jump --capture-review
python3 scripts/test-desktop-ui.py --journey typing_particles --capture-review
python3 scripts/test-desktop-ui.py --journey effects_pacing --release
python3 scripts/test-desktop-ui.py --journey terminal_load
python3 scripts/test-desktop-ui.py --journey terminal_theme
python3 scripts/test-desktop-ui.py --journey explorer_files --capture-review
python3 scripts/test-desktop-ui.py --journey explorer_toggle --release --capture-review
python3 scripts/test-desktop-ui.py --journey pdfium_resize --release --capture-review
python3 scripts/test-desktop-ui.py --journey background_fades --capture-review
python3 scripts/test-desktop-ui.py --journey images
python3 scripts/test-desktop-ui.py --journey find
python3 scripts/test-desktop-ui.py --journey empty
python3 scripts/test-desktop-ui.py --journey focus
python3 scripts/test-desktop-ui.py --journey preview
python3 scripts/test-desktop-ui.py --journey pdf_output
python3 scripts/test-desktop-ui.py --journey tex
python3 scripts/test-desktop-ui.py --journey tex_documentation
python3 scripts/test-desktop-ui.py --journey citation_import
python3 scripts/test-desktop-ui.py --journey tex_editing
python3 scripts/test-desktop-ui.py --journey tex_intelligence
python3 scripts/test-desktop-ui.py --journey tex_save
python3 scripts/test-desktop-ui.py --journey tex_preview_scroll --capture-review
python3 scripts/test-desktop-ui.py --journey harper_math_prose --release
```

The runner compiles a separate `desktop-ui-tests` feature build and copies it
into a uniquely identified temporary app bundle. It launches one editor window
with a disposable document, fresh settings and session storage. Use `--release`
for the optimized production profile; the profile is recorded in `result.json`.
The focused `explorer_toggle` fixture adds 1,800 long comment rows to exercise
source reflow while preserving the three-page preview. It also closes, reopens,
and drags Explorer while Code/Preview-only views hide the split editor, then
checks that returning to Split preserves the independent divider. Its focus
checks cover explicit Code focus and Cmd+F after closing a focused Explorer.
The `images` journey also starts from a typesetting document to check that an
active image remains visible beside a popped-out designated source preview, with
no duplicate main-window preview divider, and restores correctly on closing it.
Preview/controls/tabs/layout/background/all runs
use a three-page Typst document and require working Tinymist, Typst and PDFium tools. The `panels`, `terminal_find`, `terminal_load` and `terminal_theme`
journeys currently require the system `/bin/zsh`; it isolates zsh startup files
and shell history with `ZDOTDIR`. No user document is opened. The fixture app
and its process group are terminated on success or failure.

Multiline fixture input uses an acknowledged native paste. The adapter holds the
previous clipboard representations in memory, waits for the runner to observe
the expected source fingerprint, then restores them unless another process has
changed the clipboard meanwhile. Native Copy checks likewise hold the prior
representations, acknowledge only whether the selected fixture text was copied,
and restore only that observed copy if no newer clipboard write occurred.
An unmatched write is left alone because it could belong to another process.
Copy/paste adapters are stopped and reaped on restoration timeouts; such a run
fails and does not claim that restoration succeeded. Clipboard contents are
never written to logs.

Normal builds contain no inspection endpoint. A feature build only starts one
when `TIPTOPTYP_DESKTOP_TEST_DIR` names a fresh private directory (mode 0700).
That directory owns both the settings file and a mode-0600 Unix socket.
The socket accepts `snapshot\n` or `snapshot <viewport>\n`; there is no command or state-mutation API.
The targeted form uses the exact observed document viewport identifier. It accepts
only that document frame and labels DOM observations with their renderer viewport;
callbacks from a previous owner are discarded. Multiwindow journeys must check
both owners, even when the pages and selected backend look identical.
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
The preview drag regression separates mouse-down, held moves and release, then
checks the native button state to prove its stationary frames precede release.
Its cleanup releases the button even if an input receipt or assertion fails.
Query-focus checks wait for the focused widget's next fresh frame before typing;
they do not click the field to compensate for an incorrect Cmd+F handoff.

## Current coverage

See [the promotion audit](desktop-ui-audit.md) for the original tests, the portions
now exercised through the real app, and the lower-level coverage retained.

| Journey | Input and assertions |
| --- | --- |
| explorer_files | Selects an actual folder row, captures its idle selected disclosure when requested, and collapses/expands it through the existing arrow hit rectangle; fresh child targets must disappear/reappear without changing the source owner. Folder and file context menus create empty named documents in the correct directory and open their tabs. Typing filenames stays in the native prompt; Cancel creates no artifact and colliding names report an error without overwriting bytes. Duplicates a file and a folder, verifies their initial copy names and exact recursive bytes (including a binary payload), renames each through the native prompt, and proves the original document, originals and preview owner remain intact. All fixtures live in the runner’s disposable workspace; no user files are touched. |
| typing_particles | Requires factory defaults (effects off, Medium density, Explosion per action), enables Editor effects through native Settings, then selects all five particle styles for Typing. Gives the other keyboard actions independent styles and verifies changes preserve every unrelated action preference and emission history. Each enabled action must produce exactly one cause-tagged burst with bounded live counts and actual GPU draw/upload evidence. Covers keyboard comment/uncomment, Copy, Paste, Cut, Undo/Redo, newline/delete/indent, and popup Copy/Toggle Comment; clipboard writes are verified and restored. Selects particle-Off Shake for Typing, Flash for Indent and Glow for Newline and observes their real finite amplitudes with no particle uploads/draws, then verifies every individual effect Off. Checks light/dark appearance, expiry/no idle draws, quiet navigation/menu dismissal, Settings input, empty-history commands and master-disabled typing with all feedback kinds still configured. Restores all per-action preferences and appearance, including on failure. Also runs inside editor_preferences; optional fresh transient framebuffers for each style, both appearances and command effects |
| effects_pacing | Optimized, observer-free particle callback measurements in saved large plain-text and Typst files with a confirmed live Tinymist preview. Uses native Settings JSON to select Low Explosion effects and snapshots on/off, then measures one first and six warmed Typing/Copy bursts per document. Verifies exact action/source/focus, bounded ledger continuity, finite expiry and shader/callback clock consistency; reports early/late intervals without an FPS threshold |
| panels | Cmd+5, repeated Terminal/Problems selection and hide/reopen; stable panel height across 30 fresh frames per sample; native terminal typing runs an isolated shell, Off reaps it and remains off across panel transitions, Restart starts a different shell; the header button opens a second shell with its own PID, the numbered switcher moves between shells, a real vim click moves its cursor, exiting one shell leaves the other parked; document stays unchanged |
| problems_focus | Both effective preview backends hand keyboard ownership to Problems; blank-area/header clicks retain it, typing/paste/undo/comment shortcuts cannot change source or invert preview pages; Cmd+F focuses Problems search, native typing and Select All/Backspace edit only its query, and Find/Replace cannot open source Find. Closing/switching panels releases focus so source typing resumes; preview → Terminal handoff delivers real shell input. A private 48-help Typst file produces real Harper help and compiler errors: initially compact flat rows keep the filter strip hidden; native wheel-up reveals it, wheel-down hides the inactive strip without resetting result scrolling, and selected filters stay pinned through scrolling. Combine source/type/search filters, search case-insensitively, enable optional grouping and collapse/expand it, return to flat rows, show zero matches, reset every filter, and navigate an original diagnostic after filtering without changing source. |
| preview_find | Both effective backends: preview Cmd+F opens and focuses the query without another click, subsequent Cmd+F and typing stay in that field, the source and preview palette stay unchanged; header dragging follows the full pointer displacement and holds a stationary pointer for at least 0.4 seconds and eight fresh frames without drift; a popped-out viewer also opens its hidden controls with Cmd+F ; PDFium query typing/replacement/clear retains ready pixels and unchanged raster request/admission counters |
| tables | Real Typst and named TeX workbenches: cell Cmd+A/typing changes only the draft, Cancel preserves source, Apply updates once with original column widths/rules or TeX placement/row height, and Undo/Redo restores exact source; whole-generated Typst and dynamic-column TeX tables are absent from a fresh source context menu; invalid drafts disable Apply and are repaired in the same grid cell. Both languages also exercise a compact minimum-size 36-row grid with long keyboard-edited cell text, real wheel scrolling and fixed Apply/Cancel footer; Typst verifies Tab moves to the next cell. Optional fresh table framebuffer for separate visual inspection |
| arxiv_tables | Opens private copies of three attributed arXiv TeX excerpts through native Open and detects each table with the source context menu. Edits grid cells in both SciPy tables and the merged/format-rich Platonic metric table; the latter retains its complete cell wrappers and citation. Cancel preserves the complete original; Apply changes only the chosen literal, including all surrounding whitespace, rules, macros and preamble, and one Undo restores the exact file. Optional fresh table framebuffers for separate visual inspection |
| math_grids | Opens private copies of the checked-in TeX and Typst math fixtures. Edits array, pmatrix, cases, mat and vec grids through the native context menu. Formula fields remain editable, keep math syntax without extra wrappers, and do not start compiler sampling. Cancel preserves the file; Apply changes only the chosen formula and one Undo restores the exact source. A matrix row-count control adds a third zero-filled row and applies/undoes exact math syntax |
| preview_jump | Right-click source → Reveal in Preview and preview → Reveal in Source in TeX/PDFium and both Tinymist modes, including a detached Tinymist preview; original clicked positions survive moving to the menu, right-click alone does not navigate, source focus returns after inverse navigation, a visible destination pulse is centered on the mapped page/caret and retires without reappearing; native Settings edits apply a 1500ms/150% effect, source and preview Off controls independently suppress only their cue while navigation/focus still work, both settings restore afterwards, and navigation leaves source files unchanged |
| command_palette | Cmd+Shift+P focuses palette search; an unmatched query cannot execute, mixed/lowercase queries return identical actions, Up/Down select exactly one enabled row, Enter and a row click each toggle wrapping once, and Escape/outside clicks preserve source and pending commands. A private real terminal retains keyboard ownership through cancellation and command execution, Comfy Preview toggles exactly once from that terminal, and palette Find targets its query instead of source Find. Native Settings ignores the document palette shortcut. The standalone journey also closes the final clean tab, creates exactly one empty document through palette New, then reopens the unchanged launch fixture; aggregate runs preserve existing tabs. Optional fresh popup framebuffer for separate visual inspection |
| terminal_find | Focused terminal Cmd+F searches a real isolated shell, including three case-insensitive matches separated by hundreds of scrolled-off lines; Cmd+G/Shift+Cmd+G and Enter/Shift+Enter step exactly once and wrap, button navigation reveals the match; Cmd+A changes only the query; grid Escape leaves search open, grid Cmd+F refocuses it, focused Cmd+F/Escape and one close click hide it; queries and matches stay separate between two shell panes; source stays unchanged |
| terminal_load | A bounded, isolated Python process redraws the real PTY at requested 30/60 Hz; native per-character typing must change only the focused scratch editor, with exact source/caret and worker output advancement; completed producer receipts record actual frames and terminal Off reaps the pane. This is interaction coverage, not a key-to-pixel latency measurement |
| terminal_theme | First output from a private real PTY queries foreground/background, all 256 palette entries, and color scheme; Light→Dark and a different dark palette each report exactly once to the opted-in application; 30 fresh frames require no notification churn; Off reaps the child and source stays unchanged |
| explorer_toggle | Cmd+1 repeatedly hides/restores Explorer before and after divider drags with Tinymist/PDFium and both divider policies; concurrent native-key observations reject empty closed-panel frames and independent preview-edge movement; large-document typing/undo still work afterward; a nearly full-height bottom panel leaves a short vertical divider for optional framebuffer inspection |
| inactive_preview | Full and partial Tinymist create and finish rendering while Settings owns their inactive document window and Finder is foreground; actual DOM readiness and effective backend are checked for root and secondary document windows, with no activation stealing or source mutation |
| pdf_layout | A generated PDF opens beside a designated Tinymist partial preview; repeated Code/Preview/Split layout-picker changes and shortcuts hide the chosen pane, preserve both tab identities, render the PDF through PDFium, leave its bytes untouched, and restore the original source after closing |
| images | Image dark mode, an explicit original-color baseline, repeated toggles and Dark/Light/Follow page choices; initial image preference restored; exact bounded RGBA/alpha observations, unchanged original file, original source restored on close; optional fresh framebuffers for separate visual inspection |
| find | Cmd+F with Find focused, repeated Replace clicks, one close-button click; expected popup state and stable measured native height after every action |
| empty | Cmd+W on the only tab, then Cmd+N; original window remains, new tab has no path and zero source bytes |
| tex_editing | Named `.tex` file, Unicode caret and undo/redo, 320-line source, `%` comment round trips including active line endpoints, math/bracket selection wrapping, literal TeX command search/replace with undo/redo; disabled autosave leaves disk unchanged |
| tex_intelligence | Real TexLab/Badness readiness and Badness lint output; Harper prose versus math/text boundaries, old Harper/Badness diagnostics retained until refreshed, Problems navigation, accepting a document-defined TexLab command with undo/redo |
| tex_save | New untyped buffer saved as `.tex` through the native panel, exact disk contents, manual save, clean close and reopening the saved file |
| tex_preview_scroll | A separate tall three-page TeX fixture receives three real edits with default scroll preservation, then three with optional TeX edit-follow enabled through Settings JSON. Each automatic compile must admit a new exact PDF revision, retain the source caret and keyboard focus, and either keep the user's visible page/offset or place the mapped baseline comfortably inside freshly painted bounds. No edit triggers an explicit Compile or Reveal action; automatic navigation remains silent and the original preference is restored |
| harper_math_prose | Real Typst and TeX mathematical-prose fixtures; clean completed diagnostics across display punctuation and compounds, native typing of a typo and repeated words (including across Typst inline formatting) at exact source locations/original paths, stale results retained after correction until a fresh clean result arrives; Typst and TeX ttt: on/off regions retain six raw Harper findings while exact visible before/after rows change 6 → 4 → 6, restoring source and disk afterward |
| harper_settings | Real Harper checks after editing Harper dictionary in Grammar & spelling settings and removing its entry; capitalized-name filtering switches on/off without suppressing lowercase spelling errors or repeated-word grammar; an editable Harper style-rule override adds then removes only its own finding, and the source remains unchanged |
| liquid_selection | Opt-in real local Liquid model: wrapped Typst/TeX prose selected by keyboard and opened through right-click; disabled empty-selection action, exact outside-selection preservation, unchanged source before Apply and after Cancel, modal Undo/Cmd+W ownership, one-step Undo after Apply, complete before/after selection fingerprints including unchanged context and wrappers in both Unified and Side by Side settings, optional fresh review framebuffer for every language/style pair. Uses a prepared read-only model reference, verifies automatic first-use preparation into an empty temporary cache without an Install action or Python, and is excluded from `all`. |
| liquid_streaming | Focused opt-in long-selection subset: wrapped Typst and TeX paragraphs selected across visual word-wrap rows through native document navigation, then streamed partial suggestions with disabled Apply, complete model-token-bounded chunks, source fences, and one-step Undo. Uses its own initially absent cache and is excluded from `all`. |
| overleaf_connection | Opens Connect to Overleaf from the actual command palette; blank, insecure HTTP and token-bearing URLs fail before browser allocation. Toggles local-copy storage, verifies temporary mode hides folder controls, types an absolute custom folder and retains it after URL rejection. A relative folder with a syntactically valid project URL on reserved `overleaf.invalid` fails before any storage worker or connection is allocated. Native Select All, typing, Undo/Redo stay in their fields; Find and New cannot affect the source owner. Cmd+W and Hide close only the connection window, reopening retains its unsent URL and folder, and document Find shortcuts work normally afterward. Checks source/disk/tab/preview ownership with bounded metadata and fingerprints. Offline and included in `all`; does not authenticate, contact Overleaf, or prove remote synchronization/hydration or temporary-file deletion. |
| overleaf_storage | A localhost HTTP fixture serves the official frontend socket/store surfaces to the real WebView bridge; no account, native IPC shortcuts or direct app-state injection. Holds one binary asset while source is shown, proves native typing is blocked until every file reaches disk, then edits and undoes source. Explicit Disconnect, hiding the manager and closing the temporary project remove its owned directory; a second connection retains an exact custom namespaced local copy after the same actions. This checks integration lifecycle and transport, not compatibility with Overleaf Cloud authentication or its production frontend. |
| latexdiff_wizard | Opt-in real LaTeXdiff through command palette and three wizard steps; native path/argument typing, Back draft retention, output/input collision rejection, original file and owner preservation, actual generated body additions/deletions, Open selects the complete `.tex` result in a separate tab, Cmd+W closes only the wizard/result, and a private sleeping tool configured through Settings JSON makes busy Cancel observable and proves its process is reaped without an output file. Requires installed LaTeXdiff; excluded from `all`. |
| editing | Typing and Unicode replacement, undo/redo caret and saved-state indicator, acknowledged multiline paste, upward mouse/keyboard newline selection, direction-preserving comment round trips including a zero-column or blank active endpoint |
| search | Query focus, next/previous, case/regex toggles, replace one/all, undo and Escape |
| tabs | Previous/next with separate unsaved buffers and carets; original preview retained |
| activity | Actual service inventory grouped into compact build/preview, code, checks, files, Git and tools rows; native wheel reveals/hides inactive filters without offset fighting, Cmd+F/Select All/typing stays in Activity, group/state/text filters combine with exact source identities and counts, collapse/expand and flat/grouped mode preserve filters, empty results remain honest, reset and Problems handoff preserve document contents/caret/tabs and the user’s panel height. Optional fresh main-viewport framebuffer for separate visual inspection. |
| layout | Code/preview/split shortcuts and every compact Layout picker row with a live Tinymist child behind the native popup; Explorer hide/maximize/restore, editor setting shortcuts, panel maximize/Activity/close and picker hide/reopen preserve manual height and selected tab; Appearance’s Compact layout controls toggle restores every separate toolbar action and retains panel state |
| explorer_scroll | Native wheel input over long Files and Contents lists, then Files with Cmd+F's sticky search open; measured offsets advance and remain advanced across fresh frames |
| git_hunks | Three changes across a 180-line Typst Git document in a split editor/preview window; a thin right-edge overview includes the offscreen last hunk, stays inside the code pane and retains its global positions during real wheel scrolling without changing pane width; double-clicking the Git file row opens the docked whole-document diff without changing source or pane width; marker hover preserves focus, clicks open all lines at the selected change, navigation moves the live cursor and an offscreen hunk receives its finite reveal pulse after scrolling into view, individual reversions and edits outside the original hunk refresh immediately and undo/redo, Find retains keyboard ownership, Escape preserves the pane width, independent/fixed-width divider settings are exercised by real drags, selected text survives right-click/Copy and direct math conversion in the docked editor, and a diff button tooltip opens in a native window with Tinymist active |
| file_versions | Creates a fresh private named Typst file, edits and explicitly saves two- and three-page revisions through the actual editor, then opens File → Revert To. History opened from Code-only must occupy the code pane in Split view with its full diff beside a visible preview and no separate native history window. Real version selectors must identify the expected older/current fingerprints; typing into the read-only comparison leaves the buffer and disk untouched. Each side’s Preview PDF button must display that side’s fingerprint and exact page count in PDFium, hide the native Tinymist view, and preserve the current source. Restore replaces the complete buffer and disk; the pre-restore newer revision remains available for a second restore. Escape from a rendered older PDF returns to the current live preview; Cmd+W and Close also leave the source tab/window intact. Only the disposable tab is closed afterward. Optional fresh main-viewport framebuffer for separate visual inspection |
| folding | Repeated shortcut and gutter collapse/expand without losing headers or source |
| closing | Dirty-close Cancel/Escape preserve text; explicit Discard closes only the requested tab |
| controls | Both engines: stable outline popup size, app-switch hide/restore, outline/history/pages/zoom/fit/search, minimize/reopen, detach/return |
| settings_links | Opens Settings, navigates to Grammar & spelling through native search, expands LTeX+ setup and clicks its Configuration reference. Requires both the exact background-launcher success notice and an observed foreground browser, then returns to the same Settings window and compares reloaded complete preferences and unchanged source/preview ownership. Runs separately from `all` because it deliberately opens the system browser; needs no model download. Unknown browser bundle identifiers are reported as unavailable evidence rather than accepted as a pass. |
| indentation | Opens real named `.typ` and `.tex` fixtures and posts Return: structural blocks/environments, paired brackets, closing-block level, root document environment, literal mixed tabs/spaces and CRLF. Exact source/caret and whole-action Undo/Redo are checked with automatic pairing off/on and spaces/tabs. Multiline paste remains verbatim. Restores complete preferences and the original source/preview owner; included in `all`, no model dependency. |
| tool_defaults | Opens the local LTeX+ reference in Settings without activating a browser or changing preferences; Cmd+F stays in the modal and Cmd+W closes only the reference. LTeX+ and Harper Normal → Verbose → Normal clicks preserve provider activation, language, dictionaries, source/preview ownership and every restored setting, including executable selections. Requires no model download; included in `all`. Optional fresh Settings framebuffers for separate inspection. |
| settings_search | Settings category switches retain the query and form edits; global search routes across Appearance, Editor, Writing, Grammar & spelling, Preview and Tools without resizing the native window; JSON drafts survive form navigation, invalid/reload/save and JSON Find stay local; Cmd+N/Cmd+5 cannot change the document, repeated Cmd+F owns search, and Cmd+W closes only Settings |
| partial_preview | A standalone fresh profile first proves buffered Typst is the actual default renderer; a real 48-page A4 document with fractional point sizes then exercises partial preview; native End, Outline and wheel navigation refill visible ink, repeated zoom buttons and keyboard events grow/shrink actual paper geometry, and fit preserves coverage, and full → partial switching preserves both choices and restores the original session |
| partial_dividers | Six actual narrow/widen drags in each Tinymist backend, cached feedback, source preservation, current sharp handoff and the original viewport-top source point; separate from OS-reserved Control-scroll input |
| partial_cache_edges | A fresh three-page document in each Tinymist backend; Home then a clamped upward wheel must retain the same warm image; three 220-point narrow/widen cycles must use cached feedback, preserve the original live-document point at the visible top, and hand off within 1.1 CSS pixels; positioned native Ctrl-wheel must preserve the original source point under an off-center cursor; after divider release, clicking the source editor must transfer keyboard ownership away from the preview |
| partial_zoom_stress | The higher-resolution viewport cache is admitted, its setting disables/releases and reenables it, and real keyboard zoom records fresh cache hits/handoffs in both Tinymist viewers and advances the production native compositor counters; Home/End work after zoom/fit; opt-in dense 48-page matrix/nested-math fixture with unequal fractional widths; real-app partial zoom/refills on the dense fixture, full-viewer keyboard zoom and last-page tracking on the standard 48-page fixture, then return to the dense partial entry; actual paper-scale/ink assertions, cached native feedback through three narrow/widen cycles in each viewer, no stranded render flight after delayed duplicate resize notifications, and native/sharp scroll-anchor comparisons within 1.1 CSS pixels, and identity-based session cleanup |
| background_fades | Edit independent blur/transparency directions in Appearance through their actual dropdowns, require the requested and applied native directions on document and Settings windows, and reject uniform fallback for a requested directional effect. Transparency changes and repeated focus/Find typing preserve each controller and its application count; fullscreen removes both effects and restores the chosen directions afterward; both directions return to Uniform. No preview backend switching |
| background | Hidden Tinymist frontend loads before reveal; WindowServer accepts separate document/Settings blur configurations in Tinymist and PDFium; Settings/Find focus, theme changes and native maximize/restore retain their accepted radius and controller identity without reapplying blur; repeated off/on, Find typing and source preservation. Accepted radius records a successful native call, not compositor readback or composed pixel verification |
| focus | Two document windows; Settings opened/closed from alternating owners; native focus returns to each owner; minimize/restore, Finder app switch, close second window once |
| pdfium_resize | Compile an isolated three-page tall TeX fixture through the normal pinned-preview controls into the effective PDFium viewer without changing the Typst backend preference, fit width, and repeatedly drag the actual editor/preview divider with native mouse down/move/up. Every held frame retains the resident batch, active scale and request/admission counters, including a stationary hold beyond the settle deadline; each release publishes exactly one sharp batch at the final physical scale. Positioned native Command-wheel then zooms in/out on page three and checks the PDF point under the cursor against fresh painted-page geometry. It does not change macOS Accessibility Zoom preferences. Source and original session ownership survive. Bounded snapshot latencies are observations, not a presentation-rate benchmark |
| preview | Pin a real Typst fixture; switch PDFium → Tinymist through Settings; require the selected renderer to become ready; light/dark × comfy transitions; applied palette and live Tinymist DOM checks |
| pdf_output | Tinymist live preview and engine refresh do not create an adjacent PDF; PDF button and Cmd+R commit actual PDF bytes, edits mark it stale without overwriting it, failed builds retain the old file and clear the queue, corrected builds recover, and PDFium cache refresh does not write the adjacent file |
| context_menu | Right-click on math opens the editor menu with a direct Convert row (Typst offers the other form; TeX keeps the side submenu on hover); choosing it rewrites the source and undo restores it; a plain-text line offers the standard rows with no Convert row; selected text survives right-click for Copy |

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

Still to add: restored Typst → pinned TeX builds and automated assertions against
composed on-screen pixels. Save journeys explicitly use the native Go to Folder
action before confirming, so new documents stay inside the private fixture
workspace even when macOS defaults the panel to Documents.
The existing `scripts/test-native-windows.py` covers a separate native lifecycle
fixture, not those complete editor journeys.

Use `--capture-review` to retain the initial toolbar/root framebuffer and, with
`settings_search`, `tool_defaults`, `controls`, `preview_find`, `tables`, `problems_focus`, `terminal_find`, `command_palette`, `images`, `explorer_toggle` or `all`, the affected settings/popup/search framebuffers for separate inspection. This is opt-in, never a visual
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
python3 scripts/test_desktop_ui_versions_contract.py
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

The Find journey also resizes the focused native popup twice through accessibility
and minimizes it, verifying that its document owner receives these operations.
This exercises the same macOS window attributes used by external window managers;
it does not execute a user's personal Hammerspoon configuration. Settings remains
an independently managed window. The Settings search journey now types invalid
JSON, verifies Save is disabled, reloads, saves a valid edit, and
uses Cmd+F to return to its search field without opening document Find.

`python3 scripts/test-desktop-ui.py --journey rectangle` is an optional local
integration check, excluded from `all`. It requires a running Rectangle and reads
its existing `almostMaximize` and `bottomLeftSixth` bindings from macOS preferences.
It sends those real shortcuts with Find focused and checks that the document
resizes while Find retains keyboard focus. It does not edit Rectangle preferences.

### Linked snippets

The `snippets` journey first filters Settings’ full-height snippet list, saves a row’s enable/disable toggle, opens its exact source with Edit, deletes just that row and restores the library. It then opens the dedicated Snippets window from Settings, verifies that source typing and Cmd+F search stay in that window, and uses Cmd+W to close only Snippets while preserving Settings and the main document. It searches and filters the master/detail library, edits and disables a bundled fraction, confirms that disabling removes its live completion, re-enables and expands the edited template, duplicates it, removes both the copy and original, and restores the defaults. It creates custom Typst and TeX templates through the highlighted source editor, changes each opening/closing delimiter pair using the explicit Rewrite markers / Keep source choices, saves, disables/re-enables, reverts an unsaved source change, and removes them. Default-looking `{{1:literal}}` text remains literal when the Typst snippet chooses another delimiter pair with Keep source. Exact document checks cover literal dollar math, backslashes and old `${...}` text, linked-field edits, undo/redo, provider symbols, and language/context scoping. The defaults use `{{1:value}}` in both languages; custom delimiter pairs are exercised independently. Bundled fractions are excluded from prose and comments; auto-miTeX dollar regions require TeX insertion while ordinary Typst markup retains Typst snippets. Optional framebuffer capture records both language forms of the dedicated manager with the full library visible (`snippets` framebuffer target). Run it with `scripts/test-desktop-ui.py --journey snippets`.
The native driver rejects a locked desktop before attempting input.

### CJK input and native candidate anchors

The `ime` journey reads the actual AppKit `NSTextInputClient.firstRectForCharacterRange` result at the top and at a scrolled line of a 200-line document, and requires that native hint to follow the visible caret. It temporarily selects the already-enabled Pinyin - Simplified source, types physical `n i h a o` keys, requires exact pending `nihao` text and checks the actual native anchor against a fresh IME caret inside the visible source viewport, then requires Space to convert it to exactly `你好` and checks the updated native hint. A collapsed preedit caret is valid; unconverted Latin text cannot pass the Chinese commit check. A Settings edit to 125% interface scale verifies the native coordinate conversion, then restores the original scale. A scoped driver restores the original input source on completion, failure or pipe closure; it never enables sources or changes input-method configuration. If Pinyin is unavailable, the check reports that limitation instead of skipping. The optional `--review-preview` pause leaves actual IME candidates visible for manual observation. Native hint geometry and committed input are interaction evidence, not a screenshot of the system candidate window.

The journey separately checks native committed CJK punctuation with the app's conversion preference off and on, then restores that preference in the isolated test profile. This does not assume a particular punctuation preference in the user's input method. Run `scripts/test-desktop-ui.py --journey ime`.

### Drawing recognition

`--journey drawing` maximizes Draw symbol, draws three native pointer strokes,
waits for real Detexify predictions, resizes the window, checks the retained ink,
and clicks the first insertable ranked prediction into a scratch document. It then switches to Detypify and Unicode output through Settings, checks that the retained ink is re-recognized by only that engine, inserts a result, and clears the canvas. The expected insertion comes from the same language/preference resolver used by the displayed row. No model result or
canvas state is injected into the app.

### Folding, selection wrapping and preview keyboard ownership

`--journey folding` covers both the short fold fixture and a 290-line document
with a 180-line code block. It wheels back to the fold header while leaving the
caret at the document end, collapses/expands without moving that caret or
scrolling to it, then folds with the caret inside the block and checks the visible
header destination over repeated frames. The source must remain unchanged.

`--journey selection_wrap` selects Unicode text and types every supported bracket
or symmetric delimiter, then checks native undo and redo. Deterministic editor
coverage also exercises Typst, TeX and plain text, disabled pairing, paste, and
selection deletion followed by typing in the same input batch.

`--journey preview_keyboard` verifies the effective PDFium and Tinymist backends.
Compilation, pointer hover and wheel scrolling must preserve source typing.
Switching a focused preview to code-only mode must restore source keyboard input.
Three click/Escape round trips per backend check keyboard ownership. Tinymist's
`t` shortcut must invert only the focused preview; typing after Escape must edit
source without changing preview colors. `--capture-review` captures the PDFium
focus outline and badge once; the root framebuffer cannot display the embedded
Tinymist WebView, whose focus and keyboard behavior are checked separately.

`editor_preferences` opens Settings Help, expands each of the six bundled-tool
references with native clicks and scrolling, requires the first and last option
text and unchanged source,
checks Help's Find routing, changes and restores indentation guides, highlighting uses of local definitions and word wrapping, opens the
custom highlight section, and verifies that source text is unchanged. It also runs
the focused `typing_particles` checks in an isolated scratch tab. Particle inspection
reports every keyboard action's independent particle/shake/flash/glow settings,
the last emitted particle style, live counts by the style stored at birth, and
cumulative bursts by editor action. The real Settings controls must change only
the chosen action; Copy's Railgun emission, for example, must not inherit Typing's Plasma choice. Each input must increment
exactly its own action count once; Copy is checked even though source does not
change. Clipboard effects are observed immediately after the real key/menu event,
before waiting for the clipboard receipt, so a slow receipt cannot hide the finite
animation. Navigation, Settings input and no-op commands must leave all source
bursts quiet. The native journey waits for bursts to expire before changing
Settings, so it does not depend on completing a window roundtrip within the 640 ms
particle lifetime. Mixed live bursts retaining their original styles are covered
by deterministic state/GPU tests. Nonparticle feedback has separate cause counters,
active-effect labels and bounded render amplitudes. The journey begins read-only
observation before posting a native key so the short Flash cannot expire during
the driver's key-up delay. With particle generation Off, Shake/Flash/Glow on three different keyboard actions
must render their own feedback once without changing particle counters or GPU work;
all three then expire without extra input. After the individual-Off check, every
feedback kind is configured again for foreign-focus and master-disabled negatives.
Deterministic editor tests additionally cover both source and editable diff views.
Use `--capture-review` to retain fresh
Help/settings/editor viewport framebuffers.
The `controls` journey also rejects an oversized short toolbar tooltip and, when
capturing, focuses that tooltip's native viewport before the screenshot shortcut.

### Observer-free effects pacing

`effects_pacing` is a focused performance lane, separate from `typing_particles`
interaction coverage and the aggregate `all` run. It uses saved ~250 KB plain text
and ~240 KB Typst source with 400 definitions, Low Explosion effects, and a small
live Tinymist preview. Real Settings JSON edits and a restored clipboard receipt
verify the preference; native `preview_zoom_overlay.enabled` additionally verifies
its application to the Typst preview. Plain text has no native preview.

For each document it measures one first and six warmed Typing bursts, followed
by the same Copy sequence on unchanged source. Each native action is followed by
0.9 seconds with **no snapshots, screenshots or application requests**, then one
observation. This matters because normal inspection explicitly requests repaints.
The runner alone enables `TIPTOPTYP_UI_TEST_FX_TIMING=1` for this lane; the
`desktop-ui-tests` build retains at most 256 in-memory GPU callback samples.
Sequence continuity rejects lost new samples rather than discarding slow bursts.

`effects-pacing.json` retains the actual build/backend/theme, fixture hashes,
source sizes, per-burst samples, and callback intervals split at shader age
320 ms. The animation and callback clocks must retain a constant offset within
1 ms: both sample the same monotonic instant, with f32 conversion allowed. This
is a clock-consistency assertion, **not a frame-rate threshold**. Callback cadence
is not GPU execution time, compositor presentation or input-to-pixel latency.
Typing emits 24 particles and Copy 12; Copy also avoids source edits.

To diagnose snapshot overhead with the same preserved optimized binary:

```sh
python3 scripts/test-desktop-ui.py --journey effects_pacing --release --executable /absolute/preserved-app --effects-preview-snapshots on
python3 scripts/test-desktop-ui.py --journey effects_pacing --release --executable /absolute/preserved-app --effects-preview-snapshots off
```

The option defaults to `on`; `off` is accepted only for `effects_pacing`. Each run
has fresh preferences/workspace and separate evidence. Preserve the binary's
original optimized build receipt; `--executable` cannot establish its provenance.
Do not run builds, CPU sampling or another GUI workload during a cadence run.
Use a separate run for CPU-stack attribution. See the
[recorded pacing and snapshot-control results](performance.md#native-effects-pacing-19-october-2026).

### Native TeX editing

The three `tex_*` journeys, `harper_math_prose` and `harper_settings` run individually, together with `--journey tex`,
or as part of `all`, against named files. Initial settings are changed through the real JSON editor and restored after
the journey. `harper_settings` edits the dictionary and capitalized-name filter through their Harper configuration controls in Grammar & spelling,
reopens the dictionary before removing its entry, and requires a fresh real Harper result after every change. They do not inject diagnostics, completion responses or editor
handlers. The intelligence journey requires working pinned TexLab and Badness;
Harper runs normally in its background worker. The Typst fixture also keeps clean sentences continuous through `#emph`, `#strong` and `#text` bodies, then checks a repetition crossing an `#emph` boundary through real typing. Both math-prose fixtures include Barbato/Romito at the beginning of a sentence and repeated straight/curly possessives in ordinary prose; none may produce spelling diagnostics. Its completion fixture includes
a full document and a locally defined command, so it does not require external
package completion databases. Read-only observations include provider readiness,
diagnostic counts/current revision and fingerprints, never diagnostic text.

These are interaction checks, not typing-latency benchmarks or PDF rendering
proof. TeX formatting, SyncTeX navigation and mixed Typst/TeX preview transitions
remain separate coverage gaps.

The `tables` journey additionally checks real Typst variable-cell samples and
TeX macro samples, attempts to type into their read-only cells, edits a literal
neighbour, and verifies that Apply preserves the original variable expressions.
Sample fingerprints come from the actual configured compiler result.

The separate `arxiv_tables` journey uses the unchanged table environments and
attribution in `tests/fixtures/arxiv-tables/provenance.json`. It opens only copies
inside the private journey directory. The SciPy roadmap and optimizer comparison
exercise grid editing around real rules, macro cells, wrapped lines and omitted
trailing cells; the Platonic metrics excerpt exercises grid editing with spans,
row formatting and citation wrappers. Platonic editing types only the projected
cell body, keeping its original group and spacing declarations outside the field;
exact full-file checks still require those wrappers to survive. Their exact
16×2, 13×14 and 10×6 grid shapes
are checked before editing. Offscreen cells are revealed with bounded native
horizontal-wheel input; stale targets and clipped slivers cannot stand in for a
usable cell, and three advancing child frames with stable positions precede the
click. Each draft is cancelled once, then applied and undone. An additional SciPy
optimizer pass requires its last-column version cell to begin offscreen, reveals
it with actual right-wheel input, changes only `1.0` to `1.1`, and checks exact
whole-file Apply and Undo. Exact full-file fingerprints prove that an edit changes only the chosen
literal and preserves the rest of the excerpt. The fixture contracts also verify
the attributed table hashes. This lane can run without the larger synthetic
`tables` journey and does not require resolved bibliography or appendix references.

The `math_grids` journey opens private copies of
`scripts/fixtures/math-grid-editor.tex` and `.typ`. It checks six mathematical
grids: TeX `array`, `pmatrix` and `cases`, and Typst `mat`, `vec` and `cases`.
For each, the source context menu must offer the grid editor at a formula,
and its kind, dimensions and complete cell fingerprints must match. Ordinary
symbols, Greek names and fraction commands remain editable; they must not be
mistaken for computed prose cells or trigger compiler samples. Each draft is
cancelled once, then applied and undone. The draft fingerprint checks the raw
formula without added math delimiters, quotations or content wrappers; exact
whole-file checks also retain alignment options, matrix delimiters, whitespace,
imports and surrounding equations. The Typst matrix also grows from two to three
rows through its native row-count control; the new zero cells and complete
structural source edit are checked before one exact Undo. Optional fresh child framebuffers can be
inspected separately. The TeX fixture includes more matrix and alignment forms
for manual inspection beyond the six native cases.

The compact-grid cases first require the native Table editor to own focus,
resize it to its 620×420-point minimum, and observe three stable native size
samples within a small titlebar tolerance while the parent sizes stay unchanged.
They then edit a long cell and its neighbour without changing the source, require a 36-point row
pitch, and scroll a 36-row grid using native wheel input. Fresh `table.grid`
scroll geometry and Apply/Cancel hit rectangles must share the current child
frame; the footer must remain fixed and outside the grid during scrolling.
Cancel preserves the exact source; Apply and one Undo restore the exact fixture.
TeX keeps Tab for source indentation, while Typst's grid Tab navigation is checked.

The same journey proves unsupported detection with a whole-generated Typst table
and TeX `*{\colcount}{l}` columns. It requires three advancing frames from a newly opened
source context menu, after verifying the exact fixture source and intended caret
character offset. It rejects a current **Edit Table…** item even if disabled, and
ignores stale targets from older menus. Dismissing the menu must preserve the
complete document. Every opened workbench must expose a current grid and footer,
with no source-editor controls in that child frame.

The table journey also opens tabularray and siunitx tables, verifies actual
compiler samples, attempts to alter locked macro cells, edits their literal
neighbours and checks that package options survive Apply/undo. It also starts with a missing siunitx import, verifies that Cancel leaves the preamble unchanged, checks automatic import insertion on Apply, restores both changes with one Undo/Redo, and reapplies without adding duplicate imports.

The completion journey also requests installed fonts in a real Typst `text(font:)` argument, waits for the private name atlas to finish, captures the list when requested, and accepts the selected family without modifying the quote boundaries. Font rows show only the family name in that font.

### Directional window backgrounds

`window_background.blur_fade` and `window_background.transparency_fade` are independent
JSON options: `none`, `left`, `right`, `up`, or `down`. The Appearance dropdowns show
`none` as **Uniform**. The chosen edge uses the configured End value; the opposite edge uses Start.
`window_background.blur_radius` and `window_background.blur_end_radius` set blur
endpoints from 0 to 50. `window_background.opacity_percent` and
`window_background.opacity_end_percent` set opacity endpoints from 0 to 100;
100 is opaque, and 0 is fully transparent. Start and End may increase, decrease,
or match. Uniform uses Start everywhere. Defaults retain the original 8→0 blur
and 80→100 opacity fades. Fullscreen uses an opaque background with both effects
off, preserving the saved directions and endpoints.
The `background_fades` journey checks native controller acceptance and UI routing;
its optional fresh framebuffer captures still require inspection and do not
establish the composed desktop blur appearance by themselves.

The native preflight reports the persisted macOS scroll-to-zoom preference when
available. `cursor_wheel_zoom` reports its check as unavailable before posting
Control-scroll when that OS feature is enabled; it never changes the preference.
The `partial_dividers` journey retains independent drag coverage, while the
owned-WebView probe exercises native pinch phases and source anchors without
system-reserved global input. An unavailable wheel check is never a passing one.

### Optional selected-text Liquid grammar

Liquid downloads and prepares its optional files automatically on the first selected-text check.
The app runs a native ONNX Runtime helper on Intel (macOS 12+) and Apple Silicon (macOS 14+); Python is not required.
Then run the model-dependent lane explicitly:

```sh
python3 scripts/test-desktop-ui.py --journey liquid_selection \
  --liquid-model-dir "$HOME/Library/Application Support/dev.tiptoptyp.editor/liquid" \
  --capture-review
```

For only the long-selection streaming checks, use `--journey liquid_streaming` with the same model folder. The full `liquid_selection` lane retains its four short language/style cases plus both streaming cases. Logical prose-line selection uses native document EOF and trims the known outside suffix; it does not depend on the visual row end under word wrap.

The supplied folder is a read-only artifact reference: it must contain
`onnxruntime/lfm25-encoder-350m-onnxruntime-v1/manifest.json`, `liquid.onnx`, the current Mac runtime, and
`tokenizer.json` from the current native model. The journey selects a fresh,
initially absent cache inside its disposable directory through Settings JSON.
Its first Typst request must automatically prepare the files and complete
inference, with no setup button. It records the prepared
manifest digest, declared asset hashes and sizes. These must match the reference.
The app must use its compiled, pinned download catalog; neither bundled model
resources nor a development `toolchain/liquid` directory supplies a fallback.
This first-use lane needs network access to the published optional assets and
allows up to 15 minutes for download/preparation. No external cache is modified
and no Python runtime is created. The remaining Typst/TeX and diff-style
matrix reuses the newly installed model. Two additional cases select a single
wrapped paragraph containing 180 unchanged sentences, an early known grammar
error, and protected math. They require an actual unfinished multi-chunk review
with visible corrections and a fresh disabled Apply button, using model-measured
chunk/token progress rather than estimating tokens from text length. The final
Apply and one Undo must preserve math, wrappers, and all outside text.

This lane is excluded from `all`; its lightweight failure-detector contracts
still run in ordinary CI. It changes preferences through the real JSON editor,
uses disposable named files, and restores its settings. Framebuffers are
separately inspected evidence, not proof of native child-view composition.

## LaTeXdiff wizard

```sh
python3 scripts/test-desktop-ui.py --journey latexdiff_wizard --release
```

This opt-in journey requires a working `latexdiff` on PATH or in MacTeX. A missing
binary is a failed/unavailable interaction check, never a skip or pass. The
wizard is opened through **Create LaTeX Diff** in the real command palette. All
fields are typed into fresh focused native controls, and their path/options
fingerprints must match the inspection state before navigation. The private
fixtures have spaces in their names and contain both changed prose and math.

The journey checks Cmd+W ownership, Files → Output and options → Review, Back
retention, an attempted output/input collision, and recovery with a fresh output
path plus `--math-markup=1`. It reads the actual generated file and requires
additions/deletions in the document body, so preamble macro definitions alone
cannot pass. Opening the result must expose its complete exact source in a new
TeX tab; neither original input nor the previous editor buffer may change.

A second run temporarily chooses a private sleeping shell executable through the
real Settings JSON editor. After its PID receipt and busy state are observed,
the native Cancel button must stop/reap that process without writing output.
The wizard closes and the executable setting returns to automatic detection in
the disposable profile. The test never edits the user's settings or runs a
configured shell against a real document. `--capture-review` additionally saves
fresh wizard framebuffers at each step for separate pixel inspection; the
semantic/native journey does not itself claim visual verification.

## Local TeX documentation

`--journey tex_documentation` requires executable Texdoc on an absolute PATH
entry or at MacTeX’s stable `/Library/TeX/texbin` link, plus the installed amsmath
PDF manual. Missing tools/manuals are unavailable or failed checks, never a pass.
The journey opens TeX Documentation from the command palette while editing a
Typst fixture, types into its real search field, checks Escape and Cmd+W ownership,
submits with Enter, and opens an actual local Texdoc result. It verifies that the
manual has its own tab and that the paper retains its designated preview; closing
the manual restores the unchanged source. It does not launch a browser or download
manuals. This optional journey is excluded from `all`. Native interaction evidence
is separate from framebuffer or composed-window visual verification.

## Citation import review

`--journey citation_import` uses a synthetic DOI and a bounded metadata fixture
in the runner’s private directory. Only the `desktop-ui-tests` build recognizes
that exact DOI and explicit fixture path; production builds always use the normal
DOI/arXiv metadata services. The fixture passes through the real BibTeX parser and
DOI verification on the lookup worker. The native journey opens the palette, checks
Cmd+W ownership, selects real source text, reviews and edits the citation key and
BibTeX, appends to a pre-existing bibliography, inserts the citation, and verifies
that one Undo restores the selected source. It verifies the actual bibliography
bytes and preview ownership. It is an offline UI/storage test, not evidence that
the live metadata services were reachable. This focused journey is excluded from
`all`; no user bibliography or document is touched.

### Bundled local LTeX+

`python3 scripts/test-desktop-ui.py --journey ltex` uses the bundled server and Java runtime (never a diagnostic mock), without requiring an external launcher. The runner restricts PATH and supplies invalid inherited Java paths so the real check must use the app's runtime. It verifies the effective Bundled mode, clicks Redownload LTeX+, observes the busy state and available Cancel control, waits for the architecture-specific pinned distribution to download and verify, and proves that source and applied preferences remain unchanged by reopening the complete validated Settings JSON. This explicit journey needs network access and downloads approximately 315 MB (ARM) or 316 MB (Intel), with a three-minute resource budget. It then enables the checker through Settings, checks both TeX and Typst with mapped rules and correction suggestions, retains stale results during native source replacement, waits for the corrected clean response, checks that the original files were not written, then disables the provider. This focused resource journey remains excluded from `all`. A development build needs the staged `toolchain/ltex` package; the packaged app uses its own resources.

To check a custom server instead, pass `--ltex-executable /path/to/ltex-ls-plus/bin/ltex-ls-plus`. The journey selects Custom path through the real Settings controls; it preserves the inherited environment for that explicit override. A missing custom executable fails preflight. Resource cancellation and repeated-click suppression have deterministic Settings UI coverage. The custom-server journey skips the bundled resource redownload. `--capture-review` captures fresh expanded Grammar & spelling settings before and after resource recovery; inspect those PNGs separately from interaction results.
