  $
    & norm(bold(S)_((n)) (r_(n+1),r_0))_(H^s arrow.r H^(s+j)) \
    & lt.eq C^(n+1)
      integral_(r<r_1<dots<r_n<theta)
      (r_(n+1)-r_n)^(-j/(2alpha-kappa))
      product_(k=1)^n (r_k-r_(k-1))^(-1/(2alpha - kappa))
      dif r_1 dots dif r_n #<eq:volterra-simplex-majorant> \
    &= C^(n+1)
      frac(
        Gamma(1-j/(2alpha-kappa))Gamma(1-1/(2alpha-kappa))^n,
        Gamma(n(1-1/(2alpha-kappa))+1-j/(2alpha-kappa))
      ) h^(
        n(1-1/(2alpha - kappa))-j/(2alpha-kappa)
      ). #<eq:volterra-simplex-bound>
  $
