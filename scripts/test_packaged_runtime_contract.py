"""Pure failure-detection contracts for the packaged runtime journey."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "packaged_runtime", Path(__file__).with_name("test-packaged-runtime.py"))
RUNTIME = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNTIME)


class PackagedCacheContract(unittest.TestCase):
    def test_missing_duplicate_models_and_wrong_permissions_cannot_pass(self):
        target = "aarch64-apple-darwin"
        path = f"{target}/{'a' * 64}/typst"
        state = {path: {"bytes": 1, "mtime_ns": 5, "mode": 0o700}}
        RUNTIME.expect_cache(state, {target: {"typst"}})
        for invalid in [{}, {**state, path.replace("a" * 64, "b" * 64): state[path]},
                        {path: {**state[path], "mode": 0o600}},
                        {f"shared/{'a' * 64}/detypify.onnx":
                         {"bytes": 1, "mtime_ns": 5, "mode": 0o600}}]:
            with self.subTest(state=invalid), self.assertRaises(RuntimeError):
                RUNTIME.expect_cache(invalid, {target: {"typst"}})

    def test_warm_cache_check_rejects_rewrites_and_layout_changes(self):
        before = {"shared/hash/font.ttf": {"bytes": 1, "mtime_ns": 5, "mode": 0o600}}
        RUNTIME.unchanged(before, dict(before))
        for after in [{}, {"shared/hash/font.ttf": {"bytes": 1, "mtime_ns": 6, "mode": 0o600}},
                      {"shared/another/font.ttf": before["shared/hash/font.ttf"]}]:
            with self.subTest(after=after), self.assertRaises(RuntimeError):
                RUNTIME.unchanged(before, after)


if __name__ == "__main__":
    unittest.main()
