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
