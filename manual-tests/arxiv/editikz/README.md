# EdiTikZ fixture

Real arXiv project for manual TeX preview checks: 39 `.tex` files across
`structure/`, a local `.sty`, a `.bib`, PNG/PDF figures, 20 packages
including TikZ with libraries, `tcolorbox`, `listings`, `nicematrix`,
`ulem`, and `natbib` bibliography commands.

- Entry: `iclr2027_conference.tex`
- Title: EdiTikZ: Scientific Figure Editing from Revision Trajectories
- Authors: Christian Greisinger, Zhixue Zhao, Steffen Eger
- arXiv: https://arxiv.org/abs/2609.01409
- License: CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/)

Unmodified apart from removing arXiv's packaging manifest, plus two
compatibility shims so older TeX bundles build it too:
- `\PassOptionsToPackage{table}{xcolor}` before the document class pulls in
  xcolor option-less (via eso-pic); otherwise Tectonic reports an option
  clash and misses `\cellcolor`.
- `amssymb` instead of `bbm` (`\mathbb{1}` for `\mathbbm{1}`); bbm ships
  only METAFONT bitmaps, which Tectonic cannot turn into PDF fonts.

Citations stay unresolved without a BibTeX run; the preview still produces pages.
