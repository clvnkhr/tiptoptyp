"""Lazy hosted-editor startup contract; native interaction is checked separately."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import json
import shutil
import subprocess
import sys
import unittest
from urllib.request import urlopen

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overleaf_storage_fixture import ASSET_BYTES, DOCUMENT, PROJECT, SOURCE, StorageFixture

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = r"""
'use strict';
const assert = require('node:assert/strict');
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const messages = [], navigations = [], chunks = [], events = new Map(), timers = new Map(), requests = [];
let nextTimer = 0;
const location = new URL(input.url + '?doc=' + input.document);
location.assign = value => { navigations.push(value); dispatch({type:'pagehide'}); };
function dispatch(event) { for (const handler of events.get(event.type) || []) handler(event); }
const document = {
  querySelector() { return null; },
  createElement(tag) { assert.equal(tag, 'script'); return {}; },
  head: {appendChild(chunk) { chunks.push(chunk.src); }},
};
const sandbox = {
  document, location, URL, URLSearchParams, TextDecoder, TextEncoder, Uint8Array, AbortController,
  tiptoptypOverleafEpoch: 7, localStorage: {setItem() {}},
  history: {replaceState(_state, _title, value) { location.href = value; }},
  ipc: {postMessage(value) { messages.push(JSON.parse(value)); }},
  addEventListener(name, fn) { if (!events.has(name)) events.set(name, new Set()); events.get(name).add(fn); },
  removeEventListener(name, fn) { events.get(name)?.delete(fn); },
  dispatchEvent: dispatch,
  CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail; } },
  setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, {fn, delay}); return id; },
  clearTimeout(id) { timers.delete(id); },
  btoa(value) { return Buffer.from(value, 'binary').toString('base64'); },
  fetch(url) {
    requests.push(url);
    if (url.includes('/doc/')) return Promise.resolve(new Response(input.source, {headers:{'Content-Type':'text/plain; charset=utf-8'}}));
    if (url.includes('/file/')) return Promise.resolve(new Response(Uint8Array.from(input.asset), {headers:{'Content-Type':'application/octet-stream'}}));
    throw new Error('Unexpected fixture request: ' + url);
  },
};
sandbox.window = sandbox; sandbox.top = sandbox;
const context = vm.createContext(sandbox);
function flushImmediate() {
  for (let count = 0; count < 100; count++) {
    const current = [...timers].find(([, timer]) => timer.delay === 0);
    if (!current) return;
    timers.delete(current[0]); current[1].fn();
  }
  throw new Error('Unbounded immediate startup timers');
}
(async () => {
  vm.runInContext(input.bridge, context, {filename:'overleaf_bridge.js'});
  vm.runInContext(input.script, context, {filename:'actual-storage-fixture.js'});
  flushImmediate();
  assert.equal(new URL(location.href).searchParams.has('doc'), false, 'hosted page consumes the deep-link query before mounting');
  assert.equal(sandbox.overleaf.unstable.store.get('editor.view'), undefined);
  assert.equal(sandbox.overleaf.unstable.store.get('editor.open_doc_id'), undefined);
  assert.deepEqual(chunks, ['/fixture/editor-chunk.js'], 'source editor is a separate pending asset');
  assert.deepEqual(navigations, [], 'bridge must not navigate while the lazy source bundle is pending without tree rows');
  assert.equal(requests.length, 0, 'source cannot be published before its real view mounts');
  vm.runInContext(input.chunk, context, {filename:'actual-held-editor-chunk.js'});
  for (let turn = 0; turn < 80; turn++) { await Promise.resolve(); flushImmediate(); }
  assert.deepEqual(navigations, [], 'natural view mounting never reloads the project');
  assert.equal(messages.filter(value => value.event === 'project').length, 1);
  assert.equal(messages.find(value => value.event === 'document').source, input.source);
  assert.equal(messages.filter(value => value.event === 'error').length, 0);
  assert.equal(messages.filter(value => value.event === 'status').at(-1).state, 'connected');
  sandbox.tiptoptypOverleaf.command({epoch:7, command:'disconnect'});
  assert.equal(timers.size, 0, 'disconnect removes the startup watchdog and finite bridge work');
  console.log(JSON.stringify({project_loads:1, pending_editor_chunks:chunks.length, navigations:navigations.length, hydrated:true}));
})().catch(error => { console.error(error); process.exitCode = 1; });
"""


def run_bridge_lifecycle(bridge_path):
    node = shutil.which('node')
    if node is None:
        raise unittest.SkipTest('Node.js is required for the real fixture startup contract')
    with StorageFixture(lazy_startup=True) as fixture:
        origin = fixture.url.rsplit('/project/', 1)[0]
        page = urlopen(fixture.url, timeout=2).read()
        script = urlopen(origin + '/fixture.js', timeout=2).read().decode()
        with ThreadPoolExecutor(max_workers=1) as pool:
            pending = pool.submit(lambda: urlopen(origin + '/fixture/editor-chunk.js', timeout=3).read().decode())
            if not fixture.editor_chunk_requested.wait(2) or pending.done():
                raise AssertionError('The source editor chunk must remain pending until released')
            fixture.release_editor_chunk.set()
            chunk = pending.result(timeout=2)
        if b'/fixture.js' not in page:
            raise AssertionError('The served project must load the actual fixture script')
        result = subprocess.run([node, '-e', RUNTIME], input=json.dumps(dict(
            url=fixture.url, document=DOCUMENT, source=SOURCE, asset=list(ASSET_BYTES),
            script=script, chunk=chunk, bridge=Path(bridge_path).read_text())),
            capture_output=True, text=True, timeout=8)
        if fixture.save_count != 0 or fixture.source != SOURCE:
            raise AssertionError('The read-only bootstrap contract modified server source')
        return result


class LazyOverleafBootstrapTests(unittest.TestCase):
    def test_held_real_editor_chunk_does_not_trigger_a_document_deep_link_reload(self):
        result = run_bridge_lifecycle(ROOT / 'src/overleaf_bridge.js')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), dict(project_loads=1, pending_editor_chunks=1,
                                                        navigations=0, hydrated=True))


if __name__ == '__main__':
    unittest.main()
