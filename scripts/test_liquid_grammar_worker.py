"""Bounded, lazy application worker checks without ML dependencies or downloads."""
from contextlib import redirect_stderr, redirect_stdout
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parent / 'liquid-grammar-worker.py'
spec = importlib.util.spec_from_file_location('liquid_worker', SCRIPT)
worker_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker_module)


class FakeAdapter:
    def __init__(self):
        self.calls = []

    def correct(self, text):
        self.calls.append(text)
        return text.replace('is is', 'is')


class WorkerTests(unittest.TestCase):
    def test_model_load_is_lazy_and_reused_for_serial_requests(self):
        adapter = FakeAdapter()
        loads = []
        worker = worker_module.Worker(Path('/unused'), loader=lambda path, threads: (loads.append((path, threads)) or adapter))
        self.assertFalse(loads)
        first = worker.respond({'id': 1, 'text': 'This is is valid.'})
        second = worker.respond({'id': 2, 'text': 'Unicode é and 😀 remain.'})
        self.assertEqual(first['corrected'], 'This is valid.')
        self.assertEqual(second['corrected'], 'Unicode é and 😀 remain.')
        self.assertEqual(len(loads), 1)
        self.assertEqual(len(adapter.calls), 2)

    def test_invalid_inputs_never_load_the_model(self):
        def forbidden(*_):
            self.fail('Invalid input loaded the model')
        worker = worker_module.Worker(Path('/unused'), loader=forbidden)
        inputs = [None, [], {}, {'id': True, 'text': 'Prose'}, {'id': -1, 'text': 'Prose'},
                  {'id': 2**64, 'text': 'Prose'}, {'id': 1, 'text': 1},
                  {'id': 1, 'text': ' '}, {'id': 1, 'text': '\x00private'},
                  {'id': 1, 'text': '\ud800'}, {'id': 1, 'text': 'a' * 4097},
                  {'id': 1, 'text': 'valid', 'extra': 'secret'}]
        for request in inputs:
            with self.subTest(request=request):
                self.assertFalse(worker.respond(request)['ok'])

    def test_load_failure_is_cached_and_source_is_not_in_error_messages(self):
        loads = []
        def loader(*_):
            loads.append(1)
            raise worker_module.Failure('runtime_unavailable', 'Install the local runtime.')
        worker = worker_module.Worker(Path('/unused'), loader=loader)
        for identifier in (1, 2):
            result = worker.respond({'id': identifier, 'text': 'PRIVATE SOURCE TEXT'})
            self.assertEqual(result['error']['code'], 'runtime_unavailable')
            self.assertNotIn('PRIVATE', json.dumps(result))
        self.assertEqual(len(loads), 1)

    def test_inference_failure_does_not_log_or_echo_source_and_next_request_can_work(self):
        adapter = FakeAdapter()
        def fail(text):
            print(text)
            print(text, file=sys.stderr)
            raise RuntimeError(text)
        adapter.correct = fail
        worker = worker_module.Worker(Path('/unused'), loader=lambda *_: adapter)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            response = worker.respond({'id': 1, 'text': 'PRIVATE SOURCE TEXT'})
        self.assertEqual(response['error']['code'], 'inference_failed')
        self.assertEqual((out.getvalue(), err.getvalue()), ('', ''))
        self.assertNotIn('PRIVATE', json.dumps(response))
        adapter.correct = lambda text: text
        self.assertTrue(worker.respond({'id': 2, 'text': 'Valid.'})['ok'])

    def test_jsonl_recovers_after_invalid_encoding_json_and_an_oversized_frame(self):
        adapter = FakeAdapter()
        worker = worker_module.Worker(Path('/unused'), loader=lambda *_: adapter)
        valid = json.dumps({'id': 9, 'text': 'This is is valid.'}).encode() + b'\n'
        data = b'\xff\nnot json\n' + b'a' * (worker_module.MAX_LINE_BYTES + 50) + b'\n' + valid
        output = io.StringIO()
        worker_module.serve(io.BytesIO(data), output, worker)
        rows = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([row.get('error', {}).get('code') for row in rows[:3]],
                         ['invalid_json', 'invalid_json', 'request_too_large'])
        self.assertEqual(rows[3]['corrected'], 'This is valid.')
        self.assertEqual(adapter.calls, ['This is is valid.'])

    def test_model_token_limit_failure_is_explicit_and_never_truncated(self):
        class LimitAdapter:
            def correct(self, text):
                raise worker_module.Failure('input_too_long', 'A selected prose fragment exceeds the 128-token limit.')
        worker = worker_module.Worker(Path('/unused'), loader=lambda *_: LimitAdapter())
        result = worker.respond({'id': 3, 'text': 'Long but bounded selection.'})
        self.assertEqual(result['error']['code'], 'input_too_long')
        self.assertNotIn('corrected', result)

    def test_invalid_model_outputs_are_bounded(self):
        for output in (None, '', 'a' * 8193, '\ud800', '\x00private'):
            with self.subTest(output_type=type(output).__name__):
                adapter = FakeAdapter();adapter.correct = lambda text: output
                worker = worker_module.Worker(Path('/unused'), loader=lambda *_: adapter)
                self.assertEqual(worker.respond({'id': 1, 'text': 'Valid.'})['error']['code'], 'invalid_output')

    def test_checksum_validation_detects_same_size_tampering(self):
        artifact = ('test/repo', 'revision', 'file.txt', 4, hashlib.sha256(b'good').hexdigest())
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp)
            path = worker_module.artifact_path(cache, artifact)
            path.parent.mkdir(parents=True)
            path.write_bytes(b'good')
            worker_module.verify_artifacts(cache, full=True, artifacts=[artifact])
            path.write_bytes(b'evil')
            # Fast preflight is explicitly presence/size only, not integrity.
            worker_module.verify_artifacts(cache, full=False, artifacts=[artifact])
            with self.assertRaises(worker_module.Failure) as raised:
                worker_module.verify_artifacts(cache, full=True, artifacts=[artifact])
            self.assertEqual(raised.exception.code, 'integrity_failed')

    def test_offline_environment_is_enforced(self):
        with patch.dict(worker_module.os.environ, {'HF_HUB_OFFLINE': '0'}, clear=True):
            worker_module.configure_offline(2)
            self.assertEqual(worker_module.os.environ['HF_HUB_OFFLINE'], '1')
            self.assertEqual(worker_module.os.environ['TRANSFORMERS_OFFLINE'], '1')
            self.assertEqual(worker_module.os.environ['TOKENIZERS_PARALLELISM'], 'false')

    def test_vendor_caches_stay_inside_the_configured_application_folder(self):
        with tempfile.TemporaryDirectory() as temp,patch.dict(worker_module.os.environ,{},clear=True):
            root=Path(temp)
            worker_module.configure_cache(root)
            for name in ('HF_HOME','HF_HUB_CACHE','HF_MODULES_CACHE','HF_XET_CACHE'):
                self.assertTrue(Path(worker_module.os.environ[name]).is_relative_to(root.resolve()))
            self.assertEqual(Path(worker_module.os.environ['HF_HUB_CACHE']),root.resolve()/'hub')
            self.assertEqual(list(root.iterdir()),[])

    def test_import_and_invalid_request_need_no_optional_ml_packages(self):
        code = ("import importlib.util,sys;"
                f"s=importlib.util.spec_from_file_location('worker',{str(SCRIPT)!r});"
                "m=importlib.util.module_from_spec(s);s.loader.exec_module(m);"
                "assert 'torch' not in sys.modules and 'transformers' not in sys.modules;"
                "assert not m.Worker(m.Path('/unused')).respond({'id':1,'text':''})['ok'];"
                "assert 'torch' not in sys.modules")
        subprocess.run([sys.executable, '-c', code], check=True, capture_output=True, text=True)

    def test_install_does_not_load_model_and_writes_atomic_receipt_when_cached(self):
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp)
            with patch.object(worker_module, 'dependency_versions', return_value={}), \
                 patch.object(worker_module, 'verify_artifacts'), \
                 patch.object(worker_module, 'load_adapter', side_effect=AssertionError('Model loaded')):
                result = worker_module.install(cache)
            self.assertTrue(result['installed'])
            receipt = json.loads((cache / 'liquid-grammar-install.json').read_text())
            self.assertEqual(receipt['revision'], worker_module.MODEL_REVISION)
            self.assertEqual(len(receipt['files']), 8)
            self.assertEqual(list(cache.glob('.liquid-install-*')), [])


    def test_deep_duplicate_and_nonfinite_json_recover_without_crashing(self):
        worker = worker_module.Worker(Path('/unused'), loader=lambda *_: FakeAdapter())
        cases = [b'{"id":1,"id":2,"text":"private"}', b'{"id":NaN,"text":"private"}',
                 b'[' * 1500 + b'0' + b']' * 1500]
        output=io.StringIO()
        worker_module.serve(io.BytesIO(b'\n'.join(cases)+b'\n'),output,worker)
        rows=[json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([row['error']['code'] for row in rows[:2]],['invalid_json']*2)
        # Python 3.12 can reject deep JSON while newer decoders parse the same
        # bounded array iteratively; neither may terminate the worker.
        self.assertIn(rows[2]['error']['code'],{'invalid_json','invalid_request'})

    def test_local_adapter_preserves_full_input_when_reporting_token_limit(self):
        class TooLong(Exception):pass
        class OutputLong(Exception):pass
        calls=[]
        def fail(text):
            calls.append(text)
            raise TooLong('PRIVATE TEXT')
        adapter=worker_module.LocalAdapter(SimpleNamespace(correct=fail),
                    SimpleNamespace(InputTooLongError=TooLong,OutputTooLongError=OutputLong))
        original='Long source ' * 100
        with self.assertRaises(worker_module.Failure) as raised:
            adapter.correct(original)
        self.assertEqual(calls,[original])
        self.assertEqual(raised.exception.code,'input_too_long')
        self.assertNotIn('PRIVATE',raised.exception.message)

    def test_load_adapter_passes_local_files_only_even_when_hub_environment_was_online(self):
        from unittest.mock import Mock
        hub=SimpleNamespace(snapshot_download=Mock(return_value='/local/snapshot'))
        torch=SimpleNamespace(set_num_interop_threads=Mock())
        module=SimpleNamespace(InputTooLongError=ValueError,OutputTooLongError=RuntimeError)
        def load(name,cache,threads):
            module._snapshot(cache,worker_module.MODEL_REPO,worker_module.MODEL_REVISION,['config.json'])
            return FakeAdapter()
        module.load_model=load
        with patch.dict(sys.modules,{'torch':torch,'huggingface_hub':hub}), \
             patch.object(worker_module,'dependency_versions',return_value={}), \
             patch.object(worker_module,'verify_artifacts') as verify, \
             patch.object(worker_module,'adapter_module',return_value=module):
            worker_module.load_adapter(Path('/unused'),2)
        verify.assert_called_once_with(Path('/unused'),full=True)
        self.assertTrue(hub.snapshot_download.call_args.kwargs['local_files_only'])
        with self.assertRaises(worker_module.Failure):
            module._snapshot(Path('/unused'),'other/model','main',[])

    def test_explicit_install_downloads_only_the_pinned_allowed_files_without_inference(self):
        from unittest.mock import Mock
        hub=SimpleNamespace(snapshot_download=Mock())
        missing=worker_module.Failure('model_not_installed','Missing.')
        with tempfile.TemporaryDirectory() as temp:
            with patch.dict(sys.modules,{'huggingface_hub':hub}), \
                 patch.object(worker_module,'dependency_versions',return_value={}), \
                 patch.object(worker_module,'verify_artifacts',side_effect=[missing,None]), \
                 patch.object(worker_module,'load_adapter',side_effect=AssertionError('Model loaded')):
                worker_module.install(Path(temp))
        self.assertEqual(hub.snapshot_download.call_count,2)
        for call in hub.snapshot_download.call_args_list:
            kwargs=call.kwargs
            self.assertFalse(kwargs['local_files_only'])
            self.assertNotEqual(kwargs['revision'],'main')
            if kwargs['repo_id']==worker_module.ENCODER_REPO:
                self.assertEqual(set(kwargs['allow_patterns']),{'config.json','modeling_lfm2_bidirectional.py'})
            else:
                self.assertEqual(kwargs['repo_id'],worker_module.MODEL_REPO)
                self.assertNotIn('reranker/model.safetensors',kwargs['allow_patterns'])


if __name__ == '__main__':
    unittest.main()
