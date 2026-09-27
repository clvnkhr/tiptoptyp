# Offline handwriting recognition

The model and 411 symbol labels come from [Detypify 0.3.0](https://www.npmjs.com/package/detypify-service/v/0.3.0), by QuarticCat, under the MIT license (LICENSE).
TeX aliases for 390 classes come from [UnicodeIt 0.7.5](https://github.com/svenkreiss/unicodeit), under LPPL 1.3c (UNICODEIT-LICENSE). Some commands require additional LaTeX packages. Classes without a TeX alias insert their Unicode character.

The model runs locally through tract's Rust ONNX runtime, with transformer support disabled. Neither Python, a web service, nor a JavaScript runtime is needed by the app. The model is initialized lazily on a background worker and reused. At most one inference per window runs at a time; stale results are discarded. Input is capped at 4096 points.

The upstream fixed-shape ONNX export contains a constant indexing Loop unsupported by tract. `scripts/prepare-handwriting-model.py` materializes that tensor, simplifies the graph, and compares eight seeded inputs against the original ONNX Runtime output before saving. The initial validation had zero maximum absolute output difference. Learned weights are unchanged.

The canvas follows upstream's 224×224, 10-pixel-padding, 8-pixel-stroke preprocessing, using a native antialiased line rasterizer. Classification is approximate: the user selects a result before text is inserted. The neural model is unchanged; the Detexify sample matcher described below runs alongside it.

## Full Detexify samples

Detypify's 411 outputs do not include calligraphic Latin letters. A separate
local point-cloud matcher now supplements them with every accepted sample from
[Detexify Next](https://github.com/kirel/detexify-next) revision
`ba0742b03b01a7a958110ced23d509a72d85744e` (MIT; `DETEXIFY-LICENSE`):
39,494 samples across 1,123 symbol definitions. The upstream rejected list is
honored. All accepted samples, including single-point marks, are retained.

Reproduce with `scripts/prepare-detexify.py /path/to/pinned-checkout`, using
Python with `unicodeit==0.7.5`. Coordinates are normalized to 32 equally spaced
ink points and stored as 16-bit values in 130-byte records (2-byte class index,
64 little-endian coordinates). The sample asset is 5,134,220 bytes. A coarse
8×8 descriptor selects 64 classes and at most three reference samples each;
symmetric nearest-point distances then rank up to eight distinct commands.
There is no runtime download or additional dependency. Four separately ranked
calligraphic suggestions are retained so similar letters and font variants in
the larger dataset do not crowd them out. Predictor groups are shown separately
because their scores are not comparable.

The separate evaluation fixture contains every tenth accepted sample. Evaluation
removes those records from its reference set; production uses the full set.
The sampled evaluation finds 342/421 expected commands among eight general
suggestions and 122/128 calligraphic letters among four dedicated suggestions.
This split is not writer-disjoint and does not establish general handwriting
accuracy. The classifier recognizes individual symbols, not whole formulas.

1,120 of the 1,123 definitions have Typst output (436 more than initially
mapped). These include `cal(A)`, `scr(A)`, `bb(h)`, upright Greek, IPA letters
and marks, punctuation, currencies, zodiac signs and mathematical aliases.
TeX/miTeX insertion retains the original LaTeX command. Unicode symbols work in
Typst even when they have no named `sym` alias. Text/IPA characters and ASCII
punctuation are quoted so they remain upright and cannot become Typst syntax.
Negated relations retain combining negation marks; `\triangle` is △, not Δ.
Glyph shapes can differ between fonts and LaTeX packages; a mapping does not
promise identical font outlines.

`typst-unicode-mappings.json` is the reviewed override table used by the generator.
Its Unicode mappings were checked against Hyperref's PU definitions at revision
[`6b7f43fe77ca122f8aebbfeceb977b0bf3fe04d3`](https://github.com/latex3/hyperref/blob/6b7f43fe77ca122f8aebbfeceb977b0bf3fe04d3/hyperref.dtx)
(LPPL 1.3 or later, `HYPERREF-LICENSE`), with phonetic corrections checked against
[the TIPA manual](https://mirrors.ibiblio.org/pub/mirrors/CTAN/fonts/tipa/tipaman.pdf).
Math style expressions follow [Typst's variants](https://typst.app/docs/reference/math/variants/).
The generator combines this table with UnicodeIt and explicit Typst expressions;
it does not require these websites at generation time or runtime.

Three definitions still have no Typst insertion: `\\textraisevibyi` (a raised
phonetic glyph without a settled text equivalent), `\\texttoneletterstem` (a
font-specific tone component), and `\\ataribox` (the Atari logo). They remain
available in TeX and show “No verified Typst mapping yet.” This is a short list
of unresolved conversions, not a limitation of Typst. Tooltips also identify
required LaTeX packages from upstream metadata.

Validation: `cargo test handwriting` includes syntax checks for every mapping
and semantic click/insertion tests for both languages. With Typst installed,
`cargo test every_typst_mapping_compiles -- --ignored` compiles all 1,120 mapped
expressions against the actual math library. Updating mappings does not alter
the sample or holdout binaries, recognition ranking, or background-worker work.

The canvas fills its panel. Logical drawing coordinates preserve the full paper
and proportions across wide/tall resizing. Ink remains four logical pixels wide.
Predictions are non-selectable text painted last, in normal text color; ink blends
only 25% of that color into the background. Clicking an available prediction still
inserts it. Empty drawings do not start recognition; work remains bounded to one
worker per window and 4,096 input points.
