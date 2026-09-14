#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")
#show "[ ]": box(stroke: 1pt, height: 0.8em, width: 0.8em)
#show "[x]": box(stroke: 1pt, height: 0.8em, width: 0.8em, fill: green, [#set align(center);x])
= running todo list

Keep this list as the source of truth. Every task has a permanent number and
`[ ]` (open, partial, unverified, or deferred) or `[x]` (completed). Never remove
completed tasks or renumber existing tasks; append new tasks with the next
unused number. Resolved and audit notes must cite the relevant item numbers.
Update the checkbox only when the entire task is complete and verified.

1. [ ] (Deferred) there should be a proper pdf preview option that is just a proper pdf viewer. maybe a webview?
2. [ ] (Deferred) perf

3. [x] typst overrides should be in appearances subsection
4. [x] the color indicator in typst overrides should always be the current color, not faded out if its 'theme/override'. In fact there should not be a 'theme/override' button, since if we want to return to the theme we can just hit the reset button
5. [x] somethings not right with alignment, see pic
6. [x] lots of tooltips in typst overrides popup kinda suck. Too big, not useful. If useless just delete it, otherwise tighten the space
7. [x] next to the theme name in the selectors, we should be able to see at a glance a colour pallette of the theme.
8. [x] allow customising UI fonts
9. [x] cmd +/- should adjust UI size. Also put a slider for this in settings
10. [x] syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ.

11. [x] if we are using a variable width font, add support to set the weight (typst overrides, and UI)
12. [x] change so that: allow triangular area from cursor to tooltip / error window to not dismiss. When mouse is over the popup, we can scroll.
13. [x] also add a keyboard shortcut to make the popup appear with keyboard focus (for scrolling up/down). Esc to unfocus and dismiss. There should be a subtle outline to indicate focus (as well as mouseover)
14. [x] I noticed that when I use a ```typ ...``` code block, it is not syntax highlighted correctly. Fix
15. [x] syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ. (Duplicate of item 10; keep both statuses in sync.)
16. [x] Mitex and cmarker integration: autodetect strings/raw strings inside `#mi(...)` etc to syntax highlight with tex or markdown. Note that you may need to put the text in math mode or text mode depending on the mitex command
17. [x] for other colour formats (luma etc) we should give that text the appropriate fill, like we do now for ` #rgb(“hex number”)`
18. [x] package manager - inspect available packages, locally installed packages
19. [x] detect when file changed outside app. If we dont have any changes queued to be saved, then reload the file
20. [x] allow font ligatures in code editor
21. [x] image / pdf preview on hover (both in code, when we include images as part of the code, and in file explorer)
22. [x] configurable shortcuts everywhere. add a popup for this in settings
23. [x] add searcg to all everything - explorer windows, settings,
24. [x] view > … needs to be in the title bar as well, and also none of the shortcuts are appearing - add
25. [x] allow us to toggle off the title bar dupes of the menu bar items
26. [ ] (Deferred) once in a while the text everywhere breaks (see pic) some sort of leak?
27. [x] after some time, the find/replace UI is not visible but is clickable. I realised this is because it is rendering behind the code editor panel. we shouldnt even make space for the find / replace UI - it should just be on top of the code panel.
28. [x] keyboard shortcuts when find and replace is active should work on find and replace text fields. and similarly for other text fields
29. [x] cmd+F / the Find button should toggle the find/replace popup, not just turn it on.
30. [x] there should be a button to toggle find and find/replace
31. [x] there should be options to allow case (in)sensitivity and regex (which we should be able to right click and get a cheat sheet). add single-symbol wide buttons for these
32. [x] if we right click and its already ‘use for preview’d then we should be able to toggle it off
33. [x] text highlight of file names is not readable, adjust color scheme
34. [x] should be able to right click file in explorer -> open in new window
35. [x] cmd-/ to toggle comment out current or selected line(s)
36. [x] shortcuts should be configurable (Duplicate of item 22; keep both statuses in sync.)
37. [x] move the problems button to the rightmost slot (matching with default shortcuts)
38. [x] remove ‘cmd/ctrl’ everywhere in popups/tooltips its just cmd
39. [x] fallback for CJK fonts
40. [x] autocomplete
41. [x] text is selectable in too many places
42. [x] file menu in menu bar does not match file in title bar. It should be single source of truth
43. [x] file explorer subsection separators should be draggable
44. [x] when closing the file explorer it should first stop showing all the contents before minimising
45. [x] there should be another subsection in the explorer, the tags/references
46. [x] color code various filetypes (various categories should be: .typ, other text-based files, pdfs and images, folders, and others) make sure you use colours from the theme
47. [x] the ttt in settings window is no longer left of file name in main window(s)
48. [x] the top right X in settings should not be there - use the Mac OS traffic lights
49. [x] selecting a file in file explorer resets the file explorer scroll amount. Why? Opening a file cannot trigger more files to appear. It should not redraw. If it does, at least restore the scroll
50. [x] the current line background should be also used for the current file. The currently opened file should also use an actual heavier UI-font face, not only stronger text color.
51. [x] find and replace shortcut just gives find.
52. [x] we should (by default) have the current scopes and the current sections/subsections etc be "sticky rows" at the top so that we have context for what we are looking at. each level should give a further row that is persisted to the top. For instance if we are in ```typ
  = first section
  // <many lines>
  == subsection
  #let a = {
  // <many lines>
  // <cursor is here>
  ```
  then overlayed at the top should be
  ```typ
  = first section
  == subsection
  #let a = {
  ```
  (together with the line numbers). Basically another code panel floating on top but with those lines. There should also be line numbers, and clicking on the sticky row should jump in main code panel to that row
53. [x] sync back from code ->  preview should be one of the right click options
54. [x] right click menu if possible should use native right click. If not, at least the text should not be centered, it should be left-aligned like normal
55. [x] special detection of typst functions: set text(font: ...) should allow us to right click on font: … and then have a scrollable selector
56. [x] allow using fonts in the root of working directory or anywhere in the working directory if it is not a performance hit.
57. [x] Table maker / editor on right click (with detection of tables)
58. [x] the tooltips on hover should be wider. Maybe more like 80 char wide
59. [x] remove unsafe rust as much as possible
60. [x] it should format on manual save (cmd+s) (but not auto save)
61. [x] if we have a file set to be used for preview, then when we switch to a different file, we dont need to restart the preview window. Also, we should not close it even if we open a non-typ file. thats the point of the used for preview setting - so that we can edit other child files like a .toml or other .typ files
62. [x] Any part of the UI with this tiny font size like that used in the file path of the file explorer should have a bigger font. Never use this small font. Also don’t use allcaps or smallcaps, not our style.
63. [x] in addition to the pdf ready indicator i also want all the greentext on the side e.g. applied theme saved automatically. and everything should be timestamped.
64. [x] we should reserve some space for the `*` in the name when a file is not yet saved, sot hat when it does appear, it doesn't shift the UI elements.
65. [x] double clicking on a line in the Problems panel should jump to that line in the code panel
66. [x] the logo icon should be redone to match the settings 'ttt'
67. [x] on closing the app ran with `cargo compile --release` it should not lose the icon as it disappears?
68. [x] Maintain UI observability and regression tests with targeted fresh screenshot inspection when pixel-level behavior is material. Capture detailed UI scenes in two representative themes (Catppuccin Latte and Mocha), and a generic main-window shot for each remaining theme.
69. [x] code panel tooltips have disappeared. very odd?
70. [x] if i am editing test.typ and i set todo.typ to be used for preview, then it should refresh the preview window to use todo.typ. this is not the current behavior, and if i then switch to opening the todo.typ file, the preview will still be stuck at test.typ
71. [x] cmd+M should minimise the window, and so on for the usual mac os shortcuts
72. [x] should have two proper dropdown for all fonts in settings (one for UI and one for code) - ability to choose any system font. In addition as mentioned above I think, we should search the local dir for fonts that we can use in the document.
73. [x] the "Applied Catpuccin Latte" etc text is stuff i also want in the log. and keep the last 100 entries instead
74. [x] font weight should not be adjusted until after mouse off the scrollbar as the UI shifts with the change causing feedback
75. [x] remove the colour palette swatches from Settings completely
76. [x] vertically centre the font-weight slider and give the code font its own independently persisted weight control
77. [x] vertically align the Syntax label, live samples, and Reset buttons in Typst overrides
78. [x] make popup focus and ownership robust so using or closing Settings cannot leave popups broken
79. [x] keep the recent-status popup close to the status bar and right-align its timestamps
80. [x] remove the obsolete pre-tiptoptyp app bundle, registration, build outputs, and compatibility namespace completely
81. [x] capture the complete maintained screenshot gallery from one app session instead of reopening the app for every image
82. [x] redraw the refresh and used-for-preview eye vector icons so the refresh arrowhead does not overlap its body and the eye is rounded rather than angular
83. [x] make tooltip Markdown links display only their linked text and open in the system browser; support opening external Typst `#link(...)` targets with Cmd+click or the editor context menu; and open preview links in the browser
84. [x] prevent selecting file-tree names, the `ttt` logo, and the title-bar filename; add file-name, absolute-path, and workspace-relative-path copy actions to file context menus; and show Cmd+1 through Cmd+5 shortcuts in the native View menu
85. [x] add a Pause/Resume control for automatic preview updates and make Compile write the effective Typst entry's PDF beside its source, using an output picker only for an unsaved document
86. [x] fix the native startup deadlock that leaves the application unresponsive before any UI appears
87. [x] allow dragging a file into the file explorer to add it into that folder. current file behavior should be scoped to falling on the code editor instead
88. [x] we should be able to right click on a file and Show in Finder
89. [x] when we reload from external inputs to the file, we should not need to reload the tinymist server since it was listening. I think just the code panel needs to be updated.
90. [x] if we open a link in an external window then the greentext in the bottom should log that
91. [x] I got this panic: ❯ cargo run --release
  Finished `release` profile [optimized] target(s) in 0.55s
  Running `target/release/tiptoptyp`

thread 'main' (37498614) panicked at /Users/calvinkhor/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/num/f32.rs:1505:9:
min > max, or either was NaN. min = 9.0, max = 8.0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
I was using two different windows with two different workspaces. Possibly i was choosing a file. I guess we need to make sure all the stuff between windows are completely separate
92. [x] the file preview should appear on the right of the cursor and in particular not on top of the file tree so it is easy to browse
93. [x] there should be a keyboard shortcut (or two?) to open/close the tooltip under the mouse cursor and under the keyboard cursor
94. [x] We need to also be able to delete files from the explorer
95. [x] Git integration - allow the most basic git functionality, enough for basic use.
96. [x] when we are editing a non-typ file, grey out Code Split Preview buttons
97. [x] completions should filter as i type more chars.
98. [x] font completions should similarly filter (all filtering should be fuzzy match), I should be able to type ```typ #set text(font: "``` and then have the full list. current behavior is the completions show when I type ```typ #set text(font:``` but disappear on the space.
99. [x] show a preview rendered in the selected font, both in font completion and font selection in Settings (clarified 2026-09-12)
100. [x] there should be an uninstall button in the package UI
101. [x] we should be able to access packages UI from View title bar
102. [x] we should be able to rename from title bar File > Rename
103. [x] there should be a button to go to the package desc on the website
104. [x] Currently, we need to wait for the online packages to be fetched before any is shown. The installed packages should immediately appear, followed by the online generated list once they are ready.
105. [x] when we comment a line out with cmd-/ the '//' should be added at the start of the line, not after the whitespace and at the first non-whitespace char.
106. [x] There should be better completions for references. Firstly, the thing on the left should be the code, and after that should follow the other data. Secondly, it should filter as I type.
107. [x] find and replace should appear on top of the sticky rows, not push them down
108. [x] the image/pdf preview should not have so much text. just the image alone is enough.
109. [x] the title-bar options (file, edit, view) do not mirror the title bar ones. make it so that it is a error, type error or other strict testing behavior that these two must always match. use a single source of truth.
110. [x] browsing font completion and font-picker previews must not rebuild the application fonts or flash text throughout the UI
111. [x] polish the Git window with right-aligned buttons, a visible diff viewer, and Unstage all
112. [x] show Git status in Explorer and clickable change markers beside line numbers, with independent state and diff windows for every document window
113. [x] audit correctness and maintainability across document workflows, file operations, Git, and background work; fix verified defects and redundant scans
114. [ ] crash recovery: restore unsaved buffers and all document windows after an unexpected exit
115. [ ] Git chunk actions: stage, unstage, and revert individual chunks, with keyboard navigation between changes
116. [ ] workspace search and replace: preview replacements across files, support regex capture groups, and provide undo
117. [ ] large-project performance: share repository status between windows and measure indexing, preview, and typing latency (extends deferred item 2)
118. [ ] PDF polish: document search, thumbnails, and reliable position restoration (extends deferred item 1)
119. [x] extract settings, editor rendering, and window orchestration into focused modules with enforced ownership boundaries
120. [x] implement refactor.typ R1–R8: sealed document mutations, workflow states, owned tasks, preview provenance, headless core, coordinate types, presentation ownership, and explicit write outcomes
121. [x] The image preview int he file explorer appears to the right of the file name position. But this is not exactly right - it should appear to the right of the fine explorer panel. This is different if the name is so long that we have to scroll the panel to see it. As currently it uses the file name length, this is past the panel boundary.
122. [x] the currently selected file should not have a bigger font size.
123. [x] it shouldnt say pdf is ready in X ms, it should say preview is ready
124. [x] on first open, it says pdf is ready in 0 ms, even when it is not true. fix
125. [x] when searching with find/find and replace, it should on each keystroke automatically jump in the code panel to the first match. currently it only jumps when we hit enter.
126. [x] when searching with find/find and replace, all matches should immediately recieve a highlight. currently it only has a highlight on the current line. thats good, but we also need a different highlight color for the matches.
127. [x] popups in a main window stop working if we have another window open e.g. packages or settings window. the popups should either always work or at least work for the currently focused window.
128. [x] close diff button is superfluous, remove it.
129. [x] in fact lets not have a separate git hunk diff window at all. Have the diff show in a popup.
130. [x] the git window should instead be a subpanel in the explorer window.
131. [ ] the tags and references subpanel should be two separate panels
132. [ ] allow us to choose the order of the explorer panels in Settings. git panel should default to under files
133. [ ] (deferred) implement tabs
134. [ ] we should be able to click on the ttt logo in a window and have it open a color picker for the bg of the logo, which will help us visually identify each window.
135. [ ] when cursor is at a bracket/dollar sign/etc we should highlight the matching char.
136. [ ] when we type one of these bracket chars, we should by default automatically insert the matching char and place the cursor in between. If we backspace, from this, delete both. This should be configurable in settings.
137. [ ] we should also implement rainbow brackets. Each type of bracket pair (`[]`, `()`, `{}`,, and mixed brackets `(],[},` etc) should use a different cycle of colors. Allow us to choose palletes to cycle through for the brackets in settings.
138. [x] i noticed that if a popup appears because i hovered over something, then scroll, the scrolling is not performant, losing frames. Investigate and fix.
139. [x] too much space is reserved for the git hunk color marker on the side. It should take at most 1 char width or other similar small measurement.

= resolved in the 2026-09-13 easy backlog pass

- Items 121, 122: Explorer asset hover candidates now end at the visible panel clip, and the active file's stronger face keeps the shared content font size.
- Items 123, 124: Preview status text uses “Preview ready” consistently and suppresses zero-millisecond timing so startup fixtures cannot claim a completed PDF build.
- Items 125, 126: Find query edits select the first result immediately, and every result receives a syntax-preserving background highlight with a stronger color for the selected match.
- Item 127: Main-window popups no longer close just because a settings, package, Git, or chunk child window is visible; only overlays owned by the root editor can block them.
- Items 128, 129: File diffs toggle closed from the same row action, and hunk diffs now render in the owning editor's popup with Escape or dismissal closing the chunk; the separate Git chunk child viewport is gone.
- Item 130: Normal document windows render the Git panel as an Explorer subpanel; the separate Git child remains only for the deterministic Git window scene. View > Git also opens Explorer when it was closed, and a one-shot reveal overrides a stale collapsed section state.
- Item 138: Tooltip code blocks cache their highlighted `LayoutJob`s per document viewport and theme, avoiding repeated syntax work while scrolling a hover popup.
- Item 139: Git hunk markers now reserve an eight-point lane, with painted and clickable geometry constrained to that lane.
- Verification: Added explorer geometry, active-font-size, startup timing, layout-highlight, popup gating, diff-toggle, popup-routing, tooltip-cache, and compact-gutter regressions; focused tests pass.

= resolved in the 2026-09-07 pass

Items 1, 2, and 26 remain deferred. This history records completed work and partial
progress; the running checklist above is authoritative for completion status.
Focused tests cover the deterministic behavior described below:

- Items 3, 4: Typst overrides now live under Appearance, show the effective color, and use Reset to return to inherited theme values.
- Item 6: Removed redundant hover cards from override labels, color swatches, live samples, and Reset buttons; decoration controls retain only concise state-and-action hints.
- Items 7, 8, 9: Built-in theme selections show palette swatches; UI font choice supports the default, editor-monospace, or a validated custom TTF/OTF/TTC file, with interface scaling via Cmd-plus/minus.
- Items 10, 14, 15: Markdown tooltip blocks use syntax highlighting; `typ`, `typst`, and `typc` fences use the Typst highlighter.
- Items 12, 13, 58: Tooltip cards are wider, scrollable, clipped to the viewport, retain a root-local pointer bridge to the card, and support click or Cmd+Shift+Space focus, selectable copying, Escape dismissal, and a focused outline.
- Items 5: Preview geometry is clamped to the actual panel clip and zoomed previews preserve their viewport origin.
- Items 19: External edits reload clean editable documents and produce an explicit conflict notice for dirty documents.
- Items 24, 25, 29, 30, 31, 51: View commands, shortcuts, title-bar menu visibility, find/replace toggling, replace actions, case sensitivity, regex mode, and the regex help menu are wired through shared commands.
- Items 27: The find/replace bar is a foreground overlay and no longer reserves editor layout space.
- Items 32, 34: Explorer context menus can open files in a new window and toggle the designated preview file on and off.
- Items 35, 53: Cmd-slash toggles comments for the current or selected lines; editor context menus can reveal the source in Preview.
- Items 37, 39, 46: Problems is the rightmost view control; file labels use theme colors by document category; CJK fallback fonts are registered when available.
- Items 45: Tags and references are indexed and navigable from a new Explorer section.
- Items 48: macOS child windows use native traffic lights; non-macOS windows retain an in-content close action.
- Items 61: Renaming a designated preview entry recreates its Tinymist URI safely.
- Items 60: Saving an existing Typst file requests formatting when Tinymist is ready and persists the formatted source; auto-save does not format. The initial Save As flow still needs formatting integration.
- Items 61: A designated preview remains alive while editing child Typst/text/PDF/image files, and its state is restored when returning to source.
- Items 47, 63, 64: Status history has timestamps, dirty-title space is reserved, and the main title bar uses the settings `ttt` logo.
- Items 62: Small-text styling was removed from the application UI; supporting text uses the shared readable type scale.

Open work is tracked only by the unchecked entries in the running list above,
so this audit history cannot become a second, stale task list.

Item 68: Native screenshot capture was initially blocked by the sandbox's
LaunchServices/Services connection. Gallery decoding and manifest tests passed;
fresh capture subsequently succeeded during the audit below.

= correctness audit (2026-09-07)

- Items 5, 9: Corrected UI scaling to preserve native display density, with a 1x/2x display regression test.
- Items 31: Replaced the recursive regex parser with the maintained regex-automata engine already used by Syntect. Added Unicode, invalid-pattern, zero-width navigation, and pathological-pattern regressions. Replacement text remains literal.
- Items 19, 27, 28: Fixed external-change polling when Explorer is hidden, kept find overlays anchored to the visible editor, and preserved find-field focus while navigating matches.
- Items 60: Prevented stale formatting replies from cancelling a newer save request; cleared pending save-format intent on edits and document switches.
- Items 61: Preserved current preview-entry edits queued before Tinymist initializes and removed a duplicate restart when renaming the current preview entry.
- Items 10, 14, 15: Shared bundled syntax databases across tooltip highlighters and corrected Typst tooltip colors in light mode.
- Item 8: Validated custom TTF/OTF/TTC bytes before loading, retained a fallback chain, and kept editor syntax fonts independent from the selected UI font.
- Items 45: Used the Typst syntax tree for labels/references, excluding strings, comments, and raw examples.
- Items 61, 68: Isolated screenshot fixtures from persisted designated-preview settings without changing those settings.
- Items 5, 9, 27, 68: Native capture succeeded outside the sandbox. Inspected the fresh find/replace PNG under `.tiptoptyp/screenshots/agent-review`; retained native bounds and Retina scale evidence in `audit-geometry.txt` alongside it.
- Items 68: Regenerated and validated all 68 maintained gallery PNGs; inspected representative light/dark editor, find overlay, and settings captures. Formatting, strict clippy, 308 application/integration tests, and 7 packaging tests pass (2 environment-dependent tests remain ignored).

Open work is represented by the unchecked entries in the running list above.

= correctness audit (2026-09-07 tooltip follow-up)

- Items 12, 13: Native tooltip geometry now converts the monitor-space child position back to root-local coordinates for pointer bridging. Click or Cmd+Shift+Space requests popup focus, Escape or defocus dismisses it, and the focused card gets a subtle outline.
- Items 10, 14, 15: Selectable tooltip labels make rendered markdown copyable. `typc` fences parse as synthetic `#{...}` code-mode snippets, strip the synthetic delimiters, and are covered for keyword, number, and function roles.
- Items 12, 13, 10, 14, 15, 68: Fresh diagnostic and function tooltip screenshots were captured and inspected in Catppuccin Latte and Mocha. The full serialized suite passes with strict Clippy and formatting checks.

= correctness audit (2026-09-07 final follow-up)

- Items 12, 13: Retained diagnostic tooltip payloads while the pointer crosses the root-to-native-card bridge, so the popup no longer disappears during transit.
- Item 19: External-file polling now distinguishes a missing file from an unreadable file and reports deletion explicitly for both clean and dirty buffers.
- Item 61: Renaming a non-preview child document no longer tears down a live designated Tinymist preview; renaming the designated entry itself still recreates its URI intentionally.
- Items 29, 46: Find toggling is consistent across the shortcut, toolbar, native Edit menu, and title-bar Edit popup; project indexing and workspace colors now accept the same case-insensitive Typst extension policy as document detection.
- Verification: The corrected suite passes with 308 application/integration tests, 2 ignored environment-dependent tests, 7 packaging tests, strict Clippy, formatting, diff checks, 68 PNG decodes, and a fresh inspected find/replace capture.

= correctness audit (2026-09-07 tooltip handoff correction)

- Items 12, 13: Replaced the center-point triangle with an edge-to-edge pointer corridor, so paths toward the tooltip's top or bottom edge remain connected during the hover handoff.
- Verification: The tooltip bridge regression now covers a top-edge transit path; focused tooltip and coordinate-conversion tests also pass, and a fresh diagnostic tooltip PNG was captured and inspected.

= correctness audit (2026-09-07 competing tooltip handoff correction)

- Items 12, 13: Prevented another native hover source from replacing the active tooltip while the pointer is inside the original tooltip's bridge or focused popup. This covers diagnostic, semantic editor, and shared native-control hover payloads.
- Verification: Added a competing-hover regression covering bridge retention, focused retention, and dismissed cleanup; the full suite, strict Clippy, formatting, diff checks, and a fresh inspected diagnostic tooltip capture pass.

= correctness audit (2026-09-08 tooltip opacity correction)

- Items 12, 13: Limited handoff blocking to competing source rectangles, allowing the active source to continue its fade-in animation to full opacity while still protecting its payload during transit.
- Verification: Added same-source versus competing-source assertions; focused tooltip, bridge, Clippy, formatting, diff checks, and a fresh inspected diagnostic tooltip capture pass. One serialized full-suite run hit a transient fake-Tinymist handshake timeout; the failing test passed when rerun alone.

= correctness audit (2026-09-08 tooltip frame correction)
#box
- Items 12, 13, 58: Sized native tooltip viewports from the tooltip frame's complete inner, outer, and stroke margins, preventing the lower rounded edge from being clipped.
- Verification: Tooltip and popup-frame tests, strict Clippy, formatting, diff checks, and a fresh inspected diagnostic tooltip capture pass.

= correctness audit (2026-09-08 native popup edge correction)

- Items 12, 13, 58: Short native tooltips now shrink their vertical scroll area to the rendered content, so the lower rounded frame remains visible instead of reaching the child viewport edge.
- Items 12, 13: Pointer and focus hit-testing now uses the painted frame rectangle, excluding transparent child-viewport padding from hover handoff and clicks.
- Item 68: Native menu content budgets now derive from the rendered frame margins rather than hard-coded chrome values; the screenshot workflow now explicitly documents that viewport PNGs are not composed desktop screenshots.
- Verification: Added a popup content-budget regression, captured and inspected the exact single-line tooltip case and the File menu in a fresh Catppuccin Latte release run, and retained the native preview geometry trace.

= correctness audit (2026-09-08 tooltip handoff regression correction)

- Items 12, 13: Restored the full transparent native child viewport as the pointer handoff envelope while keeping painted-card hit-testing precise, preventing a competing tooltip from taking over before the pointer reaches the active card.
- Verification: Added a regression for competing targets across the child viewport, passed the focused tooltip tests, and captured and inspected a fresh traced Catppuccin Latte release tooltip framebuffer.

= correctness audit (2026-09-08 robust tooltip handoff correction)

- Items 12, 13: Closed the semantic-editor hover path that bypassed the shared native-tooltip handoff gate, and added identity-preserving geometry plus a 300 ms grace window so a transient pointer gap cannot dismiss the active tooltip before the pointer reaches it.
- Verification: Added a transient-gap regression, passed 7 focused tooltip tests, passed the full 286-test application suite plus integration and 7 packaging tests, and captured and inspected fresh release tooltip framebuffer `1788801929503-0001-diagnostic-diagnostic-tooltip.png`.

= correctness audit (2026-09-08 child-owned tooltip handoff and syntax correction)

- Items 12, 13: Made the native tooltip child viewport the sole owner of its pointer-presence state. Root-window pointer gaps can extend the route deadline but can no longer clear a child-owned hover, focus-transfer frames remain renderable, and identity checks prevent a closing tooltip from mutating its replacement.
- Items 10, 14, 15: Converted language-tagged Tinymist marked strings into real fenced Markdown, routed `typ` and `typst` through Typst source mode and `typc` through synthetic `#{...}` code mode, and syntax-highlighted inline code without leaking its style past a closing backtick.
- Items 10, 12, 13, 14, 15, 68: Added regressions for child-owned pointer state, stale tooltip identities, focus transfer, source-versus-code highlighting, and inline marker closure. Formatting, strict Clippy, 291 application tests (289 passed and 2 environment-dependent ignored), integration tests, 7 packaging tests, and all 68 PNG decodes pass. Fresh Catppuccin Latte and Mocha diagnostic and function tooltip framebuffers were captured and inspected.

= correctness audit (2026-09-08 pre-commit)

- Items 24, 29, 34, 51: Ordered modified shortcuts before their generic variants, preventing Open, Find, and interface scaling from consuming Open in New Window, Change Workspace Root, Find and Replace, or raster-preview zoom. Raster zoom now uses Cmd/Ctrl+Option/Alt with Plus, Minus, or 0 so item 9 keeps the unmodified Cmd/Ctrl+Plus and Minus contract.
- Item 28: Enter and Shift+Enter now move forward and backward respectively while returning focus to the find field; the broader cross-field shortcut audit remains open.
- Item 19: External-file observations are cleared on document switches and successful saves, preventing one file's missing/changed state from suppressing another file's notification.
- Item 60: Manual saves confirmed through the external-change overwrite dialog now request formatting too, while save-before-close/open flows remain immediate. Tinymist restarts invalidate outstanding save-after-format intent.
- Items 12, 13: Stale tooltip interaction state is identity-checked and cannot block all future tooltip sources when native geometry is temporarily absent.
- Items 39, 46, 63: Added the omitted ICO image category, made UTC status timestamps explicit with `Z`, and avoided reapplying UI font/style configuration every frame.
- Item 68: Added shortcut, timestamp, file-category, and tooltip-state regressions. Formatting, strict Clippy, 295 application tests (293 passed and 2 environment-dependent ignored), 26 integration tests, and 7 packaging tests pass. All 68 maintained PNGs validate; fresh Catppuccin Latte and Mocha find, diagnostic-tooltip, and status-log captures were inspected, with final native tooltip/preview geometry retained in `audit-geometry.txt`.

= resolved in the 2026-09-08 font and embedded-syntax pass

- Item 11: UI fonts now expose their real OpenType weight range or installed static faces, while editor syntax roles use numeric weights from 100 through 900. Typst override settings migrate the old bold flag to weight 700 without losing saved themes.
- Item 16: String and raw-string arguments to MiTeX math/text entry points and Cmarker render entry points now use embedded TeX or Markdown highlighting while preserving exact editor byte positions.
- Item 17: Literal arguments to Typst's RGB, luma, CMYK, linear RGB, HSL, HSV, Oklab, and Oklch constructors now show a readable preview fill. Detection is limited to Typst's actual namespaces and valid literal component types so unrelated custom calls are unchanged.
- Audit for items 11, 16, and 17: Corrected a clipped Settings weight row, removed axis-relative slider rounding that changed 400 to 401, kept deterministic screenshot runs from persisting QA-only settings, made the offline Cmarker fixture valid Typst, and tightened invalid color-call detection. Formatting and strict Clippy pass; 302 application tests pass with 2 environment-dependent tests ignored, alongside 26 integration/gallery tests and 7 packaging tests. All 68 maintained PNGs decode and validate; fresh Catppuccin Latte and Mocha main, Settings, and Typst-overrides viewport PNGs were inspected, with traced native preview bounds remaining clipped to the panel.
#link("www.google.com")

= correctness audit (2026-09-08 fonts, popups, and greenfield cleanup)

- Items 20, 72, 74, 76: Code ligatures are enabled. Settings has separate system and workspace font dropdowns and independently persisted UI and code weights. Continuous weight changes are staged until pointer release, and both slider rows share one vertical-centering contract.
- Items 55, 56: Right-clicking a text font argument opens a scrollable document-font selector. Workspace fonts are discovered asynchronously and supplied to Typst and Tinymist. File fingerprints catch additions and in-place replacements while preserving the prior catalog during a rescan.
- Items 63, 73, 79: Preview status and visible notices share a timestamped newest-first log capped at 100 entries. The popup grows only for visible rows, hugs the status bar, and uses a fixed right-aligned timestamp column.
- Items 69, 78: Popup state now has one owner. Settings and app menus close competing transients, blur requires a grace period, each menu opening has independent scroll memory, and popups paint immediately at full opacity.
- Items 75, 77: Settings palette swatches are gone. Typst override headers, syntax labels, samples, and Reset buttons share exact row centers. Deterministic Settings and override scenes reset their scroll origins.
- Item 80: Removed the obsolete app bundle, registration, build products, scratch previews, deprecated names and environment variables, old settings migrations, deprecated LSP root path, Finder process-serial handling, and disconnected editor receiver plumbing. The current settings schema is a clean break.
- Item 26 remains deferred.
- Items 68, 72 through 80: Regenerated and decoded all 68 gallery PNGs and inspected fresh Catppuccin Latte and Mocha Settings, theme-picker, menu, tooltip, status, font-selector, and Typst-override captures. The native inspector did not expose the current bundle window, so viewport captures are not claimed as proof of composed desktop geometry.
- Verification for items 68, 72 through 80: Formatting, strict Clippy, 318 application tests (316 passed and 2 environment-dependent ignored), 26 integration tests, and 7 packaging tests pass. Fake-Tinymist test deadlines now tolerate full-suite scheduler load without changing runtime timeouts. Diff checks pass and no obsolete pre-rename namespace, deprecated root-path field, or removed alias remains in tracked implementation.

= correctness audit (2026-09-08 pinned preview and risk-based UI evidence)

- Items 61, 70: The designated Typst entry is passed to both Tinymist compilation and `startDefaultPreview`. Switching among project Typst, text, PDF, and image files preserves the same native WebView and only updates the active LSP document. A replacement preview server now forces one navigation even when the operating system reuses its predecessor's localhost URL.
- Items 61, 70: Interactive preview suppresses duplicate raster compilation during steady-state use. Raster pages remain an explicit fallback for unavailable native preview, export, and targeted framebuffer capture.
- Items 68, 81: `AGENTS.md` and its regression test now require screenshots only for materially visual risks. The maintained gallery remains a serialized one-app-session workflow, but no fresh capture was performed in this audit at the user's request.
- Verification for items 61, 68, 70, and 81: Formatting, strict Clippy, the full Rust suite, all decodable gallery/manifest checks, and 7 packaging tests pass. Items 61, 70, and 81 remain unchecked until their explicitly outstanding manual exercises are completed.

= correctness audit (2026-09-08 vector icon redraw)

- Item 82: Replaced the overlapping refresh chevron with a continuous arc-and-arrow path whose outer wing remains outside the circular body. Replaced the straight-sided preview eye with symmetric cubic Bezier curves and a centered round pupil.
- Verification for item 82: Deterministic geometry tests cover arrow/body separation and curved-eye symmetry. Formatting, strict Clippy, and the full Rust suite pass. No app launch or screenshot was used; item 82 remains unchecked for user visual verification.

= correctness audit (2026-09-08 external link interactions)

- Item 83: Tooltip Markdown links now omit their destination syntax, retain selectable text, and dispatch normalized HTTP(S) targets through the shared system-browser route. The editor recognizes literal external targets in real `#link(...)` calls: normal clicks remain editing actions, Cmd+click opens the link, and right-click exposes an Open Link in Browser action. Raster and native preview links use the same route.
- Item 83: Native preview navigation callbacks now read the current preview URL, project root, and designated-source directory from shared session state. Reusing a pinned WebView after Tinymist restarts therefore cannot mistake the replacement localhost origin for an external page or resolve project links relative to a stale editor file.
- Verification for item 83: Parser, Unicode cursor mapping, unsafe-scheme rejection, command-click priority, semantic tooltip activation, semantic context-menu activation, popup sizing, project-file routing, external preview dispatch, and replacement-origin routing have deterministic regressions. No app launch or screenshot was used; item 83 remains unchecked for user interaction verification.

= correctness audit (2026-09-08 inert chrome and explorer copy actions)

- Item 84: File-tree names, every `ttt` logo instance, and the main title-bar filename now explicitly opt out of egui label selection while document-like labels remain selectable. File context menus expose separate commands for the file name, absolute file path, and path relative to the workspace root; directory menus retain their existing Copy Path command.
- Item 84: The native View menu now advertises the same Cmd+1 through Cmd+5 mapping already handled by the editor and shown in the title-bar View popup: Explorer, Code, Split, Preview, and Problems.
- Verification for item 84: Selection behavior, clipboard text derivation, context-menu routing and sizing, and native shortcut metadata have deterministic regressions. No app launch or screenshot was used; item 84 remains unchecked for user interaction verification.

= correctness audit (2026-09-08 stable explorer activation)

- Item 49: Opening another document inside the current workspace now preserves the cached filesystem snapshot instead of clearing it or scheduling an unnecessary scan. The stable snapshot generation keeps the tree identity, expansion state, and scroll memory intact; a real workspace-root change still installs a new snapshot.
- Item 50: The active Explorer entry retains the shared active-row background and now uses a separately registered heavier face of the selected UI font. This makes the current file genuinely bold instead of relying on egui's stronger-color-only `strong` flag.
- Verification for items 49 and 50: Workspace invalidation and the distinct strong UI font role are covered programmatically. No app launch or screenshot was used; both items remain unchecked for user interaction verification.

= correctness audit (2026-09-08 compilation controls)

- Item 85: The toolbar now has an explicit Pause/Resume control. Pausing cancels queued automatic work, stops the persistent CLI watcher, suppresses editor-driven Tinymist updates, and leaves one-shot PDF exports and deterministic captures available. Resuming sends the newest editor buffer and restarts whichever preview pipeline is required.
- Item 85: Compile and Cmd+R now materialize the canonical PDF beside the effective saved Typst entry, including a designated preview entry while another file is being edited. Unsaved Typst documents ask for their first output location; Export PDF remains the choose-another-destination command. Compile and Export retain distinct chooser, queued, completion, and cancellation messages, and a second output request can no longer overwrite an already queued destination.
- Item 85: Completing a one-shot Compile or Export while automatic compilation is paused now reaps the temporary watcher again instead of leaving it active after producing the PDF.
- Verification for item 85: Deterministic tests cover pause gating, explicit-build exceptions, watcher shutdown, toggle copy, current/designated output paths, output intent, and unsaved/non-Typst rejection. Formatting, strict Clippy, all 372 non-ignored repository tests, all 7 packaging tests, and the ignored real-`typst` watcher compile/error/recovery integration test pass. No app launch or screenshot was used; item 85 remains unchecked for user interaction verification.

= completed non-deferred backlog and startup correction (2026-09-09)

- Items 18, 21, 23: Added an asynchronous searchable package inspector for available and every locally installed release/path, recursive search across all Explorer sections and every individual Settings control, and bounded image/PDF previews for editor literals and Explorer files.
- Items 22, 28, 36, 38: Added one configurable shortcut model for all application actions and every owned text-edit viewport. Effective bindings now drive menus and tooltips; semantic clipboard events respect rebinding, delayed native paste is admitted once within a bounded window, invalid plain-letter bindings cannot steal typing, and reassignment removes hidden collisions.
- Items 33, 41 through 44, 49, 50, 52, 54, 65: Made application chrome inert while preserving selection in document content, gave the active file a readable current-line background and true heavier font, unified native/title-bar menus, added draggable Explorer sections and a one-frame content hide, preserved tree/scroll identity, added clickable line-numbered sticky context, left-aligned fallback menus, and made real Problems-row double-clicks navigate.
- Items 40 and 57: Integrated versioned Tinymist completions with snippets, additional edits, stale-response rejection, keyboard navigation, and atomic undo; added conservative syntax-tree table detection plus a modal row/column/cell editor whose changes apply as one Unicode-safe replacement.
- Item 59: Enabled crate-wide unsafe-code denial and retained only four narrowly scoped native adapters, each with an adjacent safety contract and an architecture inventory.
- Items 60, 61, 70, 71: Manual Save and Save As format the exact destination Typst document while auto-save remains unformatted; a designated preview entry survives unrelated document switches and refreshes without recreating its WebView; standard macOS window shortcuts work, and native Quit routes through the process-wide dirty-document guard.
- Items 66, 67: Added canonical `ttt` vector/raster/ICO artwork, embedded the runtime icon before native window creation, and included the icon contract in packaged builds so direct and packaged launches retain it through shutdown.
- Items 81 through 84: Captured the complete 68-image maintained gallery in one app session; verified the redrawn refresh/preview icons, external Markdown/Typst/preview link routing, inert tree/logo/title labels, three file-copy actions, and Cmd+1 through Cmd+5 native View-menu metadata.
- Item 85 correction: Pause/Resume controls automatic preview refresh only. Paused documents continue sending Tinymist LSP changes so completion, diagnostics, and formatting remain live. Compile/Export waits for a strictly newer artifact after pending edits and writes the effective saved Typst entry beside its source, using a picker only where required.
- Item 86: Removed the startup deadlock by deriving viewport-scoped egui IDs before entering context data/memory locks. Compiler, rasterizer, and Tinymist pipe readers now also have bounded shutdown after their direct child exits, so a wrapper descendant retaining stdout/stderr cannot wedge stop, restart, or application exit.
- Verification: Formatting, strict Clippy, and diff checks pass. The repository suite passes 540 tests with 2 explicitly environment-dependent tests ignored; all 8 packaging tests pass. All 68 maintained PNGs were freshly generated and decoded, representative light/dark main, Settings, menu, tooltip, and icon captures were inspected, and a final traced native launch painted, captured, reported in-bounds preview geometry, and exited normally.
At that point, only deferred items 1, 2, and 26 remained open.

= correctness audit (2026-09-12 easy backlog)

- Items 88, 89, 90, 91, 92, 96, 101, 102, 103, 104, 105, and 107: Added the
  Finder/file-manager action to the Explorer menu, reused a live Tinymist
  session for in-place external reloads, recorded successful browser launches
  as green status notices, normalized malformed variable-font axes, placed
  asset previews to the right of Explorer rows, disabled non-Typst view-mode
  controls, added Packages and Rename to title-bar menus, exposed package
  website links, surfaced installed packages before the registry fetch,
  inserted line comments at column zero, and painted Find/Replace after the
  sticky context overlay.
- Verification: Formatting, strict Clippy, 517 application tests (2 ignored),
  26 integration tests, and 8 packaging tests pass. New regressions cover
  variable-axis normalization, popup actions, package-local loading, external
  link notices, comment placement, view-mode availability, and sticky/find
  geometry.


= correctness audit (2026-09-12 remaining active backlog)

- Items 87 and 94: Explorer drops copy regular files into the hovered folder
  without overwriting existing entries; dropping onto the editor opens files.
  Explorer deletion requires confirmation and handles the current document and
  designated preview entry. Filesystem regressions cover collisions, source
  preservation, directory boundaries, and symlinks; UI tests cover drop routing.
- Item 93: Cmd-Shift-Space toggles the tooltip under the mouse; Cmd-Shift-K
  opens it at the caret. Both shortcuts are configurable. Focused cards support
  scrolling and Escape dismissal, with dismissal retained until rearmed or the
  pointer leaves the source.
- Item 95: View > Git opens workspace-scoped status, diffs, staging, unstaging,
  commits, recent history, fetch, fast-forward pull, and push. Operations use
  files on disk and configured remotes and credentials. Tests exercise temporary
  repositories and a local remote, including rejecting divergent pulls without
  discarding local changes.
- Items 97, 98, and 106: Completions fuzzy-filter as the prefix changes, with
  Unicode-safe rebasing and filtering before the result limit. Font completion
  remains available after a space or opening quote. Reference suggestions show
  insertion code before descriptive metadata and filter on that code. Member
  completions filter the member name independently of its receiver.
- Item 99: Completion selections and Settings font-picker hover cards render
  a sample using the actual font file. Loading happens in the background and
  replaces a bounded picker slot. Font pickers also support fuzzy search.
- Item 100: Each installed package version has an Uninstall action, a directory
  confirmation, release-path validation, and an immediate installed-list refresh.
  Tests verify that sibling versions and linked directories remain untouched.
- Item 108: Ready image/PDF hover cards contain only the image. Loading and
  failure states retain their explanatory messages.
- Item 109: Mandatory command descriptors generate both native and title-bar
  File/Edit/View menus. Semantic tests exercise every shared command. Menu
  geometry tests cover all rows, separators, and changing from a shorter menu
  to a taller one without retaining the previous scroll clipping.
- Verification: Formatting, strict Clippy, and diff checks pass. The repository
  suite passes 583 tests, with 2 environment-dependent tests ignored; all 8
  packaging tests pass. The complete 68-image gallery was regenerated in one
  app session and validated. Fresh targeted font, asset, and menu captures under
  `.tiptoptyp/screenshots/agent-review` were inspected in Latte and Mocha.
  The final editor-context capture after font-preview scenes reproduced missing
  glyphs in one disabled label; the stable gallery's corresponding label renders
  correctly. This remains tracked by deferred item 26, not treated as resolved.

Only deferred items 1, 2, and 26 remain unchecked.


= correctness audit (2026-09-12 font-preview flashing)

- Items 99 and 110: The previous preview implementation called egui's
  `set_fonts` when the highlighted family changed, invalidating the shared
  font atlas and cached text layouts across the application. Samples now use
  an isolated, worker-owned font atlas uploaded as a separate texture, with
  four recently used samples cached per picker. Browsing fonts never changes
  the application's font definitions. Font files are released after rasterizing.
- Items 26 and 110: Font-preview flashing was a regression in the new preview
  implementation. The earlier observation should not have been attributed to
  deferred item 26 without tracing it. This specific trigger is fixed; item 26
  remains open for any unrelated intermittent text corruption.
- Verification: A regression repeatedly switches samples while preserving
  custom UI/code text-layout identities and asserting that no shared-font
  texture updates occur. Separate geometry tests cover different actual fonts,
  1x/2x resolution, and normalized texture coordinates. Formatting, strict Clippy,
  all 584 repository tests (2 environment-dependent tests ignored), and all 8
  packaging tests pass. Five fresh native viewport captures were inspected:
  font completion and Settings samples in Latte/Mocha, followed by the editor
  context menu. The previously incomplete menu label renders fully.


= correctness audit (2026-09-13 Git window)

- Item 111: Git now has section headings, file counts, consistent right-aligned
  action columns, truncated paths with full-path hints, and a dedicated,
  selectable diff viewer with colored additions and deletions. Comparisons
  explain staged versus unstaged changes, including loading, empty, and error
  states. Finished comparisons scroll into view even in a short window.
- Git jobs are collected in the Git child viewport as well as the editor.
  Previously the child depended on the editor repainting to collect results,
  and an empty diff appeared as a generic operation-success message.
- Unstage all removes index changes while preserving working files, including
  files edited again after staging in a repository without its first commit.
  Stage all excludes app-owned .tiptoptyp artifacts and handles literal paths
  through a NUL-delimited pathspec file. Already staged artifacts stay visible
  for unstaging; no existing repository index was changed during this work.
- Verification: All 11 focused Git tests pass, including real temporary Git
  repositories, child-only async completion, narrow/wide action alignment,
  short-window diff reveal, empty/error states, and file preservation. Formatting,
  strict Clippy, all 591 repository tests (2 environment-dependent tests ignored),
  and all 8 packaging tests pass. One sidecar-shutdown test failed on the initial
  full run, passed in isolation, and passed in the final full run. Fresh Git
  viewport captures were inspected in Latte and Mocha; the 68-image gallery
  was regenerated in one app session and validated.


= correctness audit (2026-09-13 Git editor integration)

- Item 112: Explorer files show Git status badges with staging details on hover.
  Added, modified, and deleted line runs appear beside the line numbers; context
  lines are not highlighted. Clicking a marker opens its selected chunk in a
  popup owned by that document window. Comparisons include staged and unsaved
  edits against the last commit, without changing the source file or Git index.
- Each document window owns its scans and selected chunk. Results carry the
  workspace, path, and buffer revision, so late results cannot replace another
  document's hunks. Background jobs repaint their originating viewport. Tests
  cover two different buffers of the same file alongside another repository,
  wrapped lines, clipping, deletion boundaries, and actual editor gutter clicks
  in both populated and empty documents.
- Regression correction: changing the document revision now closes the selected
  chunk before a new Git scan, so a popup cannot display stale content after a
  file switch or edit. View > Git also reveals a previously closed Explorer.
- The document status row now reports Git line totals as `+ added`,
  `~ modified`, and `− deleted` from the current buffer diff. Deleted runs
  retain their old-line count even though their editor range is an empty
  boundary, and the summary has a descriptive hover tooltip.
- Item 26: The combined Git/theme capture reproduced missing glyphs. GPU
  readback confirmed pixels missing from the uploaded font atlas. Immediate
  viewport rendering now resynchronizes the current atlas after font-cache
  changes and repeated parent layout passes. Ordinary frames keep incremental
  uploads, and existing text layouts are preserved. Three regressions cover
  stale parent uploads, font-cache replacement, and steady-state uploads.
  This reproducible transition is fixed; the older deferred report remains open
  for any unrelated cause. No renderer dependency fork or diagnostic code remains.
- Verification: Formatting and strict Clippy pass. All 607 repository tests
  pass, with 2 environment-dependent tests ignored; all 8 packaging tests pass.
  An initial gallery-manifest check overlapped gallery regeneration; the complete
  suite passed after capture finished. Six fresh release viewport captures of
  the Git page, editor, and selected chunk were inspected in Latte and Mocha,
  including the formerly failing transition. The complete 68-image gallery was
  regenerated in one app session, validated, and representative fresh images
  inspected. Existing repository index contents were preserved.


= correctness audit (2026-09-13 code review)

- Item 113: Native asynchronous file dialogs remember their originating
  viewport, so completing a dialog after another window becomes current wakes
  the correct window. Imported files preserve their source permissions,
  including executable scripts, while retaining collision protection.
- Both Git diff consumers share deterministic unified-output arguments.
  Custom diff markers and suppressed blank context lines no longer shift
  editor change markers. Intent-to-add files now receive an Added badge and
  use an empty baseline when absent from the last commit.
- Background Git decorations obtain branch and file status in one status
  command without fetching history or taking optional index-refresh locks.
  The Git window also reuses its initial snapshot for a plain refresh instead
  of immediately repeating the same scan.
- Switching files or workspaces detaches obsolete read-only scans, allowing
  the new file to scan immediately. Typing in the same file keeps one worker
  and debounces the next request. Detached workers can finish their bounded
  operation, but cannot publish a result or error into the new document.
- Verification: Six new regression tests cover the fixes, including actual
  temporary Git repositories, permission preservation, viewport wake routing,
  and blocked-worker transitions. Formatting and strict Clippy pass; all 613
  repository tests and all 8 packaging tests pass, with 2 environment-dependent
  repository tests ignored. These changes have deterministic behavioral
  coverage; no new visual verification is claimed for this audit.


= refactoring implementation (2026-09-13)

- Items 114–118 record the next product priorities: crash recovery, Git chunk
  actions, workspace replacement, shared status/performance work, and PDF polish.
- Item 119: Settings, editor rendering, and native child views now have focused
  modules under `src/app/`; tests have their own module. Window orchestration
  stays in `src/windowing.rs`. `app.rs` is 14,792 lines, down from roughly 22,800
  including tests. Runtime helpers remain candidates for future extraction.
- Item 120: Implemented the R1–R8 boundaries in `refactor.typ`, including a
  headless core crate, sealed edit/snapshot/save APIs, exclusive workflow phases,
  coordinated multiwindow close, bounded compiler/asset queues, retained mutation
  completions, an exit barrier for active background operations, preview
  provenance, typed coordinates, context-owned palettes,
  scoped workspace targets, and explicit write durability outcomes.
- The new Unicode tests found and fixed an LSP edit panic at the empty final
  line after a newline. Worker disconnection is now terminal rather than an
  indefinite loading state; PDF link extraction has a bounded, reaped process.
- Native verification reproduced a light/dark-to-new-child rendering abort.
  The viewport adapter now waits for the appearance-change frame before creating
  a new child, while retaining existing children. Capture batches also own and
  restore their original fixture independently of scene-specific document edits.
- Verification: Formatting, strict Clippy, all 643 workspace tests, and all 8
  xtask tests pass; 2 environment-dependent workspace tests remain ignored.
  The complete 68-image gallery was regenerated in one app session and validated.
  Four fresh font, settings, save-modal, and restored-main viewports were inspected,
  along with representative gallery images. Native bounds traces are retained
  under `.tiptoptyp/screenshots/refactor-review/native-geometry.log`; composed
  native child-window placement was not visually verified. Both Typst documents
  compile to private review outputs without overwriting existing PDFs.

= correctness audit (2026-09-13 Git and document-state regressions)

- Item 130: View > Git now opens the Explorer panel when it was closed, and
  explicitly reveals the Git section once even if a persisted collapsing state
  had left it hidden.
- Item 112: Selected Git chunks are discarded whenever the owning document
  revision or path changes. A stale chunk popup therefore cannot remain visible
  after editing or switching files, while each window keeps its own selection.
- The Git Explorer section is visible by default in normal document windows;
  View > Git still toggles it when more file-tree space is needed. Its body
  starts with repository actions; the workspace path remains in the parent
  window header rather than being repeated inside the panel.
- Workspace/file refreshes queue a Git status scan automatically, including
  updates after saves, external reloads, and the periodic filesystem check.
- Git editor decorations are refreshed by document, filesystem, and Git
  operation events; an unchanged editor does not schedule periodic scans or
  repaint the interface just to rediscover the same snapshot.
- Verification: Formatting, strict Clippy, all 585 application tests (2
  environment-dependent tests ignored), and all 8 xtask tests pass. A release
  capture attempt for `git-window` could not start a native viewport in this
  environment because macOS LaunchServices/HIServices reported an invalid XPC
  connection and then timed out; no visual pass is claimed from that attempt.
