#!/usr/bin/env python3
"""Opt-in macOS WKWebView check. Needs Swift, WindowServer and Tinymist."""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import plistlib
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--hold', action='store_true', help='Keep the final window visible for 60 seconds')
parser.add_argument('--without-load-replay', action='store_true', help='Negative probe: deliberately reproduce the reload failure')
args = parser.parse_args()
if sys.platform != 'darwin':
    parser.error('This native regression requires macOS, Swift and WindowServer')

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / '.tiptoptyp/screenshots/agent-review/native-preview-palette'
OUTPUT.mkdir(parents=True, exist_ok=True)
(OUTPUT / 'metadata.json').write_text(json.dumps({
    'platform': platform.platform(), 'viewport': [800, 600], 'pages': 32,
    'adapter_sha256': hashlib.sha256((ROOT / 'src/preview_navigation.js').read_bytes()).hexdigest(),
    'replay_after_load': not args.without_load_replay,
    'evidence': 'WKWebView snapshots; on-screen composition requires separate app observation'
}, indent=2) + '\n')
program = os.environ['TIPTOPTYP_TEST_TINYMIST']
app = ROOT / '.tiptoptyp/native-preview-palette/Palette QA.app'
executable = app / 'Contents/MacOS/palette-test'
executable.parent.mkdir(parents=True, exist_ok=True)
(app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'dev.tiptoptyp.palette-test', 'CFBundleName': 'tiptoptyp Palette QA',
    'CFBundleExecutable': 'palette-test', 'CFBundlePackageType': 'APPL'}))
subprocess.run(['swiftc', str(ROOT / 'scripts/test-preview-palette.swift'), '-o', str(executable)], check=True, timeout=60)
with tempfile.TemporaryDirectory(prefix='ttt-native-palette-') as directory:
    source = Path(directory) / 'main.typ'
    source.write_text('\n#pagebreak()\n'.join(f'= Page {n}\nNative preview text.' for n in range(1, 33)))
    with (Path(directory) / 'server.log').open('w+') as log:
        server = subprocess.Popen([program, 'preview', '--no-open', '--partial-rendering=false',
                                   '--host', '127.0.0.1:0', '--data-plane-host', '127.0.0.1:0',
                                   '--control-plane-host', '127.0.0.1:0', '--invert-colors', 'never', str(source)],
                                  stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                log.seek(0)
                match = re.search(r'Static file server listening on: ([\d.:]+)', log.read())
                if match:
                    break
                if server.poll() is not None:
                    raise RuntimeError('Tinymist exited before starting preview')
                time.sleep(.05)
            else:
                raise TimeoutError('Tinymist preview did not start')
            subprocess.run([str(executable),
                            'http://' + match[1], str(ROOT / 'src/preview_navigation.js'), str(OUTPUT),
                            *sys.argv[1:]], check=True, timeout=100 if "--hold" in sys.argv else 45)
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
