#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")
#show "[ ]": box(stroke: 1pt, height: 0.8em, width: 0.8em)
= Outstanding work

Only unresolved tasks are listed here, in ascending order. Numbers are permanent;
append new tasks starting at 339. The historical checklist and detailed work diary
are preserved in #link("archive/todo-2026-09-23.typ")[the archived todo file].
See #link("docs/backlog-300-worklog.md")[the completion summary] for the latest work.

26. [X] (Deferred) once in a while the text everywhere breaks (see pic) some sort of leak?

114. [ ] crash recovery: restore unsaved buffers and all document windows after an unexpected exit

116. [ ] workspace search and replace: preview replacements across files, support regex capture groups, and provide undo

117. [ ] large-project performance: share repository status between windows and measure indexing, preview, and typing latency (ongoing optimization, using the profiling foundation in item 2)

163. [x] Allow for the pdf preview to pop out into a different window. if I close it, the preview should go back to the window. I think we can essentially reuse the main window code for the extra window, just dont let it open a bottom panel, or explorer panel, or code panel. and the two windows should be linked in their actions - if in the main window we toggle the preview back on whether in split or just preview mode, then this extra preview window should be closed. and, if we close the extra preview window, it should open back in the main window as either preview or split mode.

182. [ ] pretty animation for dragging tabs

223. [ ] prove out typst-compatible binaries like calepin

234. [X] Isolate the transient oddly shaped large-window flash reported when
opening Settings with multiple main windows. Duplicate Settings ownership is
fixed in 231, but steady-state native observations/framebuffers cannot certify
that a short-lived flash is gone. Keep this separate from singleton acceptance.

237. [ ] Consolidate Problems, index and preview navigation/focus handoff after
reproducing the native failure. Test document/range routing and secondary owners;
do not equate helper tests with verified native word-navigation behavior.
Implementation: `src/app/navigation.rs` owns destination conversion, diagnostic
targeting, current-file/deferred routing and source-range handoff. File links and
Explorer entries share routing; Problems retain already-mapped editor coordinates,
while canonical file/LSP positions still use the miTeX mapping. Existing document
workflow epoch checks and native-parent → viewport → TextEdit focus order remain.
Reproduced a concrete focus failure before fixing it: with Find open, a source
jump moved the caret but left Find focused. The existing pending selection now
distinguishes Search (retain focus) from Focus (source/editor navigation), rather
than introducing a separate focus flag. Find stays open after navigation.
Regression coverage exercises all three entry points in root and secondary
viewports, Option/Cmd Left/Right, owner-only one-shot focus and Find focus retention.
This is deterministic UI coverage, not an observed macOS WebView first-responder
handoff; leave the item open for that native acceptance check.
Accounting versus the preceding leaf-extraction commit: app.rs 12,840 → 12,647
(−193); navigation module 226 lines; other production adapters +3, for +36 total
non-test lines. Tests +181. The added selection intent fixes a demonstrated
correctness gap; it is not claimed as a net-negative consolidation. No workers,
repaint loops, per-frame source copies or native geometry changes are introduced;
the extra branch runs only for a pending selection. No timing speedup is claimed.
Verification: failing-before/passing-after Find-open focus regression, all 13
navigation-filter tests, Find focus-retention test, 1,061 full-suite test executions
(16 opt-in tests ignored), 13 xtask tests, formatting and strict all-target Clippy
pass. Native WebView delivery remains unverified; subsequent steps are tracked below.

251. [ ] (Deferred after 246's review; rationale in the work diary.)
Consolidate one structured-edit path, starting with formatting result
application. Trace its existing document-key and canonical/display checks;
share an existing validated edit primitive only where semantics match, retaining
format-specific admission. Do not turn completion transactions into a universal
framework. Acceptance: stale/reopened-document and out-of-order replies are
rejected, Unicode and miTeX ranges remain correct, accepted edits have one undo
step, and ordinary Typst incurs no projection or extra source copy. Delete the
replaced mutation path. Depends on 246; expand to other edit kinds only in
separately scoped follow-ups justified by actual duplication.

253. [ ] (Deferred after 246's review; rationale in the work diary.)
Narrow native-preview resource ownership for one child-view kind.
Use 246's writer map to select the existing view handle, applied-property cache
and teardown paths that must change together. Make their lifecycle operations
explicit and delete bypass setters; leave readiness/retry policy in the existing
PreviewController. Acceptance: hide/show, replacement, close and repeated teardown
release once, unchanged properties cause no native setters, and stale owners
cannot reposition a replacement view. Preserve UI-thread ownership and verify
native geometry/composition when affected. Do not bundle every preview flag into
a second controller. Depends on 246 and relevant native gates 234/237.

264. [ ] Tinymist preview: smooth zoom on large real documents. Continuous
  pointer-anchored Ctrl-scroll is implemented, but full-SVG layout remains slow.
  Native preview shortcuts now dispatch to the viewer and mixed inputs accumulate
  in order. Evaluate a cached viewport bitmap for gesture transitions: capture
  after rendering settles, not on the first gesture event; bound pixel memory,
  preserve the cursor anchor, and swap only when sharp content is ready.

265. [ ] Tinymist preview: smooth divider resizing on large real documents.
  Redundant rerenders are removed; the full SVG still costs about 200 ms to
  rescale in WKWebView. Validate bounded visible-region rendering and scroll
  refill before closing these items; the small fixture was insufficient.

266. [ ] vim mode

268. [ ] while adjusting the zoom, we show the user a blank page while it loads. it should always show us the lower res page until it loads the higher res page

283. [ ] connect to TPIX https://typstify.com/tpix

289. [ ] Document and expose a deliberate partial-rendering buffer policy for the
  Tinymist preview. The pinned frontend currently offers only the boolean
  `--partial-rendering` switch: its SVG window is derived from the preview
  scroll position and viewport, expanded to page boundaries, and is not tied
  to the editor cursor. Decide whether a bounded page/viewport prefetch is
  worth implementing upstream or in the adapter, then measure scroll refill
  latency and memory before changing the production default.

290. [ ] (deferred) Add a bounded macOS bitmap transition for the live Tinymist viewport.
  Keep one in-memory WKWebView snapshot, show it only during an active
  zoom/divider transition, and replace it after the live preview settles.
  Capture no image every frame, invalidate stale callbacks when the webview
  or document changes, preserve pointer anchoring, and verify that the
  overlay cannot steal editor/preview input. Add deterministic frame/state
  tests and a native geometry trace before enabling it by default.

295. [x] Removed the legacy raster PDF viewer and PDF.js, including their settings,
  adapters, assets, render workers and capture surrogate. PDFium owns PDF viewing
  and framebuffer captures; image viewing and hover thumbnails remain separate.

= NEW TODOS

1. [x] sometimes tinymist can redirect you to a source file outside the current workspace root (when it sends you to a package file). When this happens, the workspace root switches to the workspace of that file, and that file is opened in a new tab. I agree that it should open in a new tab but not that the workspace root changes. if we nav back to the old tab the workspace root changes back. It should instead, at the top of the code editor, have a small banner that says "You are currently editing a file outside the current workspace root. The workspace root is now [workspace root path]." and have a button that says "Switch to this workspace root" which when clicked, switches the workspace root to the new one. This way, the user is aware of the change and can choose to switch or not.
2. [x] the stage all and commit button / commit all staged button in the git subpanel should be text
3. [x] we need to implement comfy mode - if on, the bg and text of a pdf / tinymist preview should be adjusted to match the theme settings.
4. [x] improve harper typst/tex integration - it needs to ignore string keys of functions and spellchecking on names somehow
5. [x] allow using a tex dist (pdfLaTeX, XeLaTeX, LuaLaTeX) and autodetect installed tex distributions (mactex)
6. [x] implement synctex for tex docs
7. [x] dragging a file from the explorer to the code panel should put the path at the cursor
8. [x] i noticed that the tinymist preview has a bunch of shortcuts. for instance pressing t inverts the view! we should use this instead of repainting the preview from scratch when we toggle dark mode. Make it work smoothly with comfy mode above
9. [x] the above shortcuts are interfering with the find feature of our litle popup. also I found that cmd+A selects all in the code panel - the find bar should be the one that receives thte cmd+A.
10. [x] the TOC doesn't jump to the sections for tinymist
11. [x] the design for the minimised icon is different for both variants (pdf and tinymist preview). And anyway it uses way too much space. fix and make the button more compact
12. [x] the UI of the interface is completely inconsistent in all states - minimised (see above and below todos), unminimised, and with TOC expanded. The UI layout should be shared code, just the wiring can be different
13. [x] instead of a floating minimized button, in fact it should be a button left of the find button in the top. So remove the floating button. Keep the floating window when expanded. And this button should be greyed out inactive if editor is nto in preview or split mode.
14. [x] the floating window should be more thoughtfully designed. the buttons needs to be correctly grouped while not taking too much space.
15. [x] i modified old todo 163 above - this action should be in our floating window
16. [x] tab -> 2 or 4 or x spaces or tab should be settable in settings (default to 2)
17. [x] i noticed in a cjk keyboard typing 。 gave me a . - we should be able to toggle this behavior

// Implementation notes (2026-09-25):
// NEW TODO 2: text commit actions and interaction coverage landed in e94766d.
// NEW TODO 11: compact controls landed in e94766d; the toolbar placement and
// shared controls requested in 12-14 supersede the floating minimized button.
// Original task descriptions and checkboxes are preserved; progress is append-only.
// NEW TODO 1: outside-root source navigation now preserves the window workspace;
// the editor banner offers an explicit root switch for all tabs.
// NEW TODO 3/8: Comfy preview recolors PDFium and Tinymist locally; appearance
// changes do not edit source or restart/recompile Tinymist.
// NEW TODO 4: Harper skips Typst math and technical named arguments, retaining
// prose strings. Mid-sentence capitalized unknown words are treated as likely
// names; this heuristic can miss capitalized typos or flag sentence-initial names.
// NEW TODO 5: system pdfLaTeX, XeLaTeX and LuaLaTeX are selectable and discovered
// on PATH / the MacTeX link. Standard-engine bibliography orchestration is future work.
// NEW TODO 6: versioned SyncTeX maps support source/PDF navigation and edit following.
// The installed synctex command is required, including when building with Tectonic.
// NEW TODO 7: Explorer file drops insert paths at the caret in one undoable edit.
// NEW TODO 9: Find and preview-search fields receive their own editing commands;
// preview keyboard shortcuts no longer consume typing in text fields.
// NEW TODO 10: Tinymist outline navigation uses compiled heading coordinates.
// NEW TODO 11-14: one shared draggable controls popup serves both viewers. Its
// toolbar toggle sits immediately left of miTeX when present, otherwise Find,
// and is disabled in Code view. The outline popup sizes itself to its content.
// NEW TODO 15 / old 163: Pop out reuses the preview and child-window host; close
// or Split/Preview restores the original document view without another compiler.
// Regression coverage includes semantic UI tests, real Tinymist browser navigation,
// real Tectonic/system-engine builds and SyncTeX round trips. Fresh controls and
// PDF-popout framebuffers were inspected; the 24-image gallery was regenerated.
// Desktop composition inspection remains unverified because the Mac was locked.
// Details and test commands: docs/preview-controls.md and docs/architecture/0007-tex-services.md.

// Follow-up (2026-09-25): checkboxes updated at the user's request; task
// descriptions remain verbatim. NEW 1-17 and old 163 are implemented.
// NEW 16: Settings chooses 1-16 spaces (default 2) or literal Tab; Shift+Tab
// removes the configured indent. Existing source and pasted tabs are preserved.
// NEW 17: optional ASCII punctuation for committed typing; default preserves
// keyboard input. Preedit and pasted text are never normalized. This cannot
// recover punctuation already substituted by the operating system/input method.
// NEW 3/8: real Tinymist browser tests verify rendered paper/text pixels after
// dark, warm-light and standard palette changes, without rerendering. Source
// rule injection is unnecessary in that tested path. Native WKWebView appearance
// and an actual native CJK keyboard session remain unverified.
// Details: docs/editor-input.md and docs/preview-controls.md.
// Verification: 1,285 Rust tests, 15 xtask tests, strict all-target Clippy,
// formatting, 8 JavaScript unit tests and the real Tinymist browser scenarios pass.

// Tinymist appearance regression (2026-09-25): 2ed0f84 replaced built-in
// inversion with a whole-preview filter. Native WKWebView snapshots showed dark
// pixels while its on-screen window stayed white. Reproduced with a Tinymist-only
// window and observed via native app capture, independent of PDFium. Apply the
// transform inside each SVG page and set the separate paper fills directly;
// after the change the actual window is dark. Keep Tinymist inversion disabled
// to avoid double application. Re-send the latest palette after native page load.
// Regression coverage checks filter placement, standard/comfy colors, refreshed
// pages and reload delivery. See docs/preview-controls.md for the native probe.
// Validation: 1,286 Rust tests, 15 xtask tests, strict Clippy, formatting,
// 8 JavaScript unit tests, real Tinymist browser tests, and native palette/reload
// checks pass. The Tinymist-only window was observed white before and dark after
// with the app-capture tool; WKWebView PNGs alone were not used for that claim.
