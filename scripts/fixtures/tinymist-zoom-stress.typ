// Opt-in renderer stress case: 48 fractional-size pages, fourteen dense matrices
// and nested estimates per page, plus real links and source-update marker ink.
// Keep the smaller tinymist-partial-large.typ for quick acceptance checks.
#set page(width: 210mm, height: 297mm, margin: 24pt)
#set text(size: 5pt)
#set par(leading: 1pt)
#set math.equation(block: true)
#let revision = 0
#let page-count = 48
#for index in range(page-count) {
  if index > 0 { pagebreak() }
  // Exercise centering and unequal paper widths as well as fractional points.
  set page(width: if calc.rem(index, 3) == 1 { 230mm } else { 210mm })
  let channels = if index == 31 and revision == 1 {
    (240, 80, 20)
  } else {
    (36 + calc.rem(index * 37, 180), 30 + calc.rem(index * 73, 180), 20 + calc.rem(index * 47, 180))
  }
  place(top + left, rect(width: 8pt, height: 793pt, fill: rgb(..channels), stroke: none))
  [
    #heading(level: 1)[Preview page #(index + 1) / revision #revision]
    #label("preview-page-" + str(index))
  ]
  let target = if index == 0 { 40 } else { 2 }
  link(label("preview-page-" + str(target)))[Jump to preview page #(target + 1)]
  for row in range(14) {
    block(spacing: 1pt)[
      $ mat(
          frac(alpha_1^2+beta_1^2,1+lambda^2), frac(partial_1 u_1,1+norm(x)^2), frac(Gamma(n+alpha),Gamma(n+1)), frac(integral_0^1 f(t) dif t,1+tau^2);
          frac(partial_2 u_1,1+norm(x)^2), frac(alpha_2^2+beta_2^2,1+lambda^2), frac(Gamma(n+beta),Gamma(n+1)), frac(integral_0^1 g(t) dif t,1+tau^2);
          frac(partial_1 u_2,1+norm(x)^2), frac(partial_2 u_2,1+norm(x)^2), frac(alpha_3^2+beta_3^2,1+lambda^2), frac(sum_(k=1)^n k^2,1+n^3);
          frac(integral_(bb(R)^3) abs(hat(u))^2 dif x,1+norm(u)^2), frac(sum_(j=1)^n j^2,1+n^3), frac(Gamma(n+gamma),Gamma(n+1)), frac(alpha_4^2+beta_4^2,1+lambda^2)
        )
        norm(u_(#index,#row))_(H^(s+alpha))^2
        <= frac(C^(n+1) Gamma(1-frac(j,2alpha-kappa)) Gamma(1-frac(1,2alpha-kappa))^n,
          Gamma(n (1-frac(1,2alpha-kappa))+1-frac(j,2alpha-kappa)))
        integral_(r<r_1<dots<r_n<theta)
        product_(k=1)^n (r_k-r_(k-1))^(-frac(1,2alpha-kappa)) dif r_1 dots dif r_n $
    ]
  }
}
