#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Theme gallery

This stable document exercises editor syntax, semantic colors, and the preview.

#let accent = rgb("#4f8cff")
#text(fill: accent, weight: "bold")[A cohesive, theme-aware Typst workspace.]

== Math and code

Inline math is highlighted too: $ integral_0^infinity e^(-x) dif x = 1 $

#for item in ("Native UI", "Live PDF", "Fast rebuilds") [
  - #item
]
