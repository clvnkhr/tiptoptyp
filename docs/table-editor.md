# Table workbench

Use **Edit → New Table…** to insert a 2×2 draft at the cursor, or **Edit → Edit
Table…** when the cursor is inside an editable `table`/`grid`. The latter also
appears in the editor's context menu. Both commands can be assigned shortcuts
in Settings. New Table does not change the source until Apply.

The workbench is a movable, resizable native window. It opens at 860×560 points
and can shrink to 620×420. A compact toolbar sits above the grid; labelled
**Apply** and **Cancel** buttons stay pinned below it while the cells scroll.
While it is open, a banner marks the source editor **Read-only**. Scrolling, selection,
copying, and searching still work. Typing, paste, cut, undo/redo, replace,
formatting, completions, font replacement, Git hunk changes and notation-mode
changes cannot alter the source. Document/tab changes are held until the draft
is applied or cancelled. Apply validates the exact original document revision
and table slice, then makes one Unicode-safe, undoable replacement.

## Cells and appearance

The Typst toolbar groups row/column actions, **Remove**, **Format**,
**Import Markdown** and **Source** in one compact row, wrapping only when
necessary. Format opens the appearance controls for **Whole table** or
**Selected cells**, including spans; they do not occupy a persistent inspector.
The following selection and cell-navigation shortcuts apply to Typst grids.
TeX cells retain normal code editing, including Tab for indentation.

- Add rows/columns or remove the selected row/last column.
- Click a cell, row number, or column heading to select. Shift-click extends
  the rectangular selection.
- Tab/Shift+Tab moves between editable cells, skipping covered span positions.
- Option/Alt+arrows moves between cells; add Shift to extend the selection.
- Shift+Space selects a row; Control+Space selects a column.
- Command/Control+Shift+A selects all cells. Ordinary Command/Control+A still
  selects the text inside the focused cell.
- Adjust row/column spans for a single cell. Expansion refuses to overwrite
  neighbouring content or styling. Clear those cells first. Shrinking a span
  exposes empty cells again.
- Border, background, alignment and padding presets apply to the whole table
  or the selected cells. Unrecognised existing expressions are shown as
  **Custom** and are preserved unless explicitly replaced.

Variables now display read-only compiler samples instead of editable source.
Typst cells containing computed markup (for example `[#price]`) and TeX cells
using a named macro (for example `\revenue`) are locked. Literal
neighbouring cells remain editable. Named TeX commands are conservatively sampled
because their definitions may come from a package or imported preamble; escaped
punctuation and literal `\verb` text remain editable. Typst's cell navigation
skips locked cells; Apply also verifies that their original expressions are
unchanged. Geometry changes and Markdown
replacement are disabled while the grid contains locked cells.

Generated Typst tables, including spreads and variable column counts, default
to a read-only sampled grid. Inherited column settings are resolved by the
compiler. The Source switch is an explicit way to change the complete table
expression; samples describe the document as opened, not an unapplied source
draft. Apply source changes and reopen to refresh the samples. If the table runs
multiple times, the first instance is shown. Cells unsupported by sampling stay
locked and show an explanation rather than becoming editable expressions.

Sampling uses the configured Typst/TeX compiler in one cancellable background
job per opening. A private source mirror preserves project-relative imports
and the unsaved buffer; original documents and exported PDFs are untouched.
Samples are limited to 4,096 grid positions, 128 columns, 16 KiB per cell and
8 MiB of compiler output, with a 60-second timeout. Closing the workbench cancels
the job and removes its private files. TeX samples expand macros in the compiler’s real context, including imported
preambles and packages;
complex protected typesetting commands may remain in the displayed expansion.

Spans render as merged editing areas. This is a source-markup editor, not a
Typst typesetting preview: the document preview shows the final typography and
appearance after Apply. Typst's supported span and styling parameters are
documented in the [table reference](https://typst.app/docs/reference/model/table/).

## TeX packages

The same grid workbench recognises `tabular`, `tabular*`, `tabularx`, `tabulary`,
`longtable`, `xltabular`, `NiceTabular`, `NiceTabular*`, and tabularray's `tblr`,
`longtblr` and `talltblr`. It keeps width/placement arguments, package options,
row heights, booktabs rules, colortbl row colours, dashed rules and longtable
header/footer markers. Header/footer rows remain in source order in the grid.

Column editing supports array's `p/m/b`, `w/W`, modifiers and repetitions;
siunitx's `S[options]`; dcolumn's `D{input}{output}{digits}`; tabulary's `L/C/R/J`;
and tabularray's `Q[options]` and weighted `X[options]`. For tabularray, the
Columns field changes only `colspec`, preserving the other inner/outer options.
Apply adds missing `\usepackage` declarations to the preamble automatically,
including column/rule/colour dependencies and `\UseTblrLibrary` declarations
for tabularray. Existing declarations, options and comments are kept. Packages
are never removed automatically: other code may still use them. The source
change and imports are one undo step.

The same import manager handles drawn TeX symbols (using the selected symbol's
package metadata), math-environment conversion, TeX completions/snippets and
new document templates. It ignores comments and literal verbatim examples,
recognises comma-separated package lists and `\RequirePackage`, and retains
line endings and Unicode cursor/tabstop positions. It does not install packages;
the configured TeX distribution supplies them as usual.

Included fragments without a preamble are not rewritten with invalid body-level
imports. If imports are needed, the edit reports which packages must be added to
the main preamble before it can be applied. Dynamic package names, custom class
internals and imported preamble files are not evaluated by this lexical manager.
It adds only empty-option imports, so existing package options remain authoritative.
Dependency/library rules follow the [tabularray manual](https://mirrors.ctan.org/macros/latex/contrib/tabularray/tabularray.pdf)
and the package references below.

Merged cells (`\multicolumn`, `\multirow`, `\Block`, `\SetCell`, structural
`cell` options), generated/inherited column specifications, custom column types,
longtable captions and nicematrix drawing blocks use exact Source editing.
Their source is retained rather than flattened into an incorrect rectangular
grid. The compiler samples named macros in supported grid cells with the
original package and imported-preamble context.

The checked-in [package fixture](../tests/fixtures/tex-table-packages.tex)
covers 12 formats, including real macro sampling in numeric columns. Three
[arXiv fixtures](../tests/fixtures/arxiv-tables/provenance.json) also retain
complete original table environments with versioned attribution and checksums:
[SciPy's roadmap](../tests/fixtures/arxiv-tables/scipy-roadmap.tex) and
[optimizer comparison](../tests/fixtures/arxiv-tables/scipy-optimizers.tex) open
as grids; the [Platonic metric comparison](../tests/fixtures/arxiv-tables/platonic-metrics.tex)
uses Source to preserve its merged cells. All three are standalone documents.
Grid Apply preserves the author's spacing, comments and rules; omitted trailing
empty cells remain omitted unless edited. Fixture regressions cover detection,
no-op Apply, exact edits, cancellation and Undo. The `arxiv_tables` native
journey exercises the same edits through the real workbench.

Package syntax follows the maintainers' references for
[tabularray](https://ctan.org/pkg/tabularray),
[tabulary](https://ctan.org/pkg/tabulary),
[xltabular](https://ctan.org/pkg/xltabular),
[nicematrix](https://ctan.org/pkg/nicematrix),
[siunitx](https://ctan.org/pkg/siunitx) and
[array/dcolumn](https://ctan.org/pkg/latex-tools).

## Markdown

Import Markdown accepts one pasted GFM table with a header separator. The import
button replaces the draft, not the document, and preserves table-level styling.
Column alignments and emphasis are retained. Plain text (including inline-code
text) is escaped for Typst so `#`, `$`, brackets and other markup cannot become
code. Links, images, HTML, surrounding prose and multiple tables are rejected
with a visible error. A failed import leaves the existing draft untouched.

## Bounds and source safety

The visual model is limited to 4,096 grid positions and 128 columns. It supports
static content cells and literal `table.cell`/`grid.cell` row/column spans, with
preserved named styling arguments. Dynamic cells, spreads, explicit `x`/`y`
placement, structural helpers (`header`, `hline`, etc.), comments between
arguments and incomplete rows use the sampled grid with an explicit Source
editor. Cancelling never rewrites them.

The cursor query reuses the editor's parsed syntax and caches results by
document revision/cursor. Unchanged frames do not reparse the source. Grid work
is bounded and offscreen cell widgets are skipped; this feature adds no frame
polling, per-frame I/O or continuous repaint. Computed tables start one finite background
sampling job and repaint only when it finishes. Tests cover reuse and
bounds; no cross-platform performance claim is made.

Cells use a compact 36-point row pitch with line-number and column headings.
Long text scrolls within its own clipped editing area rather than increasing
row height. The complete grid scrolls independently above the pinned footer,
so Apply/Cancel remain visible even at the minimum window size. The Typst
footer identifies the selected cell; validation messages occupy the footer
without expanding the grid.

## Checks

`cargo test --bin tiptoptyp table_` covers spans, dimension changes, loss-safe
errors, Markdown escaping, selection, keyboard navigation, minimum window
geometry, source locking/copy/scroll, menu availability, insertion and undo.

Deterministic framebuffer scenes: `table-editor` and `table-editor-narrow` for Typst;
`table-editor-tex` and `table-editor-tex-narrow` for TeX. The real source fixtures
are in `scripts/fixtures/table-editor.typ` and `scripts/fixtures/table-editor.tex`.
An explicit `--ui-screenshot main:table-source-lock` with the former captures
the parent source banner separately; it does not composite the child window.

On 22 September 2026, fresh Catppuccin Latte (940×700-point) and Mocha
(620×600-point) workbench framebuffers and the parent read-only banner were
inspected under `.tiptoptyp/screenshots/agent-review/table`. Narrow-window
inspector/footer clipping was corrected, then recaptured and inspected.
The 68-image gallery was regenerated in one session and validated. This also
caught a QA scene-reset bug: restoring into the empty-workspace placeholder
disabled later preview captures; resets now retain a real document tab.

The opt-in compiler integration passed with the bundled Typst executable:
`TIPTOPTYP_TEST_TYPST=/path/to/typst cargo test --bin tiptoptyp table_generated_spans_and_markdown_compile_with_typst -- --ignored`.
It compiles model-generated combined spans, styling and escaped Markdown into
a real PDF. The native `tables` journey resizes the workbench to 620×420,
edits long cell text, scrolls the grid and checks the pinned footer. Framebuffer
captures remain separate evidence for the child viewport's pixels.

On 11 October 2026, 13 sampled-value regressions passed, including actual bundled
Typst/Tectonic evaluation, imported definitions, inherited columns, generated
rows, read-only typing, Tab navigation, Apply protection, cancellation and private
file cleanup. The native `tables` journey passed on macOS 14.6.1/Apple Silicon;
it verifies real compiler value fingerprints, attempts to type into locked
samples, edits neighbouring literal cells and checks Apply/undo. Fresh workbench
framebuffers were inspected and retained under
`.tiptoptyp/screenshots/agent-review/table-samples`. These verify the child
viewport pixels, not composed desktop geometry. The full Rust and xtask suites,
56 Python journey-contract tests, formatting and strict clippy checks passed.
The maintained gallery was refreshed in one app session and all 25 PNGs validated.
Sampling adds one finite job when opening a computed table and no idle polling
or per-frame filesystem work; no cross-platform speed claim is made.

The TeX package extension added nine focused parser/UI/compiler regressions.
All 12 package formats compiled and resolved real macro samples with the bundled
Tectonic engine. The extended native table journey passed with Accessibility and
input permissions on macOS; tabularray/siunitx child framebuffers were freshly
captured and inspected under
`.tiptoptyp/screenshots/agent-review/tex-table-packages`. Formatting, strict
clippy, the full Rust/xtask suites and 56 journey-contract checks passed.
Package parsing uses the existing revision/cursor cache and bounded column/option
scans; it introduces no new idle work or background polling.

Automatic import coverage was verified on 11 October 2026 by 16 focused Rust
regressions and the real macOS table journey, including missing siunitx imports,
Cancel, Apply, Undo/Redo and repeat Apply without duplication. Shared import
planning also preserves snippet fields and loads packages before preamble
configuration commands. See `docs/desktop-ui-audit.md` for the build receipt and
native evidence. Imports add no idle/per-frame work.
