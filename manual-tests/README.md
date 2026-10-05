# Manual inspection

- `typst-preview.typ` and `tex-preview.tex` are three-page documents for Find, pause/resume,
  compilation, zoom and scrolling. Open their generated PDFs to check that PDF.js
  starts with its outline/sidebar closed.
- `typst-diagnostics.typ` intentionally fails. Comment out its last line to resolve it.
- `tex-diagnostics.tex` intentionally produces an overfull-box warning. Its commented
  undefined command can be enabled to test an error too.

In Settings, search for **Toolbar buttons** and try Text only, Text and icons,
and Icons only. Compile's gear should animate only while a build is active.

Select files in Explorer and check that the file icons stay visible. For drops,
use distinct filenames from a temporary folder outside this repository, drag
them into Explorer or a folder row, and repeat rapidly. A drop on the editor
opens the file instead of importing it. Existing destination files are never
overwritten; the status message explains that conflict. The drop hint should
fit its text. Test both a focused and an unfocused document window.

The TeX examples require the configured Tectonic tool and its standard article
support. Generated PDFs and auxiliary files need not be committed.

## arXiv projects

`arxiv/` holds real arXiv sources for heavier manual TeX checks (many
packages, TikZ figures, multi-file layouts, figures, bibliographies):

- `arxiv/editikz/` — 39-file conference paper, 20 packages, TikZ libraries,
  local `.sty`, natbib bibliography. Entry: `iclr2027_conference.tex`
  (arXiv 2609.01409, CC BY-SA 4.0).
- `arxiv/lax-frobenius/` — single file, 20 packages, 103 TikZ environments,
  natbib bibliography. Entry: `LRS_Diag_Frob_Envelope.tex`
  (arXiv 2604.24166, CC BY 4.0).

Each fixture documents its provenance and any compatibility shims in its own
README. Bibliographies intentionally have no committed `.bbl` and no backend
BibTeX/Biber run, so citations stay unresolved while pages render; that also
exercises the diagnostics panel. `cargo test arxiv_manual_fixtures -- --ignored`
builds every fixture with Tectonic, pdfLaTeX, XeLaTeX and LuaLaTeX through the
real compiler backend and requires toolchains, package downloads and minutes.
