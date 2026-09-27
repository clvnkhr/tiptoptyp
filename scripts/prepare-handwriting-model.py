#!/usr/bin/env python3
"""Rebuild the pinned offline model (development only).

uv run --python 3.12 --with onnx==1.23.0 --with onnxsim==0.7.3 \
  --with onnxruntime==1.30.0 --with unicodeit==0.7.5 \
  scripts/prepare-handwriting-model.py
"""
import io
import json
from pathlib import Path
import tarfile
import urllib.request
import numpy as np
import onnx
import onnxruntime as ort
import onnxsim
from unicodeit.data import REPLACEMENTS

root = Path(__file__).resolve().parents[1] / "assets/handwriting"
url = "https://registry.npmjs.org/detypify-service/-/detypify-service-0.3.0.tgz"
archive = tarfile.open(fileobj=io.BytesIO(urllib.request.urlopen(url).read()))
model = onnx.load_model_from_string(archive.extractfile("package/train/model.onnx").read())
original = ort.InferenceSession(model.SerializeToString(), providers=["CPUExecutionProvider"])
model, valid = onnxsim.simplify(model)
assert valid
# The fixed-shape export has a constant as_strided indexing loop. Materialize
# its index tensor; no learned weights or numerical operators are changed.
model.graph.output.append(onnx.helper.make_tensor_value_info("indices_16", onnx.TensorProto.INT64, None))
probe = ort.InferenceSession(model.SerializeToString(), providers=["CPUExecutionProvider"])
indices = probe.run(["indices_16"], {"x": np.zeros((1, 1, 224, 224), dtype=np.float32)})[0]
del model.graph.output[-1]
nodes = [node for node in model.graph.node if node.op_type not in ("Loop", "SequenceEmpty")]
del model.graph.node[:]
model.graph.node.extend(nodes)
model.graph.initializer.append(onnx.numpy_helper.from_array(indices, "indices_16"))
model, valid = onnxsim.simplify(model)
assert valid
optimized = ort.InferenceSession(model.SerializeToString(), providers=["CPUExecutionProvider"])
rng = np.random.default_rng(42)
for _ in range(8):
    pixels = rng.random((1, 1, 224, 224), dtype=np.float32)
    np.testing.assert_allclose(original.run(None, {"x": pixels})[0], optimized.run(None, {"x": pixels})[0], rtol=1e-5, atol=1e-5)
root.mkdir(exist_ok=True)
onnx.save(model, root / "detypify.onnx")
symbols = json.load(archive.extractfile("package/train/infer.json"))
for symbol in symbols:
    candidates = [tex for tex, char in REPLACEMENTS if char == symbol["char"] and tex.startswith("\\") and "{" not in tex]
    if candidates:
        symbol["tex"] = min(candidates, key=lambda text: (len(text), text))
(root / "symbols.json").write_text(json.dumps(symbols, ensure_ascii=False, separators=(",", ":")) + "\n")
print(f"Validated model; {len(symbols)} symbol classes")
