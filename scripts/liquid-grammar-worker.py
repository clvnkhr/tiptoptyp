#!/usr/bin/env python3
"""Optional local Liquid grammar worker: one lazy model, serial bounded JSONL.

Request: {"id": unsigned_integer, "text": selected_prose}
Success: {"id": ..., "ok": true, "corrected": ..., "elapsed_ms": ...}
Failure: {"id": ... or null, "ok": false, "error": {"code": ..., "message": ...}}

Only --install may access the network. Correction never downloads artifacts.
The app materializes this file beside the exact pinned comparison models.py;
repository execution finds the same adapter under grammar_compare/models.py.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager, redirect_stderr, redirect_stdout
import hashlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from typing import Any

MODEL_REPO = 'LiquidAI/LFM2.5-Encoder-350M-Spellchecker'
MODEL_REVISION = '4e2f724c10985e9ec90f34189c22be5d5f2e41e0'
ENCODER_REPO = 'LiquidAI/LFM2.5-Encoder-350M'
ENCODER_REVISION = 'b886781f7c6f10ca9b7096e21b83e30a073c2f39'
REQUIREMENTS = {
    'torch': '2.14.1', 'transformers': '5.20.2', 'tokenizers': '0.23.3',
    'huggingface-hub': '2.2.0', 'safetensors': '0.8.0', 'sacremoses': '0.2.0',
}
# SHA256 values are fixed in reviewed application code, not trusted from a cache
# receipt. The encoder's unused weights and optional reranker are never fetched.
ARTIFACTS = (
    (MODEL_REPO, MODEL_REVISION, 'config.json', 494,
     'edeaa3214d6a34153416e3417c8960c998e9144f2d87e6bff7eac94d4da54102'),
    (MODEL_REPO, MODEL_REVISION, 'model.safetensors', 713449656,
     'e788053172f2dbc5a1573454475230dca21f19cd3ca1193e2dca5b1a9fd27bfa'),
    (MODEL_REPO, MODEL_REVISION, 'modeling_gectagger.py', 20673,
     'ecd2ee64c22e3ad020fc2fd9c926d2163549d624ba28a36b38035483d508d552'),
    (MODEL_REPO, MODEL_REVISION, 'reranker_gectagger.py', 22767,
     'c63b299cab6d24b144642fc106d75b5c7fe1bc3ec620495b56f0294854536a13'),
    (MODEL_REPO, MODEL_REVISION, 'tokenizer.json', 4733371,
     '1efc3a6609abf6b63b1f47188d139f3b59973a6a434dffe970a7261a51ed2711'),
    (MODEL_REPO, MODEL_REVISION, 'tokenizer_config.json', 306,
     'ba91dd002005cdf48ada14d3406571d06adc50b141bb4284e390b0e4c8d8aa2c'),
    (ENCODER_REPO, ENCODER_REVISION, 'config.json', 1503,
     '559f88ddcebfb0a7b46ba99a074b17f3a951f278b3010a0f22fdf5d9cc358d11'),
    (ENCODER_REPO, ENCODER_REVISION, 'modeling_lfm2_bidirectional.py', 7093,
     'f171f518be2a07da48b17fdea5655cad0a2452ab548e90e8ae903143686647e2'),
)
MAX_LINE_BYTES = 64 * 1024
MAX_TEXT_CHARS = 4096
MAX_TEXT_BYTES = 16 * 1024
MAX_OUTPUT_CHARS = 8192
INPUT_TOKEN_LIMIT = 128
_SILENT_STREAM = None


class Failure(Exception):
    """Stable, source-free user-facing failures; never expose exception text."""
    def __init__(self, code: str, message: str):
        self.code, self.message = code, message
        super().__init__(message)

    def response(self, identifier=None):
        return {'id': identifier, 'ok': False,
                'error': {'code': self.code, 'message': self.message}}


@contextmanager
def quiet_vendor_io():
    # Keep this one stream alive: vendor logging handlers may retain it after
    # importing. A StringIO could accumulate unbounded library diagnostics.
    global _SILENT_STREAM
    if _SILENT_STREAM is None:
        _SILENT_STREAM = open(os.devnull, 'w')
    with redirect_stdout(_SILENT_STREAM), redirect_stderr(_SILENT_STREAM):
        yield


def artifact_path(cache_dir: Path, artifact):
    repo, revision, name, _, _ = artifact
    return cache_dir / 'hub' / ('models--' + repo.replace('/', '--')) / 'snapshots' / revision / name


def verify_artifacts(cache_dir: Path, *, full: bool, artifacts=ARTIFACTS):
    for artifact in artifacts:
        path = artifact_path(cache_dir, artifact)
        try:
            if not path.is_file():
                raise Failure('model_not_installed', 'Install the local Liquid model before checking grammar.')
            if path.stat().st_size != artifact[3]:
                raise Failure('integrity_failed', 'Local Liquid model files failed verification. Reinstall the model.')
            if full:
                with path.open('rb') as stream:
                    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                if digest != artifact[4]:
                    raise Failure('integrity_failed', 'Local Liquid model files failed verification. Reinstall the model.')
        except OSError as error:
            raise Failure('model_unreadable', 'The local Liquid model files could not be read. Check the model folder permissions.') from error


def dependency_versions():
    if sys.version_info[:2] != (3, 12):
        raise Failure('unsupported_python', 'Liquid grammar requires its Python 3.12 environment. Run setup or choose that interpreter.')
    versions = {}
    for package, expected in REQUIREMENTS.items():
        try:
            versions[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError as error:
            raise Failure('runtime_unavailable', 'Install the Liquid grammar runtime to enable this action.') from error
        if versions[package] != expected:
            raise Failure('runtime_version_mismatch', 'The Liquid grammar runtime has different dependency versions. Run setup again.')
    return versions


def configure_cache(cache_dir: Path):
    # Keep dynamic custom-code compilation and Xet metadata in the explicitly
    # chosen application folder, rather than an unrelated global HF cache.
    root = cache_dir.resolve()
    os.environ.update({
        'HF_HOME': str(root / 'huggingface'),
        'HF_HUB_CACHE': str(root / 'hub'),
        'HF_MODULES_CACHE': str(root / 'modules'),
        'HF_XET_CACHE': str(root / 'xet'),
    })


def configure_offline(threads: int):
    os.environ.update({
        'HF_HUB_OFFLINE': '1', 'TRANSFORMERS_OFFLINE': '1',
        'HF_HUB_DISABLE_TELEMETRY': '1', 'HF_HUB_DISABLE_PROGRESS_BARS': '1',
        'TOKENIZERS_PARALLELISM': 'false', 'OMP_NUM_THREADS': str(threads),
        'MKL_NUM_THREADS': str(threads),
    })


def adapter_module():
    here = Path(__file__).resolve().parent
    candidates = [here / 'models.py', here / 'grammar_compare' / 'models.py']
    path = next((path for path in candidates if path.is_file()), None)
    if path is None:
        raise Failure('adapter_unavailable', 'The local grammar helper is incomplete. Run setup again.')
    name = '_tiptoptyp_liquid_adapter'
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise Failure('adapter_unavailable', 'The local grammar helper could not be opened. Run setup again.')
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    if (module.MODEL_SPECS['liquid'].revision != MODEL_REVISION
            or module.LIQUID_ENCODER_REVISION != ENCODER_REVISION):
        raise Failure('adapter_version_mismatch', 'The local grammar helper uses a different model version. Run setup again.')
    return module


class LocalAdapter:
    def __init__(self, adapter, module):
        self.adapter, self.module = adapter, module

    def correct(self, text):
        try:
            return self.adapter.correct(text)
        except self.module.InputTooLongError as error:
            raise Failure('input_too_long', "A selected prose fragment exceeds Liquid's 128-token limit. Select a shorter sentence.") from error
        except self.module.OutputTooLongError as error:
            raise Failure('output_too_long', 'The model could not finish a bounded correction. Select a shorter sentence.') from error


def load_adapter(cache_dir: Path, threads: int):
    # No library or tokenizer may try a network fallback during correction.
    configure_cache(cache_dir)
    configure_offline(threads)
    dependency_versions()
    verify_artifacts(cache_dir, full=True)
    with quiet_vendor_io():
        try:
            import torch
            from huggingface_hub import snapshot_download
            module = adapter_module()

            def local_snapshot(cache, repo, revision, patterns):
                if (repo, revision) not in {(MODEL_REPO, MODEL_REVISION), (ENCODER_REPO, ENCODER_REVISION)}:
                    raise Failure('adapter_version_mismatch', 'The grammar helper requested an unapproved model version.')
                return Path(snapshot_download(repo_id=repo, revision=revision,
                                              cache_dir=cache / 'hub', allow_patterns=patterns,
                                              local_files_only=True))

            module._snapshot = local_snapshot
            torch.set_num_interop_threads(1)
            adapter = module.load_model('liquid', cache_dir, threads=threads)
            return LocalAdapter(adapter, module)
        except Failure:
            raise
        except (ImportError, OSError) as error:
            raise Failure('runtime_unavailable', 'The local Liquid runtime could not load. Run setup or choose a working Python environment.') from error
        except Exception as error:
            raise Failure('model_load_failed', 'The local Liquid model could not load. Reinstall the runtime and model.') from error


class Worker:
    def __init__(self, cache_dir: Path, threads: int = 2, loader=load_adapter):
        self.cache_dir, self.threads, self.loader = cache_dir, threads, loader
        self.adapter = None
        self.unavailable = None

    def respond(self, request: Any):
        identifier = request.get('id') if isinstance(request, dict) else None
        if type(identifier) is not int or not 0 <= identifier < 2 ** 64:
            return Failure('invalid_request', 'A request must include an unsigned integer id.').response()
        if not isinstance(request, dict) or set(request) != {'id', 'text'}:
            return Failure('invalid_request', 'A request must contain only id and text.').response(identifier)
        text = request['text']
        if not isinstance(text, str):
            return Failure('invalid_request', 'Selected text must be a string.').response(identifier)
        if not text.strip():
            return Failure('empty_selection', 'Select some prose before checking grammar.').response(identifier)
        if (any(0xD800 <= ord(ch) <= 0xDFFF for ch in text)
                or any(ord(ch) < 32 and ch not in '\n\r\t' for ch in text)):
            return Failure('invalid_text', 'The selection contains unsupported control characters.').response(identifier)
        if len(text) > MAX_TEXT_CHARS or len(text.encode('utf-8')) > MAX_TEXT_BYTES:
            return Failure('input_too_long', 'The selected fragment is too large. Select a shorter sentence.').response(identifier)
        started = time.perf_counter()
        try:
            if self.unavailable is not None:
                raise self.unavailable
            if self.adapter is None:
                try:
                    self.adapter = self.loader(self.cache_dir, self.threads)
                except Failure as error:
                    self.unavailable = error
                    raise
            with quiet_vendor_io():
                corrected = self.adapter.correct(text)
            if (not isinstance(corrected, str) or not corrected.strip()
                    or len(corrected) > MAX_OUTPUT_CHARS
                    or any(0xD800 <= ord(ch) <= 0xDFFF for ch in corrected)
                    or any(ord(ch) < 32 and ch not in '\n\r\t' for ch in corrected)):
                raise Failure('invalid_output', 'The local model returned an unusable correction. Try a shorter sentence.')
            return {'id': identifier, 'ok': True, 'corrected': corrected,
                    'elapsed_ms': (time.perf_counter() - started) * 1000}
        except Failure as error:
            return error.response(identifier)
        except Exception:
            return Failure('inference_failed', 'The local grammar check could not complete. Try a shorter sentence or restart the helper.').response(identifier)


def decode_request(line):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('Duplicate JSON field')
            result[key] = value
        return result

    def reject_constant(_):
        raise ValueError('Non-finite JSON number')

    return json.loads(line.decode('utf-8'), object_pairs_hook=unique_object,
                      parse_constant=reject_constant)


def serve(input_stream, output_stream, worker: Worker):
    while True:
        line = input_stream.readline(MAX_LINE_BYTES + 1)
        if not line:
            return
        if len(line) > MAX_LINE_BYTES:
            while not line.endswith(b'\n'):
                line = input_stream.readline(MAX_LINE_BYTES + 1)
                if not line:
                    break
            response = Failure('request_too_large', 'The grammar request is too large. Select less text.').response()
        else:
            try:
                request = decode_request(line)
                response = worker.respond(request)
            except (UnicodeDecodeError, ValueError, RecursionError):
                response = Failure('invalid_json', 'The grammar request is not valid UTF-8 JSON.').response()
        output_stream.write(json.dumps(response, ensure_ascii=False, separators=(',', ':')) + '\n')
        output_stream.flush()


def threads_value(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError('Threads must be between 1 and 8.') from error
    if not 1 <= number <= 8:
        raise argparse.ArgumentTypeError('Threads must be between 1 and 8.')
    return number


def install(cache_dir: Path):
    """Explicit setup only: download pinned artifacts, never initialize a model."""
    configure_cache(cache_dir)
    dependency_versions()
    force = False
    try:
        verify_artifacts(cache_dir, full=True)
    except Failure as error:
        if error.code not in ('model_not_installed', 'integrity_failed'):
            raise
        force = error.code == 'integrity_failed'
        os.environ.update({'HF_HUB_OFFLINE': '0', 'TRANSFORMERS_OFFLINE': '0',
                           'HF_HUB_DISABLE_TELEMETRY': '1', 'HF_HUB_DISABLE_PROGRESS_BARS': '1'})
        with quiet_vendor_io():
            try:
                from huggingface_hub import snapshot_download
                for repo, revision in ((MODEL_REPO, MODEL_REVISION), (ENCODER_REPO, ENCODER_REVISION)):
                    names = [row[2] for row in ARTIFACTS if (row[0], row[1]) == (repo, revision)]
                    snapshot_download(repo_id=repo, revision=revision, cache_dir=cache_dir / 'hub',
                                      allow_patterns=names, force_download=force, local_files_only=False)
            except Exception as download_error:
                raise Failure('install_failed', 'The pinned Liquid model could not download. Check network access and retry setup.') from download_error
        verify_artifacts(cache_dir, full=True)
    cache_dir.mkdir(parents=True, exist_ok=True)
    receipt = dict(version=1, model=MODEL_REPO, revision=MODEL_REVISION,
                   encoder_revision=ENCODER_REVISION, runtime_versions=REQUIREMENTS,
                   files=[dict(repo=r, revision=v, name=n, bytes=b, sha256=h) for r, v, n, b, h in ARTIFACTS])
    temporary = None
    try:
        with tempfile.NamedTemporaryFile('w', encoding='utf-8', dir=cache_dir,
                                         prefix='.liquid-install-', delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(receipt, stream, ensure_ascii=False, indent=2)
            stream.write('\n')
        temporary.replace(cache_dir / 'liquid-grammar-install.json')
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return {'ok': True, 'installed': True, 'model': MODEL_REPO, 'revision': MODEL_REVISION,
            'artifact_bytes': sum(row[3] for row in ARTIFACTS)}


def check(cache_dir: Path):
    versions = dependency_versions()
    verify_artifacts(cache_dir, full=False)
    return {'ok': True, 'ready': True, 'model': MODEL_REPO, 'revision': MODEL_REVISION,
            'runtime_versions': versions, 'integrity': 'size_preflight_only; SHA256 is checked before first inference'}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-dir', type=Path, required=True)
    parser.add_argument('--threads', type=threads_value, default=2)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--install', action='store_true')
    modes.add_argument('--check', action='store_true')
    args = parser.parse_args(argv)
    if args.install or args.check:
        try:
            result = install(args.cache_dir) if args.install else check(args.cache_dir)
        except Failure as error:
            result = error.response()
        except Exception:
            result = Failure('setup_failed', 'The local grammar setup could not complete. Check the Python environment and model folder.').response()
        print(json.dumps(result, ensure_ascii=False, separators=(',', ':')), flush=True)
        return 0 if result['ok'] else 1
    configure_cache(args.cache_dir)
    configure_offline(args.threads)
    serve(sys.stdin.buffer, sys.stdout, Worker(args.cache_dir, args.threads))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
