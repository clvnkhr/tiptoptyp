// Isolates native serial resize/handoff checks from dense SVG capture cost.
// Keep the dense tinymist-zoom-stress.typ lane as the capacity regression.
#set page(width: 210mm, height: 297mm, margin: 32pt)
#set text(size: 11pt)
#for index in range(24) {
  set page(width: if calc.rem(index, 3) == 1 { 230mm } else { 210mm })
  [
    = Preview handoff #index
    This small page exercises source anchors, fixed page gaps, and queued
    divider widths without making dense SVG preparation part of the check.

    $ integral_0^1 x^2 dif x = 1 / 3 $

    The native cached image must stay responsive while the vector renderer
    completes its current request. Its successor must then finish at the
    latest viewport size.
  ]
  if index < 23 { pagebreak() }
}
