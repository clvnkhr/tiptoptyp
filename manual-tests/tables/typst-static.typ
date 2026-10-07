#set heading(numbering: "1.")
#set page(width: 20cm, height: auto, margin: 1cm)
= Table workbench — static cells

Right-click any cell to open the editor. Try changing column widths, selecting
an edge, then adding a colored rule. Cancel must leave the source untouched.

#table(
  columns: (20%, 1fr, auto),
  inset: (x: 6pt, y: 3pt),
  align: (left, center, right),
  fill: (white, rgb("#eef2f6"), white),
  stroke: 0.5pt + gray,
  table.hline(y: 0, stroke: 1pt + blue),
  table.vline(x: 1, stroke: 1pt + red),
  [*Name*], [*Formula*], [*Meaning*],
  [$alpha$], [$x^2$], [See @static-note],
  table.hline(stroke: 0.8pt),
  [Unicode: Héllo], [#strong[Inline markup]], [Third row],
)

= A reference target <static-note>

= Spans

#table(
  columns: (1fr, 1fr, 1fr),
  table.cell(colspan: 2, fill: rgb("#dcfce7"))[*Heading*], [Third],
  table.cell(rowspan: 2)[Two rows], [Middle], [Right],
  [Next middle], [Next right],
  table.hline(y: 3, stroke: 1pt + blue),
)
