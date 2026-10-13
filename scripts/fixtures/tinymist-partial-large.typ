// Bounded real-renderer fixture: 48 pages, dense math, unique ink per page.
// The browser probe checks the ink, not merely virtual page skeleton geometry.
#set page(width: 500pt, height: 680pt, margin: 24pt)
#set text(size: 8pt)
#set par(leading: 2pt)
#let revision = 0
#let page-count = 48
#for index in range(page-count) {
  if index > 0 { pagebreak() }
  let channels = if index == 31 and revision == 1 {
    (240, 80, 20)
  } else {
    (36 + calc.rem(index * 37, 180), 30 + calc.rem(index * 73, 180), 20 + calc.rem(index * 47, 180))
  }
  let marker = rgb(..channels)
  place(top + left, rect(width: 8pt, height: 632pt, fill: marker, stroke: none))
  [
    #heading(level: 1)[Preview page #(index + 1) / revision #revision]
    #label("preview-page-" + str(index))
  ]
  let target = if index == 0 { 40 } else { 2 }
  link(label("preview-page-" + str(target)))[Jump to preview page #(target + 1)]
  for row in range(8) {
    block(spacing: 2pt)[
      Row #row: $ norm(u_(#index,#row))_(H^s)^2 <= C sum_(k=1)^n frac(k^2 + alpha, (1 + k)^3)
        + integral_0^1 abs(f(t))^2 dif t $.
    ]
  }
}
