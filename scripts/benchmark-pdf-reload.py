#!/usr/bin/env python3
"""Run native WKWebView and PDFium probes sequentially, outside compilation.
Usage: python3 scripts/benchmark-pdf-reload.py RELEASE_TEST_BINARY SWIFT_PROBE OUT_DIR
Build the test binary with cargo test --release --bin tiptoptyp --no-run;
build the Swift probe with swiftc -O scripts/benchmark-pdfjs-native.swift -o PATH.
Requires macOS desktop, bundled Typst/PDFium; no third-party Python dependencies.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import threading
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
binary, swift, output = map(lambda p: Path(p).resolve(), sys.argv[1:])
output.mkdir(parents=True, exist_ok=True)
tool = root / 'toolchain/bin/typst-aarch64-apple-darwin'
source = (root / 'scripts/fixtures/pdfjs.typ').read_text()
metadata = {'platform': platform.platform(), 'machine': platform.machine(),
            'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
            'typst': subprocess.check_output([str(tool), '--version'], text=True).strip(),
            'test_binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'swift_binary_sha256': hashlib.sha256(swift.read_bytes()).hexdigest(),
            'profile': 'release', 'cpu': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip(),
            'memory_bytes': int(subprocess.check_output(['sysctl', '-n', 'hw.memsize'], text=True)),
            'fixtures': [], 'runs': []}
for count in [24, 240]:
    paths = []
    for variant in ['a', 'b']:
        src = output / f'{variant}{count}.typ'
        pdf = src.with_suffix('.pdf')
        src.write_text(source if variant == 'a' else source.replace('Scroll freely', 'Updated freely'))
        subprocess.run([str(tool), 'compile', '--input', f'pages={count}', str(src), str(pdf)], check=True)
        paths.append(pdf)
        metadata['fixtures'].append({'name': pdf.name, 'bytes': pdf.stat().st_size,
                                    'sha256': hashlib.sha256(pdf.read_bytes()).hexdigest()})
    for run in range(2):
        env = {**os.environ, 'TIPTOPTYP_PDFJS_FIXTURE': str(paths[0])}
        server = subprocess.Popen([str(binary), '--ignored', '--exact', 'pdfjs::tests::browser_fixture', '--nocapture'],
                                  cwd=root, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True, bufsize=1)
        inbox = queue.Queue()
        def read_lines():
            for line in server.stdout:
                inbox.put(line)
            inbox.put('')
        threading.Thread(target=read_lines, daemon=True).start()
        try:
            # Test harness lines precede the endpoint JSON.
            while True:
                line = inbox.get(timeout=20)
                if not line: raise RuntimeError('Rust fixture server exited')
                if line.startswith('{'):
                    url = json.loads(line)['url']
                    break
            destination = output / f'pdfjs-{count}-{run}.json'
            subprocess.run([str(swift), url, str(root/'scripts/benchmark-pdfjs-native.js'),
                            *map(str, paths), str(destination)], check=True, timeout=250)
        finally:
            if server.poll() is None:
                server.stdin.write('quit\n'); server.stdin.flush()
                try: server.wait(timeout=5)
                except subprocess.TimeoutExpired: server.kill(); server.wait()
        env = {**os.environ, 'TIPTOPTYP_PDFIUM_PROBE_A': str(paths[0]),
               'TIPTOPTYP_PDFIUM_PROBE_B': str(paths[1]), 'TIPTOPTYP_PDFIUM_PROBE_PAGES': str(count)}
        probe = subprocess.run([str(binary), '--ignored', '--exact', 'pdfium::tests::native_fixture_probe', '--nocapture'],
                               cwd=root, env=env, text=True, capture_output=True, timeout=120, check=True)
        worker = [json.loads(line.split('PDFIUM_PROBE ', 1)[1]) for line in probe.stdout.splitlines() if line.startswith('PDFIUM_PROBE ')]
        (output/f'pdfium-{count}-{run}.json').write_text(json.dumps(worker, indent=2)+'\n')
        metadata['runs'].append({'pages':count,'run':run,'pdfjs':json.loads(destination.read_text()),'pdfium':worker})
        (output/'results.json').write_text(json.dumps(metadata,indent=2)+'\n')
        print(f'Completed {count} pages, run {run+1}', flush=True)
