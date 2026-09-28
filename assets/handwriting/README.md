# Offline handwriting recognition

The model and 411 symbol labels come from [Detypify 0.3.0](https://www.npmjs.com/package/detypify-service/v/0.3.0), by QuarticCat, under the MIT license (LICENSE).
TeX aliases for 390 classes come from [UnicodeIt 0.7.5](https://github.com/svenkreiss/unicodeit), under LPPL 1.3c (UNICODEIT-LICENSE). Some commands require additional LaTeX packages. Classes without a TeX alias insert their Unicode character.

The model runs locally through tract's Rust ONNX runtime, with transformer support disabled. Neither Python, a web service, nor a JavaScript runtime is needed by the app. The model is initialized lazily on a background worker and reused. At most one inference per window runs at a time; stale results are discarded. Input is capped at 4096 points.

The upstream fixed-shape ONNX export contains a constant indexing Loop unsupported by tract. `scripts/prepare-handwriting-model.py` materializes that tensor, simplifies the graph, and compares eight seeded inputs against the original ONNX Runtime output before saving. The initial validation had zero maximum absolute output difference. Learned weights are unchanged.

The canvas follows upstream's 224×224, 10-pixel-padding, 8-pixel-stroke preprocessing, using a native antialiased line rasterizer. Classification is approximate: the user selects a result before text is inserted. Settings selects exactly one recognizer for each drawing. Both recognizers support both Typst and TeX; the document language controls insertion, not model selection. Detexify is the default for its broader symbol coverage.

## Full Detexify samples

Detypify's 411 outputs do not include calligraphic Latin letters. A separate
local point-cloud matcher provides an alternative using every accepted sample from
[Detexify Next](https://github.com/kirel/detexify-next) revision
`ba0742b03b01a7a958110ced23d509a72d85744e` (MIT; `DETEXIFY-LICENSE`):
39,494 samples across 1,123 symbol definitions. The upstream rejected list is
honored. All accepted samples, including single-point marks, are retained.

Reproduce with `scripts/prepare-detexify.py /path/to/pinned-checkout`, using
Python with `unicodeit==0.7.5`. Coordinates are normalized to 32 equally spaced
ink points and stored as 16-bit values in 130-byte records (2-byte class index,
64 little-endian coordinates). The sample asset is 5,134,220 bytes. A coarse
8×8 descriptor selects 64 classes and at most three reference samples each;
symmetric nearest-point distances then rank up to 16 distinct commands.
There is no runtime download or additional dependency. Results are one distance-ranked list of up to 16 commands; calligraphic candidates compete in that same list. No second recognizer or extra calligraphic list is appended.

The separate evaluation fixture contains every tenth accepted sample. Evaluation
removes those records from its reference set; production uses the full set.
The sampled evaluation finds 342/421 expected commands among eight general
suggestions. The former dedicated calligraphic list has been removed so the result order reflects the selected recognizer alone.
This split is not writer-disjoint and does not establish general handwriting
accuracy. The classifier recognizes individual symbols, not whole formulas.

1,099 of the 1,123 definitions have Typst output (436 more than initially
mapped). These include `cal(A)`, `scr(A)`, `bb(h)`, upright Greek, IPA letters
and marks, punctuation, currencies, zodiac signs and mathematical aliases.
TeX/miTeX insertion retains the original LaTeX command. Unicode symbols work in
Typst even when they have no named `sym` alias. Text/IPA characters and ASCII
punctuation may be quoted in the internal mapping metadata; insertion resolves the actual character and never emits those string delimiters.
Multi-character approximations are unavailable for Typst; `\triangle` is △, not Δ.
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
available in TeX and are omitted in Typst. This is a short list
of unresolved conversions, not a limitation of Typst. Tooltips also identify
required LaTeX packages from upstream metadata.

Validation: `cargo test handwriting` includes syntax checks for every mapping
and semantic click/insertion tests for both languages. With Typst installed,
`cargo test every_typst_mapping_compiles -- --ignored` compiles all 1,099 mapped
expressions against the actual math library. Updating mappings does not alter
the sample or holdout binaries, recognition ranking, or background-worker work.

The canvas fills its panel. Logical drawing coordinates preserve the full paper
and proportions across wide/tall resizing. Ink remains four logical pixels wide.
Ink uses the normal text color. Predictions are compact, right-aligned rows painted over the ink with translucent backgrounds. Each row shows a monospace command followed by its glyph on the right; raw Unicode insertion shows only the glyph. Long rows elide on the left to preserve the rightmost glyph; tooltips retain the full insertion. Hover does not reserve scrollbar space or expand the rows. Previous predictions remain while adding strokes and are replaced on completion; Clear flushes them. Clicking inserts that command. Empty drawings do not start recognition; work remains bounded to one
worker per window and 4,096 input points.

## Selection, insertion and bundled size

Settings → Handwriting recognition selects `detexify` (default) or `detypify`.
An engine switch preserves the ink, rejects any old completion and queues one
recognition with the selected engine after the current worker finishes. Repaints
and output-format changes never launch another inference.

| Recognizer | Production assets | Bytes | MiB |
| --- | --- | ---: | ---: |
| Detypify | ONNX model + 411 labels | 4,640,866 | 4.43 |
| Detexify | 39,494 samples + 1,123 labels | 5,269,345 | 5.03 |

These are embedded asset contributions, not differential executable sizes.
Detypify additionally uses tract's linked ONNX runtime; Detexify has no extra
runtime dependency. Both assets stay bundled so users can switch offline.
The separate 533 KiB held-out evaluation file is test-only. The shared verified
Typst name catalogue is about 22 KiB.

Typst insertion defaults to escaped punctuation, then a verified math-mode
`name`, then an unquoted Unicode glyph. Disable **Prefer Typst math-mode names**
for full `#sym.name` references. Style expressions such as `cal(A)`
remain available where a symbol alias does not exist. Disable **Prefer Typst
symbol names** for Unicode; delimiters such as `$` still use `\$`. TeX always
uses the original command when one is available. Layout-only `\\` is excluded.

Regenerate the name catalogue with:

```sh
python3 scripts/prepare-handwriting-symbol-names.py toolchain/bin/typst-aarch64-apple-darwin
```

It enumerates the bundled Typst 0.15.1 `sym` module, checks every exported alias
against the compiler, rejects deprecated names, and keeps verified Detypify
canonical names where available. Runtime insertion never probes the compiler.

The UI now requests up to 16 candidates per recognizer. Detexify still evaluates
the same 64-class shortlist; this increases only the number returned/displayed.
All multi-scalar character approximations are excluded in Typst, including doubled
floors/ceilings and combined negation stand-ins; their TeX commands remain available.
Drawing controls provide Clear and bounded (32 edits) undo/redo history.
