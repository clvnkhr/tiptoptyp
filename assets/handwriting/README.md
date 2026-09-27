# Offline handwriting recognition

The model and 411 symbol labels come from [Detypify 0.3.0](https://www.npmjs.com/package/detypify-service/v/0.3.0), by QuarticCat, under the MIT license (LICENSE).
TeX aliases for 390 classes come from [UnicodeIt 0.7.5](https://github.com/svenkreiss/unicodeit), under the MIT license (UNICODEIT-LICENSE). Some commands require additional LaTeX packages. Classes without a TeX alias insert their Unicode character.

The model runs locally through tract's Rust ONNX runtime, with transformer support disabled. Neither Python, a web service, nor a JavaScript runtime is needed by the app. The model is initialized lazily on a background worker and reused. At most one inference per window runs at a time; stale results are discarded. Input is capped at 4096 points.

The upstream fixed-shape ONNX export contains a constant indexing Loop unsupported by tract. `scripts/prepare-handwriting-model.py` materializes that tensor, simplifies the graph, and compares eight seeded inputs against the original ONNX Runtime output before saving. The initial validation had zero maximum absolute output difference. Learned weights are unchanged.

The canvas follows upstream's 224×224, 10-pixel-padding, 8-pixel-stroke preprocessing, using a native antialiased line rasterizer. Classification is approximate: the user selects a result before text is inserted. This model is Detypify's classifier, not Detexify's separate DTW dataset.
