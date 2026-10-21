"""Native ONNX export/build contracts without model downloads or large inference."""
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


exporter = load("liquid_onnx_export", "export-liquid-onnx.py")
builder = load("liquid_onnx_builder", "build-liquid-onnx.py")
try:
    import onnx
    import numpy as np
except ImportError:
    onnx = np = None


def receipt(root):
    return {"matched": 268, "compared_reference_steps": 427, "mismatches": [],
            "fixture_sha256": exporter.CANONICAL_FIXTURE_SHA256, "lossless_weights": True,
            "model_sha256": exporter.digest(root / "liquid.onnx"),
            "tokenizer_sha256": exporter.digest(root / "tokenizer.json"),
            "max_error_probability_delta": 0,
            "shape_boundaries": [{"sequence": n, "tags_match": True,
                                   "max_error_probability_delta": 0}
                                  for n in [1, 2, 8, 32, 127, 128]]}


class DecodeTests(unittest.TestCase):
    def test_keep_drops_only_bos_and_error_threshold_gates_edits(self):
        ids = [1, 30, 40]
        self.assertEqual(exporter.apply_predictions(ids, [0, 1, 100], [0, .4999, .4999], 100), [30, 40])
        self.assertEqual(exporter.apply_predictions(ids, [0, 1, 100], [0, .5, .5], 100), [98])
        with self.assertRaisesRegex(ValueError, "lengths"):
            exporter.apply_predictions(ids, [0], [0], 100)
        with self.assertRaisesRegex(ValueError, "probability"):
            exporter.apply_predictions([1], [0], [float("nan")], 100)
        with self.assertRaisesRegex(ValueError, "vocabulary"):
            exporter.apply_predictions([1], [202], [1], 100)

    def test_tag_boundaries_replace_append_and_sentence_initial_insertion(self):
        self.assertEqual(exporter.apply_predictions([1, 30, 40], [109, 2, 201], [1, 1, 1], 100), [7, 0, 40, 99])
        self.assertEqual(exporter.apply_predictions([1, 30], [9, 1], [1, 1], 100), [7])

    def test_every_piece_is_processed_including_unicode_bpe_pieces(self):
        self.assertEqual(exporter.apply_predictions([1, 50, 51, 52, 53], [0, 0, 1, 110, 11], [0, 0, 1, 1, 1], 100),
                         [50, 52, 8, 9])


class ExportContracts(unittest.TestCase):
    def test_canonical_fixture_pin_covers_distinct_inputs_and_all_refinement_passes(self):
        cases = exporter.canonical_cases()
        self.assertEqual(len(cases), 268)
        self.assertEqual(len(set(case["text"] for case in cases)), 268)
        self.assertEqual(sum(len(case["reference_passes"]) for case in cases), 427)
        self.assertEqual(builder.CANONICAL_FIXTURE_SHA256, exporter.CANONICAL_FIXTURE_SHA256)

    def test_build_receipts_stay_outside_strict_raw_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            raw = Path(directory) / "raw"
            raw.mkdir()
            self.assertEqual(exporter.evidence_directory(raw), raw.parent / "raw-evidence")
            self.assertEqual(list(raw.iterdir()), [])

    def test_changed_model_partial_coverage_or_nonfinite_evidence_cannot_reuse_parity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "liquid.onnx").write_bytes(b"model")
            (root / "tokenizer.json").write_bytes(b"tokenizer")
            proof = receipt(root)
            exporter.validate_receipt(root, proof)
            for key, value in [("matched", 1), ("lossless_weights", False),
                               ("fixture_sha256", "0" * 64), ("max_error_probability_delta", float("nan"))]:
                invalid = dict(proof, **{key: value})
                with self.assertRaisesRegex(ValueError, "canonical parity"):
                    exporter.validate_receipt(root, invalid)
            (root / "liquid.onnx").write_bytes(b"changed model")
            with self.assertRaisesRegex(ValueError, "exact current"):
                exporter.validate_receipt(root, proof)

    @unittest.skipIf(onnx is None, "optional ONNX export dependencies are not installed")
    def test_initializer_compaction_preserves_bits_and_retains_nonrepresentable_weights(self):
        exact = np.asarray([1.5, -0.0], dtype=np.float32)
        precise = np.asarray([1.0001], dtype=np.float32)
        graph = onnx.helper.make_model(onnx.helper.make_graph([], "weights", [], [], initializer=[
            onnx.numpy_helper.from_array(exact, "exact"), onnx.numpy_helper.from_array(precise, "precise")]))
        report = exporter.compact_initializers(graph)
        self.assertTrue(report["lossless_weights"])
        self.assertEqual(report["initializer_casts"], 1)
        compact, retained = graph.graph.initializer
        restored = onnx.numpy_helper.to_array(compact).astype(np.float32)
        np.testing.assert_array_equal(restored.view(np.uint32), exact.view(np.uint32))
        self.assertEqual(retained.data_type, onnx.TensorProto.FLOAT)
        np.testing.assert_array_equal(onnx.numpy_helper.to_array(retained).view(np.uint32), precise.view(np.uint32))
        self.assertEqual(graph.graph.node[0].op_type, "Cast")


class BuildContracts(unittest.TestCase):
    def test_download_stops_after_pinned_size_plus_one_byte(self):
        class Source(io.BytesIO):
            consumed = 0
            def read(self, count=-1):
                result = super().read(count)
                self.consumed += len(result)
                return result
        source = Source(b"x" * (1024 * 1024))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(builder.urllib.request, "urlopen", return_value=source):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    builder.download(("https://example.invalid/file.tgz", 3, "0" * 64), root, False)
            self.assertEqual(source.consumed, 4)
            self.assertEqual(list(root.iterdir()), [])
            with self.assertRaisesRegex(ValueError, "Missing pinned"):
                builder.download(("https://example.invalid/file.tgz", 3, "0" * 64), root, True)

    def test_archive_traversal_and_external_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, name in enumerate(["../outside", "/absolute", "vendor/external"]):
                archive = root / f"unsafe-{index}.tgz"
                with tarfile.open(archive, "w:gz") as output:
                    info = tarfile.TarInfo(name)
                    if index == 2:
                        info.type = tarfile.SYMTYPE
                        info.linkname = "/outside"
                    output.addfile(info)
                with self.assertRaises((ValueError, tarfile.FilterError)):
                    builder.extract_vendor(archive, root / f"vendor-{index}")
            self.assertFalse((root / "outside").exists())

    def test_normalization_rewrites_versioned_library_id_and_clears_vendor_rpaths(self):
        names = builder.runtime_names()
        settings = builder.DISTRIBUTIONS["x86_64-apple-darwin"]
        original = settings["upstream_install_name"]
        self.assertEqual(builder.normalization_plan("libonnxruntime.dylib", names, original,
            [original, "/usr/lib/libSystem.B.dylib"], ["/vendor/build"], original),
            [("-id", "@rpath/libonnxruntime.dylib"), ("-delete_rpath", "/vendor/build")])
        self.assertEqual(builder.normalization_plan("liquid-onnx", names, None,
            [original], ["@loader_path"], original),
            [("-change", original, "@loader_path/libonnxruntime.dylib")])
        with self.assertRaisesRegex(ValueError, "outside"):
            builder.normalization_plan("liquid-onnx", names, None, ["@rpath/unbundled.dylib"], [])
        self.assertEqual(builder.native_rpaths("cmd LC_RPATH\n cmdsize 32\n path @loader_path (offset 12)\n"), ["@loader_path"])
        self.assertEqual(builder.minimum_os("cmd LC_BUILD_VERSION\n platform 1\n minos 14.0\n sdk 14.5\n"), "14.0")

    def test_notices_include_complete_upstream_license_and_third_party_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "LICENSE").write_text("Full MIT license")
            (root / "ThirdPartyNotices.txt").write_text("Full third-party notice")
            output = root / "notices.txt"
            builder.create_notices("x86_64-apple-darwin", builder.DISTRIBUTIONS["x86_64-apple-darwin"], root, output)
            self.assertIn("Full MIT license", output.read_text())
            self.assertIn("Full third-party notice", output.read_text())

    def test_runtime_closure_and_minimum_os_are_explicit(self):
        self.assertEqual(builder.runtime_names(), ["liquid-onnx", "libonnxruntime.dylib"])
        self.assertEqual(builder.DISTRIBUTIONS["aarch64-apple-darwin"]["minimum_macos"], [14, 0])
        self.assertEqual(builder.DISTRIBUTIONS["x86_64-apple-darwin"]["minimum_macos"], [12, 0])
        for settings in builder.DISTRIBUTIONS.values():
            url, size, sha = settings["archive"]
            self.assertTrue(url.startswith("https://github.com/microsoft/onnxruntime/releases/"))
            self.assertGreater(size, 0)
            self.assertRegex(sha, r"^[0-9a-f]{64}$")

    def test_smoke_requires_correct_target_and_loaded_model_prediction(self):
        ready = {"ready": True, "protocol": 1, "target": "aarch64-apple-darwin", "threads": 4}
        prediction = {"id": 1, "ok": True, "best_tags": [0] * 6, "error_probs": [.1] * 6}
        builder.validate_smoke(ready["target"], json.dumps(ready) + "\n" + json.dumps(prediction))
        with self.assertRaisesRegex(ValueError, "smoke"):
            builder.validate_smoke("x86_64-apple-darwin", json.dumps(ready) + "\n" + json.dumps(prediction))
        prediction["error_probs"][0] = float("nan")
        with self.assertRaisesRegex(ValueError, "smoke"):
            builder.validate_smoke(ready["target"], json.dumps(ready) + "\n" + json.dumps(prediction))


if __name__ == "__main__":
    unittest.main()
