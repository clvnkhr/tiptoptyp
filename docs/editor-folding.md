# Folding and sticky context

Click a foldable line number or its triangle to collapse/expand it. A collapsed
header ends with a clickable `...`; the dots are a view decoration, never source
text. The triangle uses an eight-point lane taken from the existing gutter
padding, between Git markers and numbers. Git and fold hit targets do not overlap.

Folding and sticky context share one revision-cached structural index. This
includes the existing Typst headings, declarations, functions, blocks, calls,
collections, equations, and raw blocks, plus embedded languages:

- Markdown in `cmarker.render` / `render-with-metadata` positional or named
  literals, and `md` / `markdown` raw blocks. CommonMark sections (including
  Setext headings), code blocks, lists/items, and block quotes are recognized.
  Local headings inside containers do not end outer document sections.
- TeX in the existing MiTeX call names (`mi`, `mitex`, `mimath`, `mitex-convert`,
  `mitext`), including named `input`, and `tex` / `latex` raw blocks. Sections,
  environments, brace groups, and display math are recognized. Comments,
  escaped symbols and verbatim content cannot introduce false structure.

Markdown uses [pulldown-cmark's source-offset iterator](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Parser.html#method.into_offset_iter).
TeX uses a tolerant structural reader, not macro expansion or an additional LSP.
String escapes are decoded with a byte-boundary map back to the literal source;
escaped newlines do not invent physical editor rows. Only physically multiline
regions can be folded or pinned.

Up/down arrows skip hidden rows. Search and explicit cursor destinations reveal
their containing folds. Nested fold choices survive toggling a parent; edits
outside a fold track its new position, and edits touching it reveal it. Replacing
the document clears its folds, as does hiding line numbers. Folds are window-local
view state, not persistent document content.

The galley retains every original source character, including hidden rows, so
selection/copy, edits, undo, search, diagnostics and hover offsets remain source
offsets. Concealed rows have zero height and no mesh. The folded galley is cached
by its input identity and invalidated by fold, source, font or width changes.
When folded, a small right-hand suffix allowance keeps `...` visible after soft
wrapping. Offscreen line numbers and hidden decorations are not painted.

## Verification

Focused deterministic tests:

```sh
cargo test --bin tiptoptyp folding
cargo test --bin tiptoptyp embedded_structure
cargo test --bin tiptoptyp sticky_context
```

Manual optimized microbenchmark (no timing threshold in CI):

```sh
cargo test --release --bin tiptoptyp folding_large_document_measurement -- --ignored --nocapture
```

It reports cold parse/index preparation, cold projection and cached projection
lookup separately for 500 folded functions. Font layout is outside the projection
timer. This is not an end-to-end input-latency or scrolling benchmark.

The `folding` screenshot scene shows collapsed Typst, Markdown and TeX regions
beside expanded siblings and Git markers. See [UI QA](ui-qa-screenshots.md) for
fresh framebuffer capture and gallery validation.

## Measurement record — 2026-09-15

macOS 14.6.1, ARM64, Rust 1.96.0, optimized builds. Native runs used the identical
large-source fixture (SHA-256 `5ce2455c087318a81fb02d53062d9c65de92afe11df220f443a15a0ee94f815e`),
six-second warmup and six-second sample window, without manual interactions:

- Baseline: `.tiptoptyp/profiles/1789502916911-45868-large-0`.
  70 source passes, mean inclusive wall time 3.678 ms.
- Initial folding/index implementation with offscreen number culling:
  `.tiptoptyp/profiles/1789505732301-48407-large-0`.
  21 source passes, mean inclusive wall time 1.851 ms.
- Final runtime including suffixes and larger arrows:
  `.tiptoptyp/profiles/1789506472391-50935-large-0`.
  No UI passes or root repaint requests during the measurement window; the app
  was idle. This is not a zero-cost frame measurement. Different background
  settling/frame counts mean totals must not be presented as a speedup ratio.

The final release microbenchmark used 251,890 bytes, 8,001 source rows and 500
folded functions (501 visible rows). Three runs with no concurrent GUI capture
or build measured cold parse/index preparation at 5.31–6.11 ms, cold projection
at 3.29–3.77 ms, and cached projection lookup at 10–13 ns per call (10,000 calls).
These are in-process wall times, excluding font layout, painting, input dispatch
and GPU work—not scroll FPS or total toggle latency. Tests separately assert
cache identity and source-coordinate invariants instead of asserting timings.
