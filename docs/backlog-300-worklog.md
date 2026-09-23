# Backlog above 300: implementation record

The 23 September 2026 request is to verify and commit the accumulated fixes,
complete or retire stale todos numbered above 300, then archive the historical
todo file and replace it with outstanding work only. Original numbers will be
retained for traceability. The old rule prohibiting removal of completed entries
is superseded by this request.

Verified baseline: commit `67fbc3a` includes the reviewed UI, Git, PDF utilities,
workspace, process customization and terminal fixes. Formatting, strict Clippy,
1,214 Rust tests, 14 xtask tests and 5 PDF.js host tests passed. Two unrelated
untracked PDFs (`editor-comparison.pdf`, `refactor-walkthrough.pdf`) were excluded.

## Implemented in this pass

- **306:** Edit → Toggle Typst Comfy Defaults inserts/removes a clearly delimited
  block of ordinary Typst page/text color rules using the current UI palette.
  Active comfy documents update their defaults when the theme changes. Source
  colors bypass preview inversion. These are visible, undoable source edits and
  therefore apply to compiled/exported documents too; TeX documents are unchanged.
- **307:** Optional offline British English checking through Harper's Typst and
  TeX parsers. A 600 ms typing pause, one background job at a time, document/settings
  identity checks, and a 2 MB document limit bound work. Results join Problems and
  inline diagnostics. There is no idle polling or network grammar service.
- **308:** Optional Unicode diagnostics plus character outlines for invisible,
  directional and full-width characters and mixed-script Latin lookalikes.
  Ordinary CJK/Greek/Cyrillic prose is not flagged merely for being non-ASCII.
- **309:** Invalid UTF-8 source opens an explicit legacy-encoding preview. Import
  creates an unsaved copy, preserves original bytes and rejects decoding errors.
  GB18030, Big5, Shift-JIS, EUC-JP, ISO-2022-JP, EUC-KR, Windows-1252 and Mac Roman
  are available. Tests cover independently verified CTAN/arXiv byte excerpts,
  complete synthetic fixtures/goldens, round trips, invalid bytes and mixed input.
  See [encoding details](text-encoding.md).
- **310 / 181:** File → New from Template provides complete Typst article, letter
  and notes documents, and LaTeX article/letter documents. Templates are new,
  unsaved tabs. Their source fixtures live in `manual-tests/templates/`.
- **311:** Closed as stale after reviewing existing LSP snippet expansion and
  atomic-edit tests. Defaults, choices, variables and final cursor placement
  already work. This does not claim a VS Code-style multi-placeholder Tab session.
- **315:** Remaining action buttons use shared vector icons, accessible names and
  hover labels: settings, reset/apply, file dialogs, package operations, table
  editing, search replacement and retry controls. Data values, selectors, headings,
  and conventional menu lists retain text.
- **337:** Two bounded PDF.js surfaces stage changed documents while retaining the
  interactive successful viewer. Swap follows visible-page/text readiness and the
  latest navigation state. Failed/superseded builds preserve the displayed PDF;
  unchanged bytes reuse it. Page, zoom, scroll, sidebar, rotation, layout and search
  survive replacement. Staging does not steal keyboard focus. Native navigation
  admits only the exact capability-protected viewer/frame URLs.

Items **301–305, 312–314, 316–336 and 338** were already implemented in the verified
baseline. Their detailed historical evidence is retained in the archive. The
terminal fix uses current Nerd Font glyphs; no legacy-codepoint remapping was added.

## Backlog organization

`archive/todo-2026-09-23.typ` preserves the complete numbered checklist and work
diary, with this pass's completions marked. The root `todo.typ` contains the 19
remaining older tasks only, in ascending order, retaining their original numbers
and acceptance notes. New tasks start at 339. No unresolved older task was silently
dropped; task 181 was the duplicate template request completed by task 310.

## Verification and limits

- All three Typst templates compiled with the bundled Typst binary; both LaTeX
  templates compiled with bundled Tectonic (the letter class required a standard
  bundle download on this machine).
- Six deterministic PDF host tests cover bounded staging, unchanged revisions,
  late navigation, failed loads, supersession and switching documents.
- The real-browser check preserves page 13, 175% zoom and scrollTop 15,294 through
  a changed-artifact reload; search, thumbnails, internal/external links, themes,
  page-count changes and resize checks pass.
- The 22-image maintained gallery was regenerated in one app session, validated,
  and its fresh contact sheet inspected. The initial 67-second watchdog expired;
  the bounded 202-second rerun completed. Viewport PNGs are not proof of composed
  native child-window geometry.
- Harper's TeX parser is intentionally syntax-aware rather than a full TeX macro
  engine. Writing checks are opt-in. Encoding import converts bytes, not obsolete
  TeX package/font declarations; mixed-encoding files still require repair.

- Final Rust suite: **1,225 passed, 25 opt-in ignored**. Formatting, strict Clippy
  across all targets, and **14 xtask tests** passed. Dependency notices were
  regenerated for Harper and encoding_rs. The shortened todo document compiles.
- Optimized writing check, Apple Silicon/aarch64, release profile, fixed 1,800-byte
  Typst fixture: disabled 61 µs; first grammar run 492,691 µs; warm grammar run
  16,431 µs; Unicode-only 43 µs. These are individual local samples, not latency
  guarantees. First-use initialization is separate from warm checks. No timing
  threshold is part of the deterministic test suite.

- Matched optimized PDF comparison (Chromium 154, macOS arm64, 24 pages,
  900×700 viewport at device scale 2): old close/open took 118.9 ms and exposed
  7 blank frames; staged replacement took 205.6 ms with 0 blank frames and at
  most two viewer frames. This is a presentation improvement with additional
  work and transient memory, not a throughput speedup. Reproducibility metadata
  is retained in `docs/performance-results/backlog-2026-09-23.json`.
- Native WKWebView passed the real gesture test (150% → 180%), scroll and
  page-width reset. Its newly captured framebuffer was inspected.
- Fixed an existing `cargo-about` template bug that omitted dependency names
  from generated license notices; the regenerated notices identify the new
  grammar and encoding libraries correctly.

- Fresh targeted template, encoding-import and Unicode-marker viewport PNGs were
  inspected: controls fit, Chinese text renders correctly, and outlines target
  the expected characters. The captures are under `.tiptoptyp/screenshots/agent-review/backlog/`.
- Stronger native WKWebView test: **three consecutive staged replacements, zero
  blank frames**, with page, zoom and scroll preserved. The new native framebuffer
  was inspected. An earlier standalone run timed out; an instrumented rerun and
  the three-reload run passed without reproducing it. Its cause remains unknown;
  the probe now emits frame/loading diagnostics on a bounded timeout.
