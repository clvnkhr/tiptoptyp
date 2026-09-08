#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Theme gallery

This stable document exercises editor syntax, semantic colors, and the preview.

#let accent = rgb("#4f8cff")
#text(fill: accent, weight: "bold")[A cohesive, theme-aware Typst workspace.]

== Embedded sources and colors

// Offline no-op stand-ins keep the visual highlighting fixture compilable.
#let mi(body) = none
#let cmarker = (render: body => none)
#mi(`\frac{1}{2} + \alpha`)
#(cmarker.render)(`# Markdown *inside* Typst`)
#let gray = luma(128)
#let print-red = cmyk(0%, 100%, 100%, 0%)

== Math and code

Inline math is highlighted too: $ integral_0^infinity e^(-x) dif x = 1 $

```rust
fn main() { println!("theme"); }
```

#for item in ("Native UI", "Live PDF", "Fast rebuilds") [
  - #item
]
