#set page(width: 20cm, height: auto, margin: 1cm)
= Table workbench — lossless source mode

These tables use generated content, headers, comments, or explicit positions.
The editor must retain the complete expression without guessing a rectangular
set of cells. Cancel must preserve every byte. Try a source edit and undo Apply.

#let widths = (1fr, 2fr)
#let rows = ([A], [One], [B], [Two])
#table(
  columns: widths,
  table.header([*Name*], [*Value*]),
  ..rows,
)

#table(columns: 2,
  [A], /* This comment must survive. */ [B],
  [#let local = "Local"; #local], [D],
)

#table(columns: 2,
  table.cell(x: 1, y: 0)[Explicitly positioned],
  table.cell(x: 0, y: 0)[First],
)

#grid(columns: (1fr, 1fr),
  [Nested #table(columns: widths, ..rows)],
  [Unrelated content],
)
