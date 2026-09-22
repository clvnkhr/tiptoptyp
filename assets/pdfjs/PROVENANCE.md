# PDF.js

Mozilla PDF.js **6.3.289**, legacy generic viewer build (includes polyfills
for system web views). Apache-2.0; see LICENSE and the notices in each asset,
including the separate font, CMap and WASM licenses.

Source: https://github.com/mozilla/pdf.js/releases/tag/v6.3.289
Archive: https://github.com/mozilla/pdf.js/releases/download/v6.3.289/pdfjs-6.3.289-legacy-dist.zip
SHA-256: `51683fac4aff7dd31ed91e9ab735a2098a78d50899d1ec529aed6dc8aa19400d`

Extracted excluding `*.map` and the example
`web/compressed.tracemonkey-pldi-09.pdf`. The app embeds these assets in its
binary and serves them on a private loopback endpoint; no CDN is used.
Our integration lives in `src/pdfjs/`, outside these upstream assets.

Local patch in `web/viewer.mjs`, `PDFLinkService.goToDestination`: bind deferred
destination focus to the PDF generation, and use `preventScroll` only while the
destination remains current and focus is inside the viewer. Upstream 6.3.289
leaves a text-layer callback alive across document replacement; following a link,
scrolling elsewhere and recompiling could focus page one and reset the scroll.
`scripts/check-pdfjs.mjs` reproduces the failure against the unpatched viewer.
