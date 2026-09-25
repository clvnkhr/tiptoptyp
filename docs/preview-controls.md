# Preview controls and document navigation

The compact controls toggle sits immediately left of miTeX, or left of Find when
miTeX is absent. It is disabled in Code view. Expanded controls float above the
preview; drag the Preview label to move them. Minimize returns to the toolbar
toggle. The outline extends the same popup, sized to its contents up to a scroll
limit. There is no separate HTML control bar in Tinymist.

Both engines use `app/preview_controls.rs`: navigation history, page selection,
zoom, fit width, Find, Outline and Pop out have one presentation. PDFium receives
Rust actions; Tinymist receives the same serialized actions through its adapter.
Tinymist outline destinations come from compiled heading positions, including
nested headings, rather than inferred source lines. Find inputs keep text-editing
commands; child-window commands are queued for that child's next paint.

Pop out moves the preview to a child window with the existing window host. The
owning editor keeps its compiler, artifact and services. Closing the child or
choosing Split/Preview in the document restores the prior presentation. Its
controls are retired with the child, and closing it never closes a source tab.
PDFium retains its navigation state; Tinymist reconnects its view to the same
preview service when moving between native parents.

**Comfy preview** in Appearance maps white and black to the active theme's
background and text colors. Intermediate RGB values are interpolated, so images
and colored text are tinted too. Explicit document inversion swaps the palette.
PDFium recolors retained, bounded visible-page pixels. Tinymist recolors the paper rectangles and applies a color filter inside each SVG
page; neither operation edits source, recompiles, nor restarts Tinymist.
The frontend's `t` shortcut changes appearance through the same settings path.
The browser regression checks actual paper and ink pixels after dark, warm-light,
and standard palette changes. Source-level `set text`/`set page` injection is not
needed for this path; it would also complicate source offsets and document rules.
The native regression is `TIPTOPTYP_TEST_TINYMIST=/path/to/tinymist python3
scripts/test-native-preview-palette.py` on macOS with Swift and WindowServer.
Add `--hold` to leave the final Tinymist-only window visible for 60 seconds.
It tests a 32-page document, standard dark, comfy dark/light, and appearance
replay after navigation; PNGs are written under `.tiptoptyp/screenshots/agent-review/native-preview-palette`.

A whole-HTML-preview SVG filter regressed native appearance in 2ed0f84:
WKWebView's `takeSnapshot` returned dark pixels while the **visible window stayed
white**. Chromium also passed, so neither is sufficient evidence for that class
of compositing bug. An actual window observation reproduced white before and
verified dark after moving the filter into individual SVG pages. The browser
regression now rejects an HTML-host filter and verifies per-page placement,
paper fills, and newly compiled pages. Native load completion also invalidates
the queued-palette cache so reloads receive current settings instead of retaining
the creation-time appearance. Tinymist's own inversion stays disabled; exactly
one transform handles both normal dark and comfy colors. No source rules,
compilation, per-frame traversal, or idle repaint loop are introduced.

Opening a source outside the workspace keeps the Explorer root. The editor banner
provides an explicit root switch, shared by all tabs in that window. Dragging a
file from Explorer into the editor inserts its path at the existing caret, using
a relative path when it is inside the source directory, in one undoable edit.

Harper additionally excludes Typst equations and technical named arguments while
retaining prose arguments such as body/title/caption. TeX uses Harper's TeX parser.
Spelling suppresses unknown capitalized words within sentences as likely names;
grammar checks still apply. This is a heuristic: sentence-initial names can still
be flagged and a capitalized typo within a sentence can be missed.

## Regression checks

Normal Rust tests cover control actions/placement, outside-root navigation,
path drop/undo, deferred Find editing, pop-out close/restore and outline admission.
The gallery includes the shared controls. `preview-window` captures the detached
PDF window; `preview-native-window` exercises the native parent with bounds tracing.
Native WebView pixels are not included in an egui framebuffer.

`node scripts/test-preview-navigation.mjs` checks bounded frontend scheduling.
The opt-in `scripts/test-preview-e2e.cjs` launches a real pinned Tinymist and a
headless browser. Set `TIPTOPTYP_PLAYWRIGHT` to an installed Playwright module,
`TIPTOPTYP_TEST_TINYMIST` to the pinned executable, and optionally
`TIPTOPTYP_TEST_CHROME` to a Chrome executable. It verifies compiled-heading
navigation, Back/Forward, zoom/fit, text input isolation and palette changes
without rerenders or replacement SVG nodes. It leaves no runtime app dependency.

Desktop composition inspection on 2026-09-25 was unavailable because computer-use
reported that the Mac was locked. Fresh controls and PDF-popout framebuffers were
inspected; this is not a claim of whole-desktop native composition verification.

The native pop-out probe used `TIPTOPTYP_UI_TRACE=1 target/debug/tiptoptyp
--ui-theme catppuccin-latte --ui-snapshot-scene preview-native-window
--ui-screenshot-subdir screenshots/agent-review --ui-screenshot-exit test.typ`.
It did not reach native-view readiness within the bounded 40-second run. The
recorded placement stays below the child titlebar and within its own viewport:

```text
ui.preview.bounds available=(0.0,26.5)-(800.0,700.0) clip=(0.0,26.5)-(800.0,700.0) egui=(0.0,26.5)-(800.0,700.0) native=(0.0,26.5)-(800.0,700.0) viewport=(0.0,0.0)-(800.0,700.0) zoom=1.000 egui_ppp=2.000 native_ppp=2.000
```

The optimized worker comparison in
[`writing-policy-2026-09-25.json`](performance-results/writing-policy-2026-09-25.json)
uses the same 11,760-byte generated fixture, four warmup calls and twenty warm
samples per run, repeated in three fresh processes. Before/after warm medians
were 60.9–62.3 ms / 61.0–61.4 ms; cold initialization was 1.66–1.69 s in both.
The new filtering removed the intended false positives without a material
worker-time change on this Mac. It is not a typing-latency or cross-platform
guarantee. Exclusion lookup uses binary search over sorted nonoverlapping ranges.
The browser evidence in
[`preview-controls-2026-09-25.json`](performance-results/preview-controls-2026-09-25.json)
records fifty palette updates without a renderer rerun or replacement SVG node;
the recorded setter duration excludes GPU paint time.
