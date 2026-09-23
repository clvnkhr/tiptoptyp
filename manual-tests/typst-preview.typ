#set page(paper: "a5", margin: 16mm)
#set text(size: 12pt)
#set heading(numbering: "1.")

= Preview and controls

Use Find to search for *needle*. Pause updates, edit this sentence, then Resume.
Compile writes a PDF beside this file. Open that PDF to check that PDF.js starts
with its sidebar closed. Toggle the outline manually to check navigation.

#outline()

== Selection and links

Select and copy this sentence. Unicode: café, αβγ, 日本語.
The needle appears on each page.

#link(<last>)[Jump to the final section.]

$ integral_0^1 x^2 dif x = 1 / 3 $

#pagebreak()
= Second page

Zoom, scroll, resize the editor, and search again for needle.

#table(columns: 2, [Action], [Result], [Pause], [Keep the old preview], [Resume], [Update the preview])

#pagebreak()
= Final section <last>

One last needle. Try switching between this document and `tex-preview.tex`.
