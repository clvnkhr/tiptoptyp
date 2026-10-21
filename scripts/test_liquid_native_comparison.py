"""Protocol and timeout checks for the developer comparison, without model assets."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest

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


if __name__ == '__main__':
    unittest.main()
