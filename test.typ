#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Welcome to tiptoptyp

Edit this document on the left. The PDF preview updates as you type. 

#let accent = rgb("#4f8cff") 
#text(fill: accent, weight: "bold")[A small, fast Typst workspace.] 

== Math and code

Inline math is highlighted too: $ integral_0^infinity e^(-x) dif x = 1 $.sssssssss 

#for item in ("Native UI", "Live PDF", "Fast rebuilds") [
  - #item
]

```tex
\alpha + \beta^{1+2\alpha} \frac{1}{a} 
\begin{equation}
sss
\end{equation}
```


























