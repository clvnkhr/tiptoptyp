# Table workbench

Use **Edit → New Table…** to insert a 2×2 draft at the cursor, or **Edit → Edit
Table…** when the cursor is inside an editable `table`/`grid`. The latter also
appears in the editor's context menu. Both commands can be assigned shortcuts
in Settings. New Table does not change the source until Apply.

The workbench is a movable, resizable, settings-style native window. While it
is open, a banner marks the source editor **Read-only**. Scrolling, selection,
copying, and searching still work. Typing, paste, cut, undo/redo, replace,
formatting, completions, font replacement, Git hunk changes and notation-mode
changes cannot alter the source. Document/tab changes are held until the draft
is applied or cancelled. Apply validates the exact original document revision
and table slice, then makes one Unicode-safe, undoable replacement.

## Cells and appearance

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
  **Custom (kept)** and are preserved unless explicitly replaced.

Spans render as merged editing areas. This is a source-markup editor, not a
Typst typesetting preview: the document preview shows the final typography and
appearance after Apply. Typst's supported span and styling parameters are
documented in the [table reference](https://typst.app/docs/reference/model/table/).

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
arguments and incomplete rows stay source-only. Cancelling never rewrites them.

The cursor query reuses the editor's parsed syntax and caches results by
document revision/cursor. Unchanged frames do not reparse the source. Grid work
is bounded and offscreen cell widgets are skipped; this feature adds no polling,
background worker, per-frame I/O or continuous repaint. Tests cover reuse and
bounds; no cross-platform performance claim is made.

Long cell text scrolls within its own clipped editing area, and the inspector
switches to two columns in narrow windows so Apply/Cancel remain visible.

## Checks

`cargo test --bin tiptoptyp table_` covers spans, dimension changes, loss-safe
errors, Markdown escaping, selection, keyboard navigation, minimum window
geometry, source locking/copy/scroll, menu availability, insertion and undo.

Deterministic framebuffer scenes: `table-editor` and `table-editor-narrow`.
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
a real PDF. Physical window dragging/resizing remains a manual acceptance check.
