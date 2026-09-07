#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")
#show "[ ]": box(stroke: 1pt, height: 0.8em, width: 0.8em)
#show "[x]": box(stroke: 1pt, height: 0.8em, width: 0.8em,fill: green,align(center)[x])
= running todo list

Keep this list as the source of truth. Every task has a permanent number and
`[ ]` (open, partial, unverified, or deferred) or `[x]` (completed). Never remove
completed tasks or renumber existing tasks; append new tasks with the next
unused number. Resolved and audit notes must cite the relevant item numbers.
Update the checkbox only when the entire task is complete and verified.

1. [ ] (Deferred) there should be a proper pdf preview option that is just a proper pdf viewer.
2. [ ] (Deferred) perf

3. [x] typst overrides should be in appearances subsection
4. [x] the color indicator in typst overrides should always be the current color, not faded out if its 'theme/override'. In fact there should not be a 'theme/override' button, since if we want to return to the theme we can just hit the reset button
5. [x] somethings not right with alignment, see pic
6. [x] lots of tooltips in typst overrides popup kinda suck. Too big, not useful. If useless just delete it, otherwise tighten the space
7. [x] next to the theme name in the selectors, we should be able to see at a glance a colour pallette of the theme.
8. [x] allow customising UI fonts
9. [x] cmd +/- should adjust UI size. Also put a slider for this in settings
10. [x] syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ.

11. [ ] if we are using a variable width font, add support to set the weight (typst overrides, and UI)
12. [x] change so that: allow triangular area from cursor to tooltip / error window to not dismiss. When mouse is over the popup, we can scroll.
13. [x] also add a keyboard shortcut to make the popup appear with keyboard focus (for scrolling up/down). Esc to unfocus and dismiss. There should be a subtle outline to indicate focus (as well as mouseover)
14. [x] I noticed that when I use a ```typ ...``` code block, it is not syntax highlighted correctly. Fix
15. [x] syntax highlight the markdown in the tooltips. If typc doesnt give colour coding then treat it as typ. (Duplicate of item 10; keep both statuses in sync.)
16. [ ] Mitex and cmarker integration: autodetect strings/raw strings inside `#mi(...)` etc to syntax highlight with tex or markdown. Note that you may need to put the text in math mode or text mode depending on the mitex command
17. [ ] for other colour formats (luma etc) we should give that text the appropriate fill, like we do now for ` #rgb(“hex number”)`
18. [ ] package manager - inspect available packages, locally installed packages
19. [x] detect when file changed outside app. If we dont have any changes queued to be saved, then reload the file
20. [ ] allow font ligatures in code editor
21. [ ] image / pdf preview on hover (both in code, when we include images as part of the code, and in file explorer)
22. [ ] configurable shortcuts everywhere. add a popup for this in settings
23. [ ] add searcg to all everything - explorer windows, settings,
24. [x] view > … needs to be in the title bar as well, and also none of the shortcuts are appearing - add
25. [x] allow us to toggle off the title bar dupes of the menu bar items
26. [ ] once in a while the text everywhere breaks (see pic) some sort of leak?
27. [x] after some time, the find/replace UI is not visible but is clickable. I realised this is because it is rendering behind the code editor panel. we shouldnt even make space for the find / replace UI - it should just be on top of the code panel.
28. [ ] keyboard shortcuts when find and replace is active should work on find and replace text fields. and similarly for other text fields (Partial: find-field focus is preserved; verify shortcuts across all other text fields.)
29. [x] cmd+F / the Find button should toggle the find/replace popup, not just turn it on.
30. [x] there should be a button to toggle find and find/replace
31. [x] there should be options to allow case (in)sensitivity and regex (which we should be able to right click and get a cheat sheet). add single-symbol wide buttons for these
32. [x] if we right click and its already ‘use for preview’d then we should be able to toggle it off
33. [ ] text highlight of file names is not readable, adjust color scheme
34. [x] should be able to right click file in explorer -> open in new window
35. [x] cmd-/ to toggle comment out current or selected line(s)
36. [ ] shortcuts should be configurable (Duplicate of item 22; keep both statuses in sync.)
37. [x] move the problems button to the rightmost slot (matching with default shortcuts)
38. [ ] remove ‘cmd/ctrl’ everywhere in popups/tooltips its just cmd
39. [x] fallback for CJK fonts
40. [ ] autocomplete
41. [ ] text is selectable in too many places
42. [ ] file menu in menu bar does not match file in title bar. It should be single source of truth
43. [ ] file explorer subsection separators should be draggable
44. [ ] when closing the file explorer it should first stop showing all the contents before minimising
45. [x] there should be another subsection in the explorer, the tags/references
46. [x] color code various filetypes (various categories should be: .typ, other text-based files, pdfs and images, folders, and others) make sure you use colours from the theme
47. [x] the ttt in settings window is no longer left of file name in main window(s)
48. [x] the top right X in settings should not be there - use the Mac OS traffic lights
49. [ ] selecting a file in file explorer resets the file explorer scroll amount. Why? Opening a file cannot trigger more files to appear. It should not redraw. If it does, at least restore the scroll
50. [ ] the current line background should be also used for the current file.
51. [x] find and replace shortcut just gives find.
52. [ ] we should (by default) have the current scopes and the current sections/subsections etc be "sticky rows" at the top so that we have context for what we are looking at. each level should give a further row that is persisted to the top. For instance if we are in ```typ
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
54. [ ] right click menu if possible should use native right click. If not, at least the text should not be centered, it should be left-aligned like normal
55. [ ] special detection of typst functions: set text(font: ...) should allow us to right click on font: … and then have a scrollable selector
56. [ ] allow using fonts in the root of working directory or anywhere in the working directory if it is not a performance hit.
57. [ ] Table maker / editor on right click (with detection of tables)
58. [x] the tooltips on hover should be wider. Maybe more like 80 char wide
59. [ ] remove unsafe rust as much as possible
60. [ ] it should format on manual save (cmd+s) (but not auto save) (Partial: existing-file manual saves format when Tinymist is ready; initial Save As remains open.)
61. [ ] if we have a file set to be used for preview, then when we switch to a different file, we dont need to restart the preview window. Also, we should not close it even if we open a non-typ file. thats the point of the used for preview setting - so that we can edit other child files like a .toml or other .typ files (Implemented in part; end-to-end preview lifecycle verification remains open.)
62. [x] Any part of the UI with this tiny font size like that used in the file path of the file explorer should have a bigger font. Never use this small font. Also don’t use allcaps or smallcaps, not our style.
63. [ ] in addition to the pdf ready indicator i also want all the greentext on the side e.g. applied theme saved automatically. and everything should be timestamped. (Partial: timestamped history exists; coverage of all notices remains open.)
64. [x] we should reserve some space for the `*` in the name when a file is not yet saved, sot hat when it does appear, it doesn't shift the UI elements.
65. [ ] double clicking on a line in the Problems panel should jump to that line in the code panel
66. [ ] the logo icon should be redone to match the settings 'ttt'
67. [ ] on closing the app ran with `cargo compile --release` it should not lose the icon as it disappears?
68. [x] Maintain UI observability and regression tests with mandatory fresh screenshot inspection. Capture detailed UI scenes in two representative themes (Catppuccin Latte and Mocha), and a generic main-window shot for each remaining theme.
69. [ ] code panel tooltips have disappeared. very odd?
70. [ ] if i am editing test.typ and i set todo.typ to be used for preview, then it should refresh the preview window to use todo.typ. this is not the current behavior, and if i then switch to opening the todo.typ file, the preview will still be stuck at test.typ
71. [ ] cmd+M should minimise the window, and so on for the usual mac os shortcuts
= resolved in the 2026-09-07 pass

Items 1 and 2 remain deferred. This history records completed work and partial
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

Open subsystem work: items 11, 16, 17, 18, 20, 21, 22, 23, 26, 36, 40,
41, 43, 44, 52, 55, 56, 57, 59, 66, and 67. Other incomplete or unverified
requests also remain unchecked in the running list.

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

Outstanding follow-up includes item 42 (menu parity), item 63 (all-notice
status history), item 60 (Save As
formatting), and 61 (end-to-end preview lifecycle verification), as well as
the other unchecked tasks above.

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
