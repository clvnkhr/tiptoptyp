"""Fixture transport checks; these are not native interaction evidence."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sys
import json
import shutil
import subprocess
import unittest
from urllib.request import Request, urlopen
from urllib.error import HTTPError

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overleaf_storage_fixture import (ASSET, ASSET_BYTES, DOCUMENT, PROJECT, SOURCE,
                                      PEER, SECOND_DOCUMENT, SECOND_SOURCE, PREVIEW_DOCUMENT, StorageFixture)


class StorageFixtureTests(unittest.TestCase):
    def test_preview_document_is_opt_in_authoritative_and_saved_independently(self):
        preview = '= Server-owned preview. α\n'
        for file_actions in (False, True):
            with self.subTest(file_actions=file_actions), StorageFixture(file_actions=file_actions) as disabled:
                origin = disabled.url.rsplit('/project/', 1)[0]
                with self.assertRaises(HTTPError) as missing:
                    urlopen(f'{origin}/project/{PROJECT}/doc/{PREVIEW_DOCUMENT}/download', timeout=2)
                self.assertEqual(missing.exception.code, 404)
                missing.exception.close()
                self.assertNotIn('preview.typ', disabled.javascript())
            with self.subTest(file_actions=file_actions), StorageFixture(
                    file_actions=file_actions, preview_source=preview) as fixture:
                origin = fixture.url.rsplit('/project/', 1)[0]
                route = f'{origin}/project/{PROJECT}/doc/{PREVIEW_DOCUMENT}/download'
                self.assertEqual(urlopen(route, timeout=2).read(), preview.encode())
                self.assertIn('preview.typ', fixture.javascript())
                self.assertIn(PREVIEW_DOCUMENT, fixture.javascript())
                if file_actions:
                    self.assertEqual(fixture.files.entry('preview.typ')['source'], preview)
                updated = preview + 'A separate preview edit.\n'
                urlopen(Request(origin + '/fixture/source/' + PREVIEW_DOCUMENT,
                                data=updated.encode(), method='POST'), timeout=2).read()
                self.assertEqual(urlopen(route, timeout=2).read(), updated.encode())
                self.assertEqual(fixture.source, SOURCE)
                self.assertEqual(fixture.save_count, 1)

    def test_preview_document_rejects_invalid_or_unbounded_source(self):
        for invalid in ('', b'bytes', 3, 'α' * 32769):
            with self.subTest(invalid_type=type(invalid)), self.assertRaises(ValueError):
                StorageFixture(preview_source=invalid)

    def test_loopback_routes_stream_fresh_source_and_hold_assets_until_explicit_release(self):
        with StorageFixture() as fixture:
            self.assertTrue(fixture.url.startswith('http://127.0.0.1:'))
            self.assertIn(b'/fixture.js', urlopen(fixture.url, timeout=2).read())
            origin = fixture.url.rsplit('/project/', 1)[0]
            source = urlopen(f'{origin}/project/{PROJECT}/doc/{DOCUMENT}/download', timeout=2).read()
            self.assertEqual(source, SOURCE.encode())
            self.assertTrue(fixture.document_requested.is_set())
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(lambda: urlopen(f'{origin}/project/{PROJECT}/file/{ASSET}', timeout=3).read())
                self.assertTrue(fixture.asset_requested.wait(2))
                self.assertFalse(pending.done())
                fixture.release_assets.set()
                self.assertEqual(pending.result(timeout=2), ASSET_BYTES)
            self.assertEqual(fixture.save_count, 0)
            urlopen(Request(origin + '/fixture/source', data=b'Updated fixture.', method='POST'), timeout=2).read()
            self.assertEqual(fixture.source, 'Updated fixture.')
            self.assertEqual(fixture.save_count, 1)
            collaborator = 'Remote collaborator changed this sentence. α\n'
            fixture.replace_remote(collaborator)
            self.assertEqual(fixture.source, collaborator)
            self.assertEqual(fixture.save_count, 1)
            self.assertEqual(urlopen(f'{origin}/project/{PROJECT}/doc/{DOCUMENT}/download',
                                     timeout=2).read(), collaborator.encode())
            self.assertIn(json.dumps(collaborator), fixture.javascript())

    def test_project_names_roundtrip_exactly_through_the_served_frontend_tree(self):
        node = shutil.which('node')
        if node is None:
            self.skipTest('Node.js is required to observe the generated project event')
        long_name = ('Mathematical 調和解析 – "quoted" \\ paths & <not-a-tag> ' * 4
                     + 'FINAL-TAIL.tex')
        runtime = r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const timers = [], projects = [];
const window = {dispatchEvent() {}};
vm.runInNewContext(input.script, {
  window, document: {}, CustomEvent: class {},
  setTimeout(fn) { timers.push(fn); }
}, {timeout: 1000});
window.io.connect().on('joinProjectResponse', event => projects.push(event.project));
for (const timer of timers) timer();
assert.equal(projects.length, 1, 'the served frontend must publish one actual project tree');
assert.equal(projects[0].name, input.expected);
console.log(JSON.stringify({name: projects[0].name, documents: projects[0].rootFolder[0].docs.length}));
"""
        for options, expected in (({}, 'Offline storage fixture'),
                                  ({'project_name': long_name}, long_name)):
            with self.subTest(custom_name=bool(options)), StorageFixture(**options) as fixture:
                origin = fixture.url.rsplit('/project/', 1)[0]
                with urlopen(origin + '/fixture.js', timeout=2) as response:
                    script = response.read().decode('utf-8')
                self.assertIn(json.dumps(expected), script)
                result = subprocess.run([node, '-e', runtime], input=json.dumps(dict(
                    script=script, expected=expected)), capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), dict(name=expected, documents=1))
                self.assertEqual(fixture.source, SOURCE)
                self.assertEqual(fixture.save_count, 0)

    def test_collaborator_edits_reject_unbounded_or_invalid_inputs_without_changing_server(self):
        fixture = StorageFixture()
        try:
            for invalid in (None, b'not text', 'α' * 32769):
                with self.subTest(invalid_type=type(invalid)), self.assertRaises(ValueError):
                    fixture.replace_remote(invalid)
                self.assertEqual(fixture.source, SOURCE)
                self.assertEqual(fixture.save_count, 0)
        finally:
            fixture.server.server_close()

    def test_server_connection_loss_reaches_only_the_current_frontend_transport(self):
        with StorageFixture(connection_events=True) as fixture:
            origin = fixture.url.rsplit('/project/', 1)[0]
            def connection(generation):
                try:
                    with urlopen(origin + '/fixture/connection?generation=' + str(generation),
                                 timeout=3) as response:
                        return response.status
                except HTTPError as error:
                    status = error.code
                    error.close()
                    return status
            self.assertIn('/fixture/connection?generation=0', fixture.javascript())
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(connection, 0)
                self.assertTrue(fixture.connection_requested.wait(2))
                self.assertFalse(pending.done())
                self.assertEqual(fixture.lose_connection(), 1)
                self.assertEqual(pending.result(timeout=2), 503)
                # The old browser cannot become live again, while a newly
                # loaded official frontend watches only the new generation.
                self.assertEqual(connection(0), 503)
                self.assertIn('/fixture/connection?generation=1', fixture.javascript())
                fixture.connection_requested.clear()
                pending = pool.submit(connection, 1)
                self.assertTrue(fixture.connection_requested.wait(2))
                self.assertFalse(pending.done())
                fixture.lose_connection()
                self.assertEqual(pending.result(timeout=2), 503)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_page_departure_navigates_to_an_isolated_loopback_origin_without_a_write(self):
        with StorageFixture(connection_events=True) as fixture:
            origin = fixture.url.rsplit('/project/', 1)[0]
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(lambda: json.loads(urlopen(
                    origin + '/fixture/connection?generation=0', timeout=3).read()))
                self.assertTrue(fixture.connection_requested.wait(2))
                self.assertFalse(pending.done())
                self.assertEqual(fixture.depart_page(), 1)
                event = pending.result(timeout=2)
            self.assertEqual(event, dict(generation=1, navigate=fixture.departure_url))
            self.assertLess(len(json.dumps(event)), 256)
            self.assertIn('http://localhost:', fixture.departure_url)
            self.assertNotIn('http://localhost:', fixture.url)
            self.assertFalse(fixture.page_requested.is_set(), 'the server cannot fabricate browser navigation')
            page = urlopen(fixture.departure_url, timeout=2).read()
            self.assertTrue(fixture.page_requested.is_set())
            for prohibited in (b'<script', b'fixture.js', b'window.ipc', b'postMessage'):
                self.assertNotIn(prohibited, page)
            fixture.restore_page()
            with self.assertRaises(HTTPError) as stale:
                urlopen(origin + '/fixture/connection?generation=0', timeout=2)
            self.assertEqual(stale.exception.code, 503)
            stale.exception.close()
            self.assertIn('/fixture/connection?generation=1', fixture.javascript())
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_ordinary_browser_input_page_uses_only_local_http_without_project_or_native_effects(self):
        with StorageFixture() as fixture:
            page = urlopen(fixture.browsing_url, timeout=2).read()
            self.assertIn(b'id="browser-input"', page)
            self.assertIn(b'encodeURIComponent(event.target.value)', page)
            for prohibited in (b'window.ipc', b'postMessage', b'tiptoptypOverleaf', b'fixture.js'):
                self.assertNotIn(prohibited, page)
            endpoint = fixture.browsing_url.rsplit('/', 1)[0] + '/fixture/browser-input?text=actual%20browser%20typing'
            with urlopen(endpoint, timeout=2) as response:
                self.assertEqual(response.status, 204)
            self.assertIn('/fixture/browser-input?text=actual%20browser%20typing', fixture.requests)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_page_departure_is_opt_in(self):
        fixture = StorageFixture()
        try:
            for action in (fixture.depart_page, fixture.restore_page):
                with self.assertRaises(ValueError):
                    action()
            self.assertEqual(fixture._connection_generation, 0)
            self.assertFalse(fixture.page_requested.is_set())
        finally:
            fixture.server.server_close()

    def test_departing_frontend_cannot_emit_disconnect_before_or_during_pagehide(self):
        fixture = StorageFixture(connection_events=True)
        try:
            node = shutil.which('node')
            if node is None:
                self.skipTest('Node.js is required for the frontend navigation contract')
            runtime = r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const events = [], requests = [];
let reply, socket;
const window = {dispatchEvent() {}, location: {assign(url) {
  events.push(['navigate', url]);
  socket.disconnect(); // the bridge's pagehide cleanup must remain silent too
}}};
vm.runInNewContext(input.script, {
  window, document: {}, CustomEvent: class {}, setTimeout() {},
  fetch(url) { requests.push(url); return new Promise(resolve => { reply = resolve; }); }
}, {timeout: 1000});
socket = window.io.connect();
socket.on('disconnect', () => events.push(['disconnect']));
reply({ok: true, json: () => Promise.resolve({generation: 1, navigate: input.destination})});
(async () => {
  for (let count = 0; count < 12; count++) await Promise.resolve();
  assert.deepEqual(events, [['navigate', input.destination]]);
  socket.disconnect();
  assert.deepEqual(events, [['navigate', input.destination]]);
  assert.equal(requests.length, 1, 'departure cannot restart a connection poll');
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
            result = subprocess.run([node, '-e', runtime], input=json.dumps(dict(
                script=fixture.javascript(), destination=fixture.departure_url)),
                capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)
        finally:
            fixture.server.server_close()

    def test_peer_moves_cross_documents_and_disconnect_over_the_official_event_transport(self):
        with StorageFixture(presence=True) as fixture:
            origin = fixture.url.rsplit('/project/', 1)[0]
            def event(generation):
                return json.loads(urlopen(origin + '/fixture/presence?generation=' + str(generation),
                                          timeout=3).read())
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(event, 0)
                self.assertTrue(fixture.presence_requested.wait(2))
                self.assertFalse(pending.done())
                fixture.move_peer(DOCUMENT, 0, 6)
                received = pending.result(timeout=2)
                self.assertEqual(received, dict(generation=1, peer=dict(
                    id=PEER, user_id='fixture-user', name='Fixture Collaborator',
                    doc_id=DOCUMENT, row=0, column=6)))
                fixture.move_peer(SECOND_DOCUMENT, 80, 5)
                received = event(1)
                self.assertEqual(received['generation'], 2)
                self.assertEqual(received['peer']['doc_id'], SECOND_DOCUMENT)
                self.assertEqual(received['peer']['row'], 80)
                fixture.remove_peer()
                self.assertEqual(event(2), dict(generation=3, peer=None))
            self.assertEqual(urlopen(f'{origin}/project/{PROJECT}/doc/{SECOND_DOCUMENT}/download',
                                     timeout=2).read(), SECOND_SOURCE.encode())
            script = fixture.javascript()
            for text in ("'clientTracking.clientUpdated'", "'clientTracking.clientDisconnected'",
                         "'doc:after-opened'", 'other.txt', 'dataset.fileId'):
                self.assertIn(text, script)
            for text in ('window.ipc', 'postMessage', 'tiptoptypOverleaf.command'):
                self.assertNotIn(text, script)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_peer_transport_rejects_invalid_positions_and_is_opt_in(self):
        fixture = StorageFixture(presence=True)
        disabled = StorageFixture()
        try:
            for position in ((DOCUMENT, -1, 0), (DOCUMENT, 0, -1), (DOCUMENT, True, 0),
                             (DOCUMENT, 1.5, 0), (DOCUMENT, 0, True),
                             (DOCUMENT, 900, 0), (DOCUMENT, 0, 900), ('f' * 24, 0, 0)):
                with self.subTest(position=position), self.assertRaises(ValueError):
                    fixture.move_peer(*position)
                self.assertEqual(fixture._presence_generation, 0)
                self.assertIsNone(fixture._peer)
            with self.assertRaises(ValueError):
                disabled.move_peer(DOCUMENT, 0, 0)
            with self.assertRaises(ValueError):
                disabled.remove_peer()
            self.assertNotIn('watchPresence', disabled.javascript())
            self.assertNotIn('other.txt', disabled.javascript())
        finally:
            fixture.server.server_close()
            disabled.server.server_close()

    def test_frontend_uses_official_bridge_surfaces_without_native_ipc_shortcuts(self):
        preview = '= Authoritative preview. α\n'
        fixture = StorageFixture(presence=True, preview_source=preview)
        try:
            script = fixture.javascript()
            for text in ('window.io.connect', 'window.overleaf', 'joinProjectResponse',
                         "'doc:saved'", "'otUpdateApplied'", "'applyOtUpdate'"):
                self.assertIn(text, script)
            for text in ('window.ipc', 'postMessage', 'tiptoptypOverleaf.command'):
                self.assertNotIn(text, script)
            node = shutil.which('node')
            if node is None:
                self.skipTest('Node.js is required for the frontend opening contract')
            # Exercise the generated page with the same setter interception used
            # by the production bridge. Assigning view.state directly must fail:
            # the fresh-source barrier requires a completed setState call before
            # doc:after-opened, not merely a matching open_doc_id notification.
            runtime = r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const buttons = [], events = [], requests = [], timers = [];
const window = {dispatchEvent(event) { events.push(event.type); }};
const document = {
  createElement() { return {dataset: {}, setAttribute() {}}; },
  body: {appendChild(node) { buttons.push(node); }}
};
vm.runInNewContext(input.script, {
  window, document,
  CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
  fetch(url, options) { requests.push({url, options}); return new Promise(() => {}); },
  setTimeout(fn) { timers.push(fn); }
}, {timeout: 1000});
const store = window.overleaf.unstable.store;
const view = store.get('editor.view');
const socket = window.io.connect();
const projects = [];
socket.on('joinProjectResponse', event => projects.push(event.project));
for (const timer of timers) timer();
assert.equal(projects.length, 1);
assert.ok(projects[0].rootFolder[0].docs.some(doc => doc._id === input.previewDocument && doc.name === 'preview.typ'));
const setState = view.setState;
view.setState = function(state) {
  const result = setState.call(this, state);
  assert.equal(store.get('editor.open_doc_id'), expectedDocument);
  assert.equal(this.state.doc.toString(), expectedSource);
  events.push('setState');
  return result;
};
store.watch('editor.open_doc_id', id => {
  assert.equal(id, expectedDocument);
  assert.equal(view.state.doc.toString(), expectedSource);
  events.push('doc-id-watch');
});
store.watch('editor.view', next => {
  assert.equal(next, view);
  events.push('view-watch');
});
const emit = socket.emit;
socket.emit = function(event, ...args) {
  if (event === 'joinDoc') {
    assert.equal(args[0], expectedDocument);
    events.push('joinDoc');
  }
  return emit.call(this, event, ...args);
};
let expectedDocument, expectedSource;
for (const [id, source] of [[input.secondDocument, input.secondSource], [input.previewDocument, input.previewSource], [input.mainDocument, input.mainSource]]) {
  expectedDocument = id; expectedSource = source;
  events.length = 0;
  buttons.find(button => button.dataset.fileId === id).onclick();
  assert.deepEqual(events, ['setState', 'doc-id-watch', 'view-watch', 'joinDoc', 'doc:after-opened']);
  assert.equal(store.get('editor.open_doc_id'), id);
  assert.equal(view.state.doc.toString(), source);
}
assert.ok(requests.every(request => !request.options));
"""
            result = subprocess.run([node, '-e', runtime], input=json.dumps(dict(
                script=script, mainDocument=DOCUMENT, mainSource=SOURCE,
                secondDocument=SECOND_DOCUMENT, secondSource=SECOND_SOURCE,
                previewDocument=PREVIEW_DOCUMENT, previewSource=preview)),
                capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)
        finally:
            fixture.server.server_close()
