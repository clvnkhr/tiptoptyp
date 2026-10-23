"""Protocol and timeout checks for the developer comparison, without model assets."""
import importlib.util
import json
import hashlib
from pathlib import Path
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("native_comparison", Path(__file__).with_name("compare-liquid-native.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProtocolTests(unittest.TestCase):
    def worker(self, directory, source):
        return MODULE.Worker([sys.executable, "-u", "-c", source], Path(directory) / "worker.stderr")

    def test_partial_line_is_subject_to_the_same_request_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            worker = self.worker(directory,
                "import pathlib,sys,time;sys.stdout.write('{');sys.stdout.flush();"
                f"pathlib.Path({str(ready)!r}).write_text('ready');time.sleep(5)")
            try:
                deadline = time.monotonic() + 5
                while not ready.exists():
                    if time.monotonic() >= deadline:
                        self.fail("test worker did not start")
                    time.sleep(0.01)
                with self.assertRaises(TimeoutError):
                    worker.request("text", timeout=0.05)
            finally:
                worker.abort()

    def test_chunked_unicode_response_is_read_completely(self):
        with tempfile.TemporaryDirectory() as directory:
            worker = self.worker(directory, """import json,sys,time
request=json.loads(sys.stdin.readline())
response=(json.dumps(dict(id=request['id'],ok=True,corrected='Tao’s result.'),ensure_ascii=False)+'\\n').encode()
cut=response.index('’'.encode())+1
sys.stdout.buffer.write(response[:cut]);sys.stdout.flush();time.sleep(.02)
sys.stdout.buffer.write(response[cut:]);sys.stdout.flush()
""".replace("+'\\\\n'", "+'\\n'"))
            try:
                actual, elapsed = worker.request("Tao’s result.")
                self.assertEqual(actual, "Tao’s result.")
                self.assertGreater(elapsed, 0)
                worker.close()
            except BaseException:
                worker.abort()
                raise

    def test_oversized_response_and_wrong_identity_are_rejected(self):
        for source in [
            "import sys;sys.stdin.readline();sys.stdout.write('x'*65537+'\\n');sys.stdout.flush()",
            "import json,sys;sys.stdin.readline();print(json.dumps(dict(id=99,ok=True,corrected='text')))",
        ]:
            with tempfile.TemporaryDirectory() as directory:
                worker = self.worker(directory, source)
                try:
                    with self.assertRaises(ValueError):
                        worker.request("text")
                finally:
                    worker.abort()


class PreparedCacheTests(unittest.TestCase):
    target = "aarch64-apple-darwin"
    other_target = "x86_64-apple-darwin"

    def prepare(self, directory):
        directory = Path(directory)
        catalogs = directory / "catalogs"
        catalogs.mkdir()
        cache = directory / "cache"
        root = cache / "onnxruntime" / MODULE.MODEL_ID
        (root / "runtime" / self.target).mkdir(parents=True)
        (root / "licenses").mkdir()
        def asset(relative, **fields):
            content = relative.encode()
            (root / relative).write_bytes(content)
            if relative.endswith("/liquid-onnx"):
                (root / relative).chmod(0o700)
            return dict(bytes=len(content), sha256=hashlib.sha256(content).hexdigest(), **fields)
        entries = [asset(name, name=name) for name in ("liquid.onnx", "tokenizer.json")]
        for name in ("liquid-onnx", "libonnxruntime.dylib"):
            entries.append(asset(f"runtime/{self.target}/{name}", name=name, target=self.target))
            # The trusted universal catalog lists both, but only this process's
            # architecture belongs in its prepared cache.
            entries.append(dict(name=name, target=self.other_target, bytes=1, sha256="a" * 64))
        catalog = dict(format=3, model_id=MODULE.MODEL_ID, upstream_revision="fixture", files=entries)
        notices = dict(format=1, files=[
            asset(name, name=name, asset_name=name) for name in ("LICENSE.txt", "PROVENANCE.json")])
        notices["files"].append(asset(f"licenses/{self.target}.txt",
            name=f"licenses/{self.target}.txt", asset_name=f"{self.target}-LICENSES.txt", target=self.target))
        notices["files"].append(dict(name=f"licenses/{self.other_target}.txt", target=self.other_target,
            bytes=1, sha256="a" * 64, asset_name=f"{self.other_target}-LICENSES.txt"))
        (catalogs / "manifest.json").write_text(json.dumps(catalog))
        (root / "manifest.json").write_text(json.dumps(catalog, indent=2))
        (catalogs / "notices.json").write_text(json.dumps(notices))
        return cache, catalogs, root

    def check(self, cache, catalogs):
        return MODULE.verify_prepared_cache(cache, catalog_root=catalogs, target=self.target)

    def test_current_architecture_and_all_notices_are_verified_without_other_architecture(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, root = self.prepare(directory)
            report = self.check(cache, catalogs)
            self.assertEqual(report["target"], self.target)
            self.assertEqual(len(report["asset_sha256"]), 4)
            self.assertEqual(len(report["notice_sha256"]), 3)
            self.assertFalse((root / "runtime" / self.other_target).exists())
            self.assertEqual(report["catalog_sha256"], MODULE.sha(catalogs / "manifest.json"))

    def test_missing_corrupt_or_symlink_assets_require_setup_before_comparison(self):
        for relative in ("liquid.onnx", "tokenizer.json", f"runtime/{self.target}/liquid-onnx",
                         f"runtime/{self.target}/libonnxruntime.dylib", "LICENSE.txt",
                         "PROVENANCE.json", f"licenses/{self.target}.txt"):
            for damage in ("missing", "same_length_corrupt", "symlink"):
                with self.subTest(relative=relative, damage=damage), tempfile.TemporaryDirectory() as directory:
                    cache, catalogs, root = self.prepare(directory)
                    path = root / relative
                    original = path.read_bytes()
                    path.unlink()
                    if damage == "same_length_corrupt":
                        path.write_bytes(b"x" * len(original))
                    elif damage == "symlink":
                        destination = Path(directory) / "outside"
                        destination.write_bytes(original)
                        path.symlink_to(destination)
                    with self.assertRaisesRegex(ValueError, "setup is excluded from timing"):
                        self.check(cache, catalogs)

    def test_installed_catalog_must_match_current_pinned_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, root = self.prepare(directory)
            installed = root / "manifest.json"
            data = json.loads(installed.read_text())
            data["files"][0]["sha256"] = "a" * 64
            installed.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "differs from the current download catalog"):
                self.check(cache, catalogs)

    def test_helper_must_already_be_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, root = self.prepare(directory)
            (root / "runtime" / self.target / "liquid-onnx").chmod(0o600)
            with self.assertRaisesRegex(ValueError, "not executable"):
                self.check(cache, catalogs)

    def test_symlink_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, root = self.prepare(directory)
            original = root / "licenses"
            moved = root / "moved-licenses"
            original.rename(moved)
            original.symlink_to(moved)
            with self.assertRaisesRegex(ValueError, "regular directory"):
                self.check(cache, catalogs)

    def test_wrong_architecture_is_not_prepared(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, _ = self.prepare(directory)
            with self.assertRaisesRegex(ValueError, "not prepared"):
                MODULE.verify_prepared_cache(cache, catalog_root=catalogs, target=self.other_target)
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            MODULE.verify_prepared_cache(Path("/unprepared"), target="unexpected")

    def test_preflight_failure_prevents_timed_workers_for_verify_and_benchmark(self):
        args = SimpleNamespace(native_cache=Path("/unprepared"), python=Path("/python"),
                               python_cache=Path("/python-cache"))
        for run in (MODULE.verify, MODULE.benchmark):
            with patch.object(MODULE, "verified_native_inputs", side_effect=ValueError("unprepared")), \
                 patch.object(MODULE, "Worker") as worker:
                with self.assertRaisesRegex(ValueError, "unprepared"):
                    run(args)
                worker.assert_not_called()

    def test_compiled_probe_metadata_selects_actual_target_even_when_driver_differs(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, _ = self.prepare(directory)
            metadata = dict(schema_version=1, target=self.target,
                catalog_sha256=MODULE.sha(catalogs / "manifest.json"),
                notices_catalog_sha256=MODULE.sha(catalogs / "notices.json"))
            args = SimpleNamespace(native_probe=Path("/probe"), native_cache=cache)
            with patch.object(MODULE, "CATALOG_ROOT", catalogs), \
                 patch.object(MODULE.platform, "machine", return_value="x86_64"), \
                 patch.object(MODULE.subprocess, "check_output", return_value=json.dumps(metadata)) as command:
                actual = MODULE.verified_native_inputs(args)
            self.assertEqual(actual["target"], self.target)
            self.assertEqual(actual["probe_metadata"], metadata)
            command.assert_called_once_with([Path("/probe"), "--metadata"], text=True, timeout=10)

    def test_stale_or_invalid_compiled_catalog_metadata_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, catalogs, _ = self.prepare(directory)
            metadata = dict(schema_version=1, target=self.target,
                catalog_sha256=MODULE.sha(catalogs / "manifest.json"),
                notices_catalog_sha256=MODULE.sha(catalogs / "notices.json"))
            args = SimpleNamespace(native_probe=Path("/probe"), native_cache=cache)
            for field in ("catalog_sha256", "notices_catalog_sha256"):
                stale = {**metadata, field: "a" * 64}
                with patch.object(MODULE, "CATALOG_ROOT", catalogs), \
                     patch.object(MODULE.subprocess, "check_output", return_value=json.dumps(stale)):
                    with self.assertRaisesRegex(ValueError, "stale compiled"):
                        MODULE.verified_native_inputs(args)
            for invalid in ([], {"schema_version": 0}, {"schema_version": 1}):
                with patch.object(MODULE.subprocess, "check_output", return_value=json.dumps(invalid)):
                    with self.assertRaisesRegex(ValueError, "unsupported metadata|Unsupported native Liquid target"):
                        MODULE.verified_native_inputs(args)

    def test_probe_command_only_uses_current_cache_and_threads(self):
        args = SimpleNamespace(native_probe=Path("/probe"), native_cache=Path("/cache"), threads=3)
        self.assertEqual(MODULE.native_command(args),
                         [Path("/probe"), "--cache-dir", Path("/cache"), "--threads", "3"])


if __name__ == '__main__':
    unittest.main()
