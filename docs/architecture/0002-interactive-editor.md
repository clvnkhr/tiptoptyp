# Interactive editor and preview architecture

Status: accepted, 2026-09-02

## Current-state amendment (2026-09-17)

The decisions below preserve the original rationale, not an exact description of
the current implementation. These notes supersede the corresponding ownership and
execution details in decisions 1–3:

- Settings now owns a separate deferred native viewport. Its local interaction
  repaints independently of document windows; changed preferences/actions return
  to the owner. It no longer temporarily replaces the workspace view.
- Tinymist sessions are owned per document window, not per workspace or per tab.
  Tabs share window services; active editing and the designated preview entry are
  distinct. Document windows also repaint independently, while native UI remains
  on its required thread.
- Interactive preview does not require a parallel CLI compile for every edit.
  The CLI artifact path runs when PDF.js, explicit raster preview, deterministic
  capture or an explicit PDF export needs it. Export still uses canonical PDF bytes, never
  rendered screen pixels. Recovery preserves the requested backend and permits
  four delayed retries before PDF.js fallback on the fifth failure. Opened PDFs
  default to PDF.js independently of the Typst preference. Raster preview is
  retained only for explicit selection and deterministic captures, pending
  [removal](../pdfjs-preview.md#raster-preview-retirement-todos-294295).
- Optional miTeX editing is enabled in the application: displayed TeX dollar
  notation maps to canonical Typst/MiTeX source at save and service boundaries.
  Ordinary documents bypass that projection. CLI compilation still reads imported
  subfiles from disk; this is not a multi-file unsaved-buffer compiler overlay.

### Current ownership map

| Boundary | Implementation and maintained contract |
| --- | --- |
| Shell, independent windows and shared preferences | `src/windowing.rs`; [multi-window audit](../multi-window-audit.md) |
| Window orchestration and tab-owned document state | `src/app.rs`, `src/app/tabs.rs`; [tabs and Git hunks](../tabs-and-git-hunks.md) |
| Settings presentation and deferred host | `src/app/settings_panel.rs`, `src/app/settings_window.rs`; [focused ownership](0005-focused-ui-and-recovery.md) |
| Canonical/displayed document boundary | `src/mitex_document.rs`, `src/mitex_projection.rs`; [miTeX projection](../mitex-projection.md) |
| Protocol, artifact production and preview policy | `src/tinymist.rs`, `src/compiler.rs`, `src/preview.rs`, `core/src/recovery.rs`; [focused recovery](0005-focused-ui-and-recovery.md) |
| Native child-view composition | `src/app/native_views.rs`, `src/child_view.rs`; [native boundaries](0004-native-unsafe-boundaries.md) |

The broader [architecture audit](../../output/pdf/architecture-audit.typ) describes
remaining extraction opportunities; its proposals are not already implemented
boundaries. `todo.typ` items 184–221 track those changes.

## Context

The first MVP used an `egui::TextEdit`, a hand-written lexer, and a persistent
`typst watch` process which emitted PDF. Poppler rasterized every PDF page at a
fixed 120 DPI and the UI displayed one page at a time. That proved the compile
loop, but it also caused several structural problems:

- transient status and diagnostics widgets changed the preview rectangle;
- fixed-resolution textures blurred when zoomed and consumed memory eagerly;
- PDF does not retain Typst source spans, so it cannot implement preview hover
  or source-to-preview navigation;
- the hand-written lexer confused markup words such as `as` with code
  keywords;
- diagnostics were unstructured terminal text and could not decorate source;
- project navigation, panel modes, and find/replace were absent.

The following upstream implementations were inspected before this decision:

- [Typstify](https://github.com/typstify/typstify/tree/df2790e8050aa603312cb41401773a725c16db1d)
  starts Tinymist through LSP and embeds Tinymist's localhost preview in a
  native child webview. It does not implement a PDF viewer or source map.
- [Tinymist preview](https://myriad-dreamin.github.io/tinymist/feature/preview.html)
  sends incremental SVG/vector diffs, stacks pages continuously, carries Typst
  spans for bidirectional jumps, supports partial rendering, cursor-centred
  zoom, hover feedback, and element-aware colour inversion.
- [`typst-syntax`](https://docs.rs/typst-syntax/latest/typst_syntax/)
  exposes Typst's tolerant parser, incremental `Source::replace`, and official
  syntax highlight tags.
- [`egui::InputState::zoom_delta`](https://docs.rs/egui/0.36.1/egui/struct.InputState.html#method.zoom_delta)
  receives macOS trackpad pinch events through winit. The MVP simply never
  consumed it.

## Decisions

### 1. The preview rectangle is a layout invariant

The toolbar, status bar, optional filesystem panel, editor/preview split, and
explicitly toggled Problems panel are the only widgets allowed to consume
layout space. Build, stale, error, and fallback state belongs in the fixed
bottom status bar; the fixed-height preview header contains controls only.
Transient state must never insert or remove content above the preview.
Settings is a dedicated in-app view rather than another side panel. Opening it
temporarily hides the workspace while preserving the preview panel and scroll
state; closing it restores the same preview geometry instead of resizing or
displacing the document.

The native fallback uses one persistent scroll area containing every page with
fixed margins and gaps. Recompilation keeps its scroll offset. Zoom is anchored
under the pointer and a pinch exits fit-to-width mode.

### 2. Tinymist owns interactive document rendering

When a compatible `tinymist` executable is available, tiptoptyp starts one LSP
sidecar per workspace, synchronizes in-memory buffers with `didOpen` and
`didChange`, invokes `tinymist.startDefaultPreview`, and embeds the returned
server URL in a Wry child webview.

The client handles `window/showDocument` so a click in the preview opens the
target file and selects the returned range. A later cursor-synchronization
milestone can invoke `tinymist.scrollPreview` for editor-to-preview movement.
The app consumes only documented LSP commands and Tinymist's own served
frontend; it does not copy or depend on Tinymist's private vector-diff protocol.

The child-webview route is initially supported on macOS and Windows. Wry child
views on Linux require X11 or additional GTK integration, so Linux retains the
native fallback until that work is completed.

### 3. `typst watch` remains authoritative

`typst watch` remains the source of compiler diagnostics and exact PDF bytes.
Export always writes those bytes and never a Tinymist approximation. Unsaved
documents compile through a project-local `.tiptoptyp` shadow source, so PDF
export does not require saving the `.typ` file. Sessions are reused where
filesystem notifications support the private path; macOS restarts the watcher
for buffer edits because dot-directory changes are suppressed.

Running both pipelines costs extra memory when interactive preview is enabled,
but separates concerns cleanly: Tinymist supplies interaction while the stock
Typst CLI supplies the canonical artifact. The rasterised PDF fallback is retained
for machines without Tinymist and for recovery if its preview server fails.

### 4. Syntax highlighting comes from Typst

The custom lexer is replaced by `typst-syntax`. A cached `Source` is updated
incrementally, official highlight tags are mapped only to theme colours, and
the emitted layout remains exactly one-to-one with the editor buffer. Semantic
tokens from Tinymist can augment this later; they are not required for correct
markup/code mode separation.

### 5. Diagnostics are typed and inline decoration is non-editable

Typst 0.15 offers human and short CLI diagnostics but not JSON, so the watcher
output is parsed into file, line, column, severity, summary, and full detail.
Parser diagnostics provide immediate feedback while a compile is pending.

Error/warning lines receive a subtle background and underline through the
editor layout. The short message is painted after the line's final glyph rather
than inserted into the `TextEdit` galley, preserving cursor offsets. Hovering
that virtual text shows the complete diagnostic. Raw output remains available
in a user-toggled Problems panel for diagnostics that cannot be associated with
the active file.

### 6. Workspace and editing controls stay native

An independently toggled filesystem panel shows the discovered project root
and opens `.typ` files with the existing dirty-buffer guard. The main view has
three explicit modes: Code, Split, and Preview. Find/replace uses the existing
`TextEditState` selection model and literal UTF-8-safe match/replace functions.

The Settings panel is the single source of truth for the persisted interface
preference: System (the default), Light, or Dark. System changes arrive through
egui/winit and explicit Light or Dark choices ignore them. Both egui styles use
identical typography and spacing, so an intentional appearance change changes
colours without reflowing the workspace. A Settings change is queued until the
next raw-input hook, before egui constructs that frame's root UI, which prevents
a half-light/half-dark frame. Native window decorations stay synchronized.

Document appearance is a separate persisted setting: Follow interface, Light
pages, or Dark pages. PDF fallback inversion affects preview pixels only, never
exported bytes.

### 7. Fallbacks preserve intent and are always observable

The requested preview backend and effective backend are separate state. A
Tinymist, preview-server, or embedded-webview failure must never rewrite the
user's Interactive preference to Rasterised PDF. While the watched-PDF viewer is
being used automatically, only the fixed bottom status bar shows the live
fallback badge and reason; the preview header contains controls only. Settings
reports the requested and effective backends plus structured service status.
Explicitly choosing Rasterised PDF is not classified as a fallback.

## Rejected alternatives

- Increasing Poppler DPI, PDFium, PDFKit, or MuPDF improves sharpness but still
  cannot reconstruct Typst spans.
- Official Typst SVG is vector but does not carry the source map needed for
  preview interaction.
- Porting `tinymist-preview`, reflexo/typst.ts, and frontend assets would bind
  the app to an internal protocol and a patched compiler for no user benefit.
- Tinymist's native Vello viewer is promising, but it is currently a standalone
  Xilem/Masonry application and uses WGPU 27 while egui-wgpu 0.36 uses WGPU 30.
  It is a separate future milestone rather than a drop-in widget.

## Verification requirements

- Preview content bounds are identical across waiting, compiling, stale,
  ready, and error states.
- A three-page document has monotonically increasing continuous page geometry.
- Zoom tests preserve the content point under the pointer and disable Fit.
- System mode tracks synthetic Light and Dark events, while explicit modes
  ignore conflicting events; both styles retain identical layout geometry.
- Missing system appearance data resolves deterministically to Dark and is
  reported as a theme fallback.
- Markup `The PDF preview updates as you type.` leaves `as` plain, while code
  mode highlights `#import "x.typ": value as alias` correctly.
- Diagnostic parsing covers Unix paths, Windows drive paths, warnings,
  continuations, UTF-8 columns, and unrelated-file diagnostics.
- Find next/previous wraps; replace-one and replace-all handle UTF-8 and empty
  queries without loops.
- Export is available for an unsaved document after a successful build and
  writes the exact watched PDF bytes.
- Filesystem traversal is sorted, excludes internal preview artifacts, and
  cannot escape the project root.
- Tinymist protocol tests reject stale generations and map a mock
  `window/showDocument` request to the expected path and selection.
- Tinymist/webview failure never changes the persisted backend preference and
  exposes the effective native backend plus its reason.
