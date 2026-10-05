# Lax-Frobenius diagrammatics fixture

Single-file arXiv paper for manual TeX preview checks: 20 packages
(`amsart`, `newpxtext`/`newpxmath`, `mathalpha`, `tcolorbox`, `thmtools`,
`cleveref`, `microtype`) with 103 `tikzpicture`/`tikzcd` environments and a
`biblatex` bibliography.

- Entry: `LRS_Diag_Frob_Envelope.tex`
- Title: Diagrammatics for lax and Frobenius monoidal functors and weak
  morphism classifiers
- Authors: Alexis Langlois-Rémillard, Mateusz Stroiński
- arXiv: https://arxiv.org/abs/2604.24166
- License: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)

Unmodified apart from removing arXiv's packaging manifest, plus one
compatibility change: biblatex with Biber became plain natbib, because some
TeX bundles cannot run Biber end to end (Tectonic pairs its older biblatex
with a newer system Biber and refuses the control file) and their older
natbib rejects a newer `.bbl`. Only plain `\cite` commands were in use, so
the alpha-style setup is preserved; without a bibliography run citations stay
unresolved while pages still render.
