#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")
#show "[ ]": box(stroke: 1pt, height: 0.8em, width: 0.8em)
= Outstanding work

Only unresolved tasks are listed here, in ascending order. Numbers are permanent;
append new tasks starting at 339. The historical checklist and detailed work diary
are preserved in #link("archive/todo-2026-09-23.typ")[the archived todo file].
See #link("docs/backlog-300-worklog.md")[the completion summary] for the latest work.

26. [ ] (Deferred) once in a while the text everywhere breaks (see pic) some sort of leak?

114. [ ] crash recovery: restore unsaved buffers and all document windows after an unexpected exit

116. [ ] workspace search and replace: preview replacements across files, support regex capture groups, and provide undo

117. [ ] large-project performance: share repository status between windows and measure indexing, preview, and typing latency (ongoing optimization, using the profiling foundation in item 2)

163. [ ] Allow for the pdf preview to pop out into a different window. if I close it, the preview should go back to the window

182. [ ] pretty animation for dragging tabs

223. [ ] prove out typst-compatible binaries like calepin

234. [ ] Isolate the transient oddly shaped large-window flash reported when
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

1. [ ] sometimes tinymist can redirect you to a source file outside the current workspace root (when it sends you to a package file). When this happens, the workspace root switches to the workspace of that file, and that file is opened in a new tab. I agree that it should open in a new tab but not that the workspace root changes. if we nav back to the old tab the workspace root changes back. It should instead, at the top of the code editor, have a small banner that says "You are currently editing a file outside the current workspace root. The workspace root is now [workspace root path]." and have a button that says "Switch to this workspace root" which when clicked, switches the workspace root to the new one. This way, the user is aware of the change and can choose to switch or not.
2. [ ] the stage all and commit button / commit all staged button in the git subpanel should be text
3. [ ] we need to implement comfy mode - if on, the bg and text of a pdf / tinymist preview should be adjusted to match the theme settings.
4. [ ] improve harper integration
5. [ ] autodetect installed tex distributions
6. [ ] implement synctex integration
