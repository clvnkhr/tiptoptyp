"""Bounded localhost Overleaf frontend fixture for real-WebView storage journeys.

The served page supplies only the official socket/store surfaces consumed by the
production bridge. It never posts native IPC or injects EditorApp state.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading

PROJECT = 'a' * 24
ROOT = 'b' * 24
DOCUMENT = 'c' * 24
ASSET = 'd' * 24
SOURCE = 'Fresh server-owned storage fixture.\n'
ASSET_BYTES = b'overleaf storage fixture asset\x00\xff'


class StorageFixture:
    def __init__(self, *, connection_events=False):
        self.asset_requested = threading.Event()
        self.release_assets = threading.Event()
        self.document_requested = threading.Event()
        self.connection_requested = threading.Event()
        self._lock = threading.Lock()
        self._condition = threading.Condition(self._lock)
        self._connection_events = connection_events
        self._connection_generation = 0
        self._stopping = False
        self._source = SOURCE
        self._save_count = 0
        self.requests = []
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def reply(self, status, content, kind):
                self.send_response(status)
                self.send_header('Content-Type', kind)
                self.send_header('Content-Length', str(len(content)))
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                try:
                    self.wfile.write(content)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def do_GET(self):
                with fixture._lock:
                    fixture.requests.append(self.path)
                if self.path.split('?')[0] == '/project/' + PROJECT:
                    self.reply(200, b'<!doctype html><meta charset="utf-8"><title>Storage fixture</title>'
                               b'<p>Offline Overleaf storage fixture</p><script src="/fixture.js"></script>', 'text/html')
                elif self.path == '/fixture.js':
                    self.reply(200, fixture.javascript().encode(), 'application/javascript')
                elif fixture._connection_events and self.path.startswith('/fixture/connection?generation='):
                    value = self.path.removeprefix('/fixture/connection?generation=')
                    if not value.isascii() or not value.isdigit() or len(value) > 9:
                        self.reply(400, b'Invalid connection generation', 'text/plain')
                        return
                    generation = int(value)
                    fixture.connection_requested.set()
                    with fixture._condition:
                        fixture._condition.wait_for(lambda:
                            fixture._stopping or generation != fixture._connection_generation,
                            timeout=20)
                        connected = not fixture._stopping and generation == fixture._connection_generation
                    self.reply(200 if connected else 503, b'Connected' if connected else b'Connection lost',
                               'text/plain')
                elif self.path == f'/project/{PROJECT}/doc/{DOCUMENT}/download':
                    fixture.document_requested.set()
                    with fixture._lock:
                        source = fixture._source
                    self.reply(200, source.encode(), 'text/plain; charset=utf-8')
                elif self.path == f'/project/{PROJECT}/file/{ASSET}':
                    fixture.asset_requested.set()
                    if not fixture.release_assets.wait(25):
                        self.reply(504, b'Asset release timed out', 'text/plain')
                    else:
                        self.reply(200, ASSET_BYTES, 'application/octet-stream')
                else:
                    self.reply(404, b'Not found', 'text/plain')

            def do_POST(self):
                length = int(self.headers.get('Content-Length', '0'))
                if self.path != '/fixture/source' or not 0 <= length <= 65536:
                    self.reply(400, b'Invalid fixture write', 'text/plain')
                    return
                source = self.rfile.read(length).decode('utf-8')
                with fixture._lock:
                    fixture._source = source
                    fixture._save_count += 1
                self.reply(200, b'OK', 'text/plain')

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, name='overleaf-storage-fixture', daemon=True)

    @property
    def url(self):
        return f'http://127.0.0.1:{self.server.server_port}/project/{PROJECT}'

    @property
    def source(self):
        with self._lock:
            return self._source

    @property
    def save_count(self):
        """Count only real bridge-originated HTTP writes, not collaborator edits."""
        with self._lock:
            return self._save_count

    def replace_remote(self, source):
        """A collaborator edits the fixture server while this client is offline."""
        if not isinstance(source, str) or len(source.encode('utf-8')) > 65536:
            raise ValueError('Fixture source must fit the bounded document endpoint')
        with self._lock:
            self._source = source

    def lose_connection(self):
        """Break the frontend's real loopback transport without a native command."""
        if not self._connection_events:
            raise ValueError('Connection-loss transport is not enabled')
        with self._condition:
            self._connection_generation += 1
            self._condition.notify_all()
            return self._connection_generation

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.release_assets.set()
        with self._condition:
            self._stopping = True
            self._condition.notify_all()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)

    def javascript(self):
        tree = dict(_id=PROJECT, name='Offline storage fixture', rootDoc_id=DOCUMENT,
                    rootFolder=[dict(_id=ROOT, name='', docs=[dict(_id=DOCUMENT, name='main.txt')],
                                     fileRefs=[dict(_id=ASSET, name='fixture.bin')], folders=[])])
        # No fixture code calls window.ipc or tiptoptypOverleaf.command. Native
        # commands arrive only through the real app, bridge, and WebView.
        with self._lock:
            generation = self._connection_generation
        connection_watch = r'''let connected = true;
function watchConnection() {
  fetch('/fixture/connection?generation=%s').then(response => {
    if (!connected) return;
    if (!response.ok) { connected = false; emitServer('disconnect'); return; }
    watchConnection();
  }).catch(() => { if (connected) { connected = false; emitServer('disconnect'); } });
}
watchConnection();''' % generation if self._connection_events else ''
        return r'''(() => {
'use strict';
const MAIN = %s, TREE = %s;
let text = %s, version = 1;
const listeners = new Map(), values = new Map(), watchers = new Map();
function emitServer(event, ...args) { for (const fn of listeners.get(event) || []) fn(...args); }
function install(source) { return {doc: {toString: () => source}, selection: {main: {anchor: 0, head: 0}}}; }
const socket = {
  publicId: 'storage-fixture-own-client',
  on(event, fn) { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn); },
  removeListener(event, fn) { listeners.get(event)?.delete(fn); },
  disconnect() { emitServer('disconnect'); },
  emit(event, ...args) {
    const callback = args.at(-1);
    if (event === 'clientTracking.getConnectedUsers' && typeof callback === 'function') callback(null, []);
    if (event === 'joinDoc' && typeof callback === 'function') callback(null, [], 0, [], null, null, {canSkipLeaveDoc: true});
  }
};
const view = {
  state: install(text),
  setState(state) { this.state = state; },
  dispatch(transaction) {
    if (transaction.selection) { this.state.selection = {main: transaction.selection}; return; }
    const {from, to, insert} = transaction.changes;
    text = text.slice(0, from) + insert + text.slice(to);
    this.state = install(text);
    socket.emit('applyOtUpdate', MAIN, {});
    fetch('/fixture/source', {method: 'POST', body: text}).then(response => {
      if (!response.ok) throw new Error('Fixture write failed');
      emitServer('otUpdateApplied', {doc: MAIN, v: version++});
      window.dispatchEvent(new CustomEvent('doc:saved', {detail: {id: MAIN}}));
    }).catch(() => emitServer('otUpdateError'));
  }
};
values.set('editor.view', view); values.set('editor.open_doc_id', MAIN);
window.overleaf = {unstable: {store: {
  get(key) { return values.get(key); },
  watch(key, fn) { if (!watchers.has(key)) watchers.set(key, new Set()); watchers.get(key).add(fn); return () => watchers.get(key).delete(fn); }
}}};
window.io = {};
window.io.connect = () => socket;
window.io.connect();
%s
setTimeout(() => {
  emitServer('joinProjectResponse', {project: TREE, permissionsLevel: 'owner', protocolVersion: 2, publicId: socket.publicId});
  socket.emit('clientTracking.getConnectedUsers', () => {});
  socket.emit('joinDoc', MAIN, () => {});
}, 0);
})();
''' % (json.dumps(DOCUMENT), json.dumps(tree), json.dumps(self.source), connection_watch)
