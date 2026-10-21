#!/usr/bin/env python3
"""Export and verify Liquid for the Python-free native ONNX Runtime helper.

Optional developer setup (Apple Silicon, Python 3.12):
  python3.12 -m venv /private/tmp/liquid-onnx-build
  /private/tmp/liquid-onnx-build/bin/python -m pip install -r scripts/liquid-onnx-requirements.txt
  /private/tmp/liquid-onnx-build/bin/python scripts/liquid-grammar-worker.py --cache-dir /private/tmp/liquid-reference --install

Export, verify, build both thin native helpers, then prepare the app payload:
  /private/tmp/liquid-onnx-build/bin/python scripts/export-liquid-onnx.py --cache-dir /private/tmp/liquid-reference --output-dir /private/tmp/liquid-onnx-raw --export
  /private/tmp/liquid-onnx-build/bin/python scripts/export-liquid-onnx.py --cache-dir /private/tmp/liquid-reference --output-dir /private/tmp/liquid-onnx-raw --verify
  python3 scripts/build-liquid-onnx.py --source /private/tmp/liquid-onnx-raw --work-dir /private/tmp/liquid-onnx-vendor
  cargo xtask prepare-liquid --source /private/tmp/liquid-onnx-raw
  cargo xtask stage-liquid

Python is a BUILD-TIME reference/export tool only. End users install no Python or
model runtime: the app automatically unpacks its bundled model/tokenizer and
matching native helper on first use. The optional reranker is not included.
Reference installation and the native builder explicitly download pinned files;
export and verification are offline. Build-only receipts stay in the sibling
OUTPUT-evidence folder, outside the strict ten-file raw payload. Lossless FP16
weight storage is cast to FP32 CPU arithmetic; there is no lossy quantization.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import importlib.metadata
import importlib.util
import json
import math
from pathlib import Path
import platform
import shutil
import statistics
import sys
import time

HERE = Path(__file__).resolve().parent
LIMIT = 128
CANONICAL_FIXTURE_SHA256 = "3567a30d66d213ad47a3fbd7cbedad4974ea9b6e19037326da82ee5050159dc2"
LICENSE_SHA256 = "4d28ca14dedc0b3d0fcc2b3339f0e79931faa33874f3d24f522183a8fc70068c"


class InputTooLong(ValueError):
    """Only explicit bounded-input rejections may be excluded from parity."""

BOUNDARIES = [
    ("native-agreement", "She go to school every day."),
    ("native-repeated", "This is is the result."),
    ("native-control", "The nonautonomous semigroup is well-posed."),
    ("native-apostrophe", "Tao's argument applies to Barbato and Romito."),
    ("native-curly-apostrophe", "Tao’s argument hold for this operator."),
    ("native-unicode", "These café estimates holds for α and β."),
    ("native-quotes", 'He say, "This is a result."'),
    ("native-hyphens", "A ten-year-old boy go to school."),
    ("native-multiline", "These estimates\n\tare uniform."),
    ("native-parentheses", "The estimates (which is uniform) hold."),
    ("native-math-placeholder", "These estimates x and x is uniform."),
    ("native-short", "I"),
]


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def module_from_file(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def load_reference(cache, threads):
    worker = module_from_file("_liquid_export_worker", HERE / "liquid-grammar-worker.py")
    return worker, worker.load_adapter(cache, threads).adapter


def make_graph_wrapper(model):
    import torch

    class Forward(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.model = model

        def forward(self, input_ids, attention_mask):
            result = self.model(input_ids, attention_mask)
            return (result["label_logits"].argmax(-1),
                    result["detect_logits"].softmax(-1)[..., 1])

    return Forward().eval()


def compact_initializers(graph):
    """Use FP16 storage only when the FP32 round trip is bit-exact.

    Published trainable weights are FP16. Their arithmetic remains FP32, exactly
    as in our reference adapter. Generated FP32 constants which cannot make an
    exact round trip retain FP32 storage. This is not quantized inference.
    """
    import numpy as np
    import onnx
    casts, saved = [], 0
    for initializer in graph.graph.initializer:
        if initializer.data_type != onnx.TensorProto.FLOAT:
            continue
        original = onnx.numpy_helper.to_array(initializer)
        compact = original.astype(np.float16)
        if not np.array_equal(original.view(np.uint32), compact.astype(np.float32).view(np.uint32)):
            continue
        name = initializer.name
        initializer.CopyFrom(onnx.numpy_helper.from_array(compact, name + "_storage_fp16"))
        casts.append(onnx.helper.make_node("Cast", [initializer.name], [name],
                                           to=onnx.TensorProto.FLOAT))
        saved += original.nbytes - compact.nbytes
    nodes = list(graph.graph.node)
    del graph.graph.node[:]
    graph.graph.node.extend(casts + nodes)
    return {"initializer_casts": len(casts), "saved_bytes": saved,
            "arithmetic_dtype": "float32", "lossy_quantization": False, "lossless_weights": True}


def export(reference, worker, cache, output):
    import torch
    import onnx
    if (output / "liquid.onnx").exists():
        raise ValueError("Choose a fresh export folder; the destination already contains a model")
    output.mkdir(parents=True, exist_ok=True)
    temporary = output / "export-fp32.onnx"
    # The legacy exporter does not lower enable_gqa=True SDPA. Eager attention
    # expands it to standard matmul/softmax operators. Parity below always
    # compares with the original SDPA reference, not this export implementation.
    original_attention = reference.model.encoder.config._attn_implementation
    reference.model.encoder.set_attn_implementation("eager")
    pieces = [1, *reference.tokenizer.encode("She go to school every day .",
                                            add_special_tokens=False)]
    ids = torch.tensor([pieces], dtype=torch.int64)
    try:
        with torch.inference_mode():
            torch.onnx.export(
                make_graph_wrapper(reference.model), (ids, torch.ones_like(ids)),
                str(temporary), input_names=["input_ids", "attention_mask"],
                output_names=["best_tags", "error_probs"],
                dynamic_axes={name: {1: "sequence"} for name in
                              ["input_ids", "attention_mask", "best_tags", "error_probs"]},
                opset_version=18, dynamo=False, do_constant_folding=False,
                external_data=False,
            )
    finally:
        reference.model.encoder.set_attn_implementation(original_attention)
    graph = onnx.load(temporary)
    compact = compact_initializers(graph)
    onnx.helper.set_model_props(graph, {
        "source_repository": worker.MODEL_REPO,
        "upstream_revision": worker.MODEL_REVISION,
        "license": "LFM Open License v1.0",
        "modification_notice": "Modified for tiptoptyp: exported to portable ONNX; eager grouped-query attention, per-token argmax and detection probability outputs; exact FP16 storage casts to FP32 arithmetic; no quantization or weight retraining.",
    })
    target = output / "liquid.onnx"
    onnx.save(graph, target)
    onnx.checker.check_model(str(target))
    temporary.unlink()
    tokenizer = worker.artifact_path(cache, worker.ARTIFACTS[4])
    shutil.copyfile(tokenizer, output / "tokenizer.json")
    license_file = HERE.parent / "assets/liquid/LFM-Open-License.txt"
    if digest(license_file) != LICENSE_SHA256:
        raise ValueError("Pinned upstream model license is missing or changed")
    shutil.copyfile(license_file, output / "LICENSE.txt")
    info = {"format": 3, "model_repo": worker.MODEL_REPO,
            "model_revision": worker.MODEL_REVISION,
            "encoder_revision": worker.ENCODER_REVISION,
            "opset": 18, "ir_version": graph.ir_version,
            "inputs": {"input_ids": "int64[1,sequence]",
                       "attention_mask": "int64[1,sequence]"},
            "outputs": {"best_tags": "int64[1,sequence]",
                        "error_probs": "float32[1,sequence]"},
            "minimum_sequence": 1, "maximum_sequence": LIMIT,
            "vocab_size": reference.model.vocab_size, "bos_id": 1, "pad_id": 0,
            "use_swap": False, "replace_base": 2, "keep_confidence": 0.0,
            "min_error_prob": 0.5, "max_iter": 3,
            "storage": compact,
            "operators": dict(sorted(Counter(node.op_type for node in graph.graph.node).items())),
            "files": {name: {"bytes": (output / name).stat().st_size,
                             "sha256": digest(output / name)}
                      for name in ["liquid.onnx", "tokenizer.json"]},
            "source_artifacts": [{"repo": x[0], "revision": x[1], "name": x[2],
                                  "bytes": x[3], "sha256": x[4]}
                                 for x in worker.ARTIFACTS],
            "versions": versions()}
    write_json(evidence_directory(output) / "export.json", info)
    return info


def versions():
    return {name: importlib.metadata.version(name) for name in
            ["torch", "transformers", "tokenizers", "sacremoses", "onnx", "onnxruntime"]}


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def session(path, threads):
    import onnxruntime as ort
    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    return ort.InferenceSession(str(path), sess_options=options,
                               providers=["CPUExecutionProvider"])


def model_step(reference, ids):
    import torch
    with torch.inference_mode():
        result = reference.model(torch.tensor([ids]), torch.ones((1, len(ids)), dtype=torch.int64))
    return (result["label_logits"].argmax(-1).numpy()[0],
            result["detect_logits"].softmax(-1)[..., 1].numpy()[0])


def onnx_step(runtime, ids):
    import numpy as np
    tags, errors = runtime.run(None, {"input_ids": np.array([ids], dtype=np.int64),
                                     "attention_mask": np.ones((1, len(ids)), dtype=np.int64)})
    return tags[0], errors[0]


def apply_predictions(ids, tags, errors, vocab_size):
    if not (len(ids) == len(tags) == len(errors)):
        raise ValueError("Prediction lengths do not match input")
    if any(not math.isfinite(float(value)) or not 0 <= float(value) <= 1 for value in errors):
        raise ValueError("Prediction probability is invalid")
    if any(not 0 <= int(tag) < 2 + 2 * vocab_size for tag in tags):
        raise ValueError("Prediction tag is outside the pinned edit vocabulary")
    result = []
    for index, (piece, tag, error) in enumerate(zip(ids, tags, errors)):
        tag = int(tag) if int(tag) != 0 and float(error) >= 0.5 else 0
        if tag == 0:
            if index:
                result.append(piece)
        elif tag == 1:
            continue
        elif tag < 2 + vocab_size:
            result.append(tag - 2)
        else:
            if index:
                result.append(piece)
            result.append(tag - 2 - vocab_size)
    return result


def correct(reference, text, step, traces=None):
    prepared = " ".join(reference._tokens(text))
    seen = {prepared}
    result = prepared
    for _ in range(3):
        ids = [1, *reference.tokenizer.encode(prepared, add_special_tokens=False)]
        if len(ids) > LIMIT:
            raise InputTooLong(f"Input has {len(ids)} model tokens, exceeds {LIMIT}")
        tags, errors = step(ids)
        pieces = apply_predictions(ids, tags, errors, reference.model.vocab_size)
        result = reference.tokenizer.decode(pieces, skip_special_tokens=True).strip()
        if traces is not None:
            traces.append({"prepared": prepared, "input_ids": ids,
                           "best_tags": [int(x) for x in tags],
                           "error_probs": [float(x) for x in errors],
                           "output_ids": pieces, "decoded": result})
        if result == prepared or result in seen:
            break
        seen.add(result)
        prepared = result
    return reference._untokenize(result, text)


def canonical_cases():
    path = HERE.parent / "assets/liquid/correction-cases.json"
    if digest(path) != CANONICAL_FIXTURE_SHA256:
        raise ValueError("The maintained canonical fixture changed; review its export pin")
    cases = json.loads(path.read_text())["cases"]
    if len(cases) != 268 or sum(len(case["reference_passes"]) for case in cases) != 427:
        raise ValueError("The canonical fixture must cover all 268 inputs/427 passes")
    return cases


def evidence_directory(output):
    directory = output.parent / (output.name + "-evidence")
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def validate_receipt(output, receipt):
    if (receipt.get("matched") != 268 or receipt.get("compared_reference_steps") != 427
            or receipt.get("fixture_sha256") != CANONICAL_FIXTURE_SHA256
            or receipt.get("mismatches") != [] or receipt.get("lossless_weights") is not True
            or receipt.get("model_sha256") != digest(output / "liquid.onnx")
            or receipt.get("tokenizer_sha256") != digest(output / "tokenizer.json")
            or not 0 <= receipt.get("max_error_probability_delta", 1) <= 1e-4
            or [row.get("sequence") for row in receipt.get("shape_boundaries", [])] != [1, 2, 8, 32, 127, 128]
            or not all(row.get("tags_match") is True and 0 <= row.get("max_error_probability_delta", 1) <= 1e-4
                       for row in receipt["shape_boundaries"])):
        raise ValueError("Passing canonical parity for the exact current model/tokenizer is required")


def verify(reference, runtime, output):
    import numpy as np
    frozen = canonical_cases()
    cases = [(case["id"], case["text"]) for case in frozen]
    expected_outputs = {case["id"]: case["reference_corrected"] for case in frozen}
    metadata = [{"path": "assets/liquid/correction-cases.json", "sha256": CANONICAL_FIXTURE_SHA256, "cases": len(cases)}]
    export_info = json.loads((evidence_directory(output) / "export.json").read_text())
    if any(export_info["files"][name]["sha256"] != digest(output / name) for name in ["liquid.onnx", "tokenizer.json"]):
        raise ValueError("The exported model/tokenizer changed before verification")
    results, mismatches, max_error_delta, total_steps = [], [], 0.0, 0
    for index, (identifier, text) in enumerate(cases):
        trace, ort_trace = [], []
        try:
            reference_output = correct(reference, text, lambda ids: model_step(reference, ids), trace)
            actual_reference = reference.correct(text)
            if actual_reference != reference_output or reference_output != expected_outputs[identifier]:
                raise AssertionError(f"Reference decode port disagrees for {identifier}")
            onnx_output = correct(reference, text, lambda ids: onnx_step(runtime, ids), ort_trace)
        except InputTooLong as error:
            results.append({"id": identifier, "text": text, "status": "input_too_long", "reason": str(error)})
            continue
        all_tags, all_applied = True, True
        for step in trace:
            ids = step["input_ids"]
            tags, errors = onnx_step(runtime, ids)
            all_tags &= np.array_equal(tags, step["best_tags"])
            all_applied &= apply_predictions(ids, tags, errors, reference.model.vocab_size) == step["output_ids"]
            delta = float(np.max(np.abs(errors - np.asarray(step["error_probs"]))))
            max_error_delta = max(max_error_delta, delta)
            total_steps += 1
        passed = all_tags and all_applied and reference_output == onnx_output
        record = {"id": identifier, "text": text, "status": "match" if passed else "mismatch",
                  "moses_tokens": reference._tokens(text), "reference_corrected": reference_output,
                  "onnx_corrected": onnx_output, "all_best_tags_match": bool(all_tags),
                  "all_applied_actions_match": bool(all_applied), "reference_passes": trace}
        results.append(record)
        if not passed:
            mismatches.append(identifier)
        if (index + 1) % 20 == 0:
            print(f"Validated {index + 1}/{len(cases)} inputs", flush=True)
    # Sequence boundaries exercise the dynamic graph independently of prose.
    shapes = []
    for length in [1, 2, 8, 32, 127, 128]:
        ids = [1] * length
        expected_tags, expected_errors = model_step(reference, ids)
        tags, errors = onnx_step(runtime, ids)
        shapes.append({"sequence": length, "tags_match": bool(np.array_equal(tags, expected_tags)),
                       "max_error_probability_delta": float(np.max(np.abs(errors - expected_errors)))})
    result = {"format": 3, "versions": versions(), "corpora": metadata,
              "fixture_sha256": CANONICAL_FIXTURE_SHA256, "lossless_weights": export_info["storage"]["lossless_weights"],
              "model_sha256": digest(output / "liquid.onnx"),
              "tokenizer_sha256": digest(output / "tokenizer.json"),
              "cases": len(cases), "matched": sum(x["status"] == "match" for x in results),
              "too_long": sum(x["status"] == "input_too_long" for x in results),
              "mismatches": mismatches, "compared_reference_steps": total_steps,
              "max_error_probability_delta": max_error_delta, "shape_boundaries": shapes,
              "results": results}
    write_json(evidence_directory(output) / "parity.json", result)
    # Full per-pass IDs/tags and final strings also serve as native-port goldens.
    write_json(evidence_directory(output) / "goldens.json", {"schema_version": 1, "cases": results})
    if mismatches or max_error_delta > 1e-4 or not all(x["tags_match"] and x["max_error_probability_delta"] <= 1e-4 for x in shapes):
        raise RuntimeError("ONNX parity failed; see parity.json")
    validate_receipt(output, result)
    provenance = {"format": 3, "source": "https://huggingface.co/" + export_info["model_repo"],
                  "revision": export_info["model_revision"], "encoder_revision": export_info["encoder_revision"],
                  "model_sha256": result["model_sha256"], "tokenizer_sha256": result["tokenizer_sha256"],
                  "change": "Portable ONNX with lossless FP16 storage and explicit FP32 casts; native ONNX Runtime CPU helper; no Python or reranker.",
                  "parity": {key: value for key, value in result.items() if key != "results"},
                  "export": export_info, "runtimes": []}
    write_json(output / "PROVENANCE.json", provenance)
    print(f"Parity: {result['matched']} matched, {result['too_long']} rejected-too-long, "
          f"{len(mismatches)} mismatches; max error delta {max_error_delta:.3g}", flush=True)
    return result


def benchmark(reference, runtime, output, threads, warmup, repeats):
    texts = [BOUNDARIES[0][1], BOUNDARIES[1][1], BOUNDARIES[2][1],
             "These estimates " + "are uniform and bounded " * 8 + "for all parameters."]
    results = []
    for text in texts:
        functions = {"pytorch": reference.correct,
                     "onnx": lambda text: correct(reference, text, lambda ids: onnx_step(runtime, ids))}
        for function in functions.values():
            for _ in range(warmup):
                function(text)
        times = {name: [] for name in functions}
        outputs = {}
        # Interleaved ABBA order reduces drift without overlapping any inference.
        for repeat in range(repeats):
            order = ["pytorch", "onnx"] if repeat % 2 == 0 else ["onnx", "pytorch"]
            for name in order:
                started = time.perf_counter_ns()
                outputs[name] = functions[name](text)
                times[name].append((time.perf_counter_ns() - started) / 1e6)
        if outputs["pytorch"] != outputs["onnx"]:
            raise AssertionError("Benchmark corrections differ")
        results.append({"text": text, "tokens": len(reference.tokenizer.encode(" ".join(reference._tokens(text)), add_special_tokens=False)) + 1,
                        "corrected": outputs["onnx"], "times_ms": times,
                        "median_ms": {name: statistics.median(values) for name, values in times.items()}})
    receipt = {"schema_version": 1, "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
               "platform": platform.platform(), "machine": platform.machine(),
               "processor": platform.processor(), "python": platform.python_version(),
               "versions": versions(), "threads": threads, "inter_op_threads": 1,
               "device": "CPU", "arithmetic_dtype": "float32", "batch_size": 1,
               "warmup_per_case_runtime": warmup, "repeats": repeats,
               "order": "interleaved AB/BA", "scope": "warm correction including identical Moses/BPE/iterative-decode, excludes source projection and GUI",
               "results": results}
    write_json(output / "benchmark.json", receipt)
    print(json.dumps({"median_ms": [x["median_ms"] for x in results]}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=4, choices=range(1, 9))
    parser.add_argument("--export", action="store_true")
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--repeats", type=int, default=7)
    args = parser.parse_args()
    if not (args.export or args.verify or args.benchmark):
        parser.error("choose --export, --verify and/or --benchmark")
    if args.repeats < 1 or args.warmup < 0:
        parser.error("repeats must be positive and warmup cannot be negative")
    worker, reference = load_reference(args.cache_dir, args.threads)
    if args.export:
        export(reference, worker, args.cache_dir, args.output_dir)
    if args.verify or args.benchmark:
        runtime = session(args.output_dir / "liquid.onnx", args.threads)
        if args.verify:
            verify(reference, runtime, args.output_dir)
        if args.benchmark:
            benchmark(reference, runtime, evidence_directory(args.output_dir), args.threads, args.warmup, args.repeats)


if __name__ == "__main__":
    main()
