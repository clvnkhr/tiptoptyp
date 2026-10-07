# Table workbench fixtures

Open a fixture, put the caret in its table, and choose Edit table.

- `typst-static.typ`: cell fill/stroke/alignment/inset, row and column rules,
  explicit widths, and independently collapsible table/cell settings.
- `typst-source.typ`: computed/spread cells and options. The workbench opens
  the complete expression in a highlighted source editor instead of inventing
  a rectangular grid.
- `tex-static.tex`: `tabular`, `tabularx`, `longtable`, `array` column types,
  vertical rules, `booktabs`, and row-height syntax.
- `tex-source.tex`: spans, custom column types and structural commands use
  the complete environment source editor to preserve their meaning.
- `tex-large.tex`: a 201-row table for scroll and input checks.

Apply changes the document buffer in one undo step; Cancel leaves it alone.
Undo/Redo should reproduce the exact before/after source. A changed document
or table range must reject an outdated draft. Applying a TeX grid repeatedly
must not add whitespace. Invalid structural cell input should disable Apply,
but Source must still expose the draft so it can be repaired.

The Typst grid paints literal colors, strokes, alignment and inset values.
Computed functions, gradients and dictionaries remain exact source expressions;
the compiler, rather than the workbench, evaluates them. Complex partial rules
are preserved and edited in Source when a grid operation cannot retain them.
TeX column options follow the detected environment and loaded package names;
unsupported custom column definitions and spans stay in Source.

The native `tables` journey covers Cancel, Apply, Undo/Redo, stale/invalid source
and TeX grid-to-source repair. It requires an unlocked macOS desktop with input
permissions. Compiling a fixture or running semantic tests is separate evidence
from this interaction journey.
