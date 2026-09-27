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

684 definitions have known Typst output. Styled capitals insert commands such
as `\mathcal{A}` in TeX/miTeX or `cal(A)` in Typst math. Other mapped symbols use
their Unicode equivalent. Predictions without a known Typst equivalent remain
visible with a TeX-only tooltip and cannot insert invalid Typst code. Tooltips
identify a required LaTeX package when the upstream definition specifies one.

The canvas fills its panel. Logical drawing coordinates preserve the full paper
and proportions across wide/tall resizing. Ink remains four logical pixels wide.
Predictions are non-selectable text painted last, in normal text color; ink blends
only 25% of that color into the background. Clicking an available prediction still
inserts it. Empty drawings do not start recognition; work remains bounded to one
worker per window and 4,096 input points.
