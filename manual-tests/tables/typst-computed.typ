#set page(width: 20cm, height: auto, margin: 1cm)
= Concrete grid with a computed cell

The first table opens as a grid. Revenue is editable; the value of price is a
protected compiler sample. Cancel must preserve every byte; Apply and Undo must
preserve the variable expression.

#let price = 60
#table(columns: 2, [Revenue], [#price])

= Whole-table generation

The following table stays in the main code editor. Its spread and variable
column geometry must not offer Edit table.

#let widths = (1fr, 2fr)
#let rows = ([A], [One], [B], [Two])
#table(columns: widths, table.header([*Name*], [*Value*]), ..rows)

= Explicit placement

This layout also stays in the main code editor when its geometry cannot be
mapped safely to an editable grid.

#table(columns: 2,
  table.cell(x: 1, y: 0)[Explicitly positioned],
  table.cell(x: 0, y: 0)[First],
)
