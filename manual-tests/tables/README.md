# Table workbench fixtures

Open a fixture, put the caret in a supported table, and choose Edit table.
All workbench edits happen in the grid. The main code panel remains the place
to edit table-generating expressions or unsupported geometry.

- `typst-static.typ`: cell fill/stroke/alignment/inset, row and column rules,
  explicit widths, and independently collapsible table/cell settings.
- `typst-computed.typ`: a concrete grid with a protected computed cell, followed
  by generated and explicitly positioned examples. Unsupported geometry must not
  offer Edit table.
- `tex-static.tex`: `tabular`, `tabularx`, `longtable`, `array` column types,
  vertical rules, `booktabs`, and row-height syntax.
- `tex-structural.tex`: merged cells, numeric columns, longtable heads/feet and
  nested cell markup. Representable tables retain their complete syntax in the grid.
- `tex-unsupported.tex`: valid TeX with a computed column count; it must remain
  in the main code editor and must not offer Edit table.
- `tex-large.tex`: a 201-row table for scroll and input checks.

Apply changes the document buffer in one undo step; Cancel leaves it alone.
Undo/Redo should reproduce the exact before/after source. A changed document
or table range must reject an outdated draft. Applying a TeX grid repeatedly
must not add whitespace. Invalid structural cell input disables Apply; repair
that same cell or Cancel.

The Typst grid paints literal colors, strokes, alignment and inset values.
Computed cell expressions and unknown TeX macros display protected compiler
samples; their definitions remain in the document. Literal neighbours stay
editable. TeX column options follow the detected environment and loaded package
names; unsupported column geometry must not open a substitute source editor.

The native `tables` journey covers Cancel, Apply, Undo/Redo, invalid-cell repair,
locked samples, rejected detection and compact grid scrolling. `arxiv_tables`
edits all three attributed real-world excerpts through their grids and checks
exact whole-file replacement and Undo. Both journeys require an unlocked macOS
desktop with input permissions. Compiling a fixture or running semantic tests
is separate evidence from these interaction journeys.
