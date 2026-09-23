#set page(width: 420pt, height: 550pt, margin: 32pt)
#set text(size: 13pt)
#let pages = int(sys.inputs.at("pages", default: "24"))
= PDF preview check

#link(<last>)[Jump to the last page]

#link("https://example.com/pdf-preview-check")[Open external link]

Select this text. Maths: $x^2 + y^2 = z^2$.

#for n in range(1, pages) {
  pagebreak()
  heading[Page #(n + 1)]
  [Scroll freely, zoom, and select text on page #(n + 1).]
  parbreak()
  lorem(80)
}

== Last page <last>

#link((page: 1, x: 0pt, y: 0pt))[Back to page one]
