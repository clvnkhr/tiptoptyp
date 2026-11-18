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
SECOND_DOCUMENT = 'e' * 24
PREVIEW_DOCUMENT = 'f' * 24
PEER = 'fixture-peer'
SECOND_SOURCE = ''.join(f'Collaborator line {line:03d}. αβ\n' for line in range(1, 161))
SOURCE = 'Fresh server-owned storage fixture.\n'
ASSET_BYTES = b'overleaf storage fixture asset\x00\xff'


class StorageFixture:
    def __init__(self, *, connection_events=False, presence=False, file_actions=False, lazy_startup=False,
                 preview_source=None,
                 project_name='Offline storage fixture'):
        if not isinstance(project_name, str) or not 1 <= len(project_name) <= 256:
            raise ValueError('The fixture project name must contain 1 to 256 characters')
        self.project_name = project_name
        if (preview_source is not None and
                (not isinstance(preview_source, str) or not 1 <= len(preview_source.encode('utf-8')) <= 65536)):
            raise ValueError('The fixture preview source must contain 1 to 65536 bytes')
        self._preview_source = preview_source
        self.asset_requested = threading.Event()
        self.release_assets = threading.Event()
        self.document_requested = threading.Event()
        self.connection_requested = threading.Event()
        self.page_requested = threading.Event()
        self.presence_requested = threading.Event()
        self.editor_chunk_requested = threading.Event()
        self.release_editor_chunk = threading.Event()
        self._lazy_startup = lazy_startup
        self._lock = threading.Lock()
        self._condition = threading.Condition(self._lock)
        self._connection_events = connection_events
        self._connection_generation = 0
        self._connection_navigation = None
        self._presence_enabled = presence
        self._presence_generation = 0
        self._peer = None
        self._stopping = False
        self._source = SOURCE
        self._save_count = 0
        self.requests = []
        self.files = None
        if file_actions:
            from overleaf_files_fixture import FileProject
            self.files = FileProject(preview_source=preview_source)
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
                if fixture.files is not None and fixture.files.get(self):
                    return
                if self.path.split('?')[0] == '/project/' + PROJECT:
                    self.reply(200, b'<!doctype html><meta charset="utf-8"><title>Storage fixture</title>'
                               b'<p>Offline Overleaf storage fixture</p><script src="/fixture.js"></script>', 'text/html')
                elif self.path == '/login':
                    fixture.page_requested.set()
                    self.reply(200, b'<!doctype html><meta charset="utf-8"><title>Fixture sign-in</title>'
                               b'<p>Fixture browser left the project.</p>', 'text/html')
                elif self.path == '/browse':
                    fixture.page_requested.set()
                    self.reply(200, b'<!doctype html><meta charset="utf-8"><title>Browser input fixture</title>'
                               b'<input id="browser-input" aria-label="Browser typing fixture" '
                               b'style="position:fixed;left:16px;top:16px;width:300px;height:28px;box-sizing:border-box">'
                               b'<script>document.getElementById("browser-input").addEventListener("input",event=>{'
                               b'fetch("/fixture/browser-input?text="+encodeURIComponent(event.target.value)).catch(()=>{});'
                               b'});</script>', 'text/html')
                elif self.path.startswith('/fixture/browser-input?text='):
                    self.reply(204, b'', 'text/plain')
                elif self.path == '/fixture.js':
                    self.reply(200, fixture.javascript().encode(), 'application/javascript')
                elif fixture._lazy_startup and self.path == '/fixture/editor-chunk.js':
                    fixture.editor_chunk_requested.set()
                    if not fixture.release_editor_chunk.wait(25):
                        self.reply(504, b'Editor chunk release timed out', 'text/plain')
                    else:
                        self.reply(200, b'window.fixtureMountEditor();', 'application/javascript')
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
                        current = fixture._connection_generation
                        navigation = (fixture._connection_navigation
                                      if generation != current else None)
                        connected = not fixture._stopping and (generation == current or navigation is not None)
                    if connected:
                        event = dict(generation=current)
                        if navigation is not None:
                            event['navigate'] = navigation
                        self.reply(200, json.dumps(event).encode(), 'application/json')
                    else:
                        self.reply(503, b'Connection lost', 'text/plain')
                elif fixture._presence_enabled and self.path.startswith('/fixture/presence?generation='):
                    value = self.path.removeprefix('/fixture/presence?generation=')
                    if not value.isascii() or not value.isdigit() or len(value) > 9:
                        self.reply(400, b'Invalid presence generation', 'text/plain')
                        return
                    fixture.presence_requested.set()
                    with fixture._condition:
                        fixture._condition.wait_for(lambda:
                            fixture._stopping or int(value) != fixture._presence_generation,
                            timeout=20)
                        event = dict(generation=fixture._presence_generation, peer=fixture._peer)
                    self.reply(200, json.dumps(event).encode(), 'application/json')
                elif fixture._presence_enabled and self.path == f'/project/{PROJECT}/doc/{SECOND_DOCUMENT}/download':
                    self.reply(200, SECOND_SOURCE.encode(), 'text/plain; charset=utf-8')
                elif fixture._preview_source is not None and self.path == f'/project/{PROJECT}/doc/{PREVIEW_DOCUMENT}/download':
                    self.reply(200, fixture._preview_source.encode(), 'text/plain; charset=utf-8')
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

            def do_DELETE(self):
                if fixture.files is not None:
                    fixture.files.mutate(self)
                else:
                    self.reply(404, b'Not found', 'text/plain')

            def do_POST(self):
                if fixture.files is not None:
                    fixture.files.mutate(self)
                    return
                length = int(self.headers.get('Content-Length', '0'))
                preview = fixture._preview_source is not None and self.path == '/fixture/source/' + PREVIEW_DOCUMENT
                if (self.path != '/fixture/source' and not preview) or not 0 <= length <= 65536:
                    self.reply(400, b'Invalid fixture write', 'text/plain')
                    return
                source = self.rfile.read(length).decode('utf-8')
                with fixture._lock:
                    if preview:
                        fixture._preview_source = source
                    else:
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
    def departure_url(self):
        # A second name for this same loopback server changes origin, so the
        # production project's injected bridge cannot itself report loading or
        # disconnecting and conceal a broken native page-departure guard.
        return f'http://localhost:{self.server.server_port}/login'

    @property
    def browsing_url(self):
        return f'http://localhost:{self.server.server_port}/browse'

    @property
    def project_loads(self):
        """Actual main-frame project requests, including deep-link reloads."""
        with self._lock:
            return sum(request.split('?')[0] == '/project/' + PROJECT for request in self.requests)

    @property
    def source(self):
        if self.files is not None:
            return self.files.source(DOCUMENT)
        with self._lock:
            return self._source

    @property
    def save_count(self):
        """Count only real bridge-originated HTTP writes, not collaborator edits."""
        if self.files is not None:
            return self.files.save_count
        with self._lock:
            return self._save_count

    def replace_remote(self, source):
        """A collaborator edits the fixture server while this client is offline."""
        if not isinstance(source, str) or len(source.encode('utf-8')) > 65536:
            raise ValueError('Fixture source must fit the bounded document endpoint')
        with self._lock:
            self._source = source

    def move_peer(self, document, row, column):
        """Publish real caret-only clientTracking data over the fixture transport."""
        sources = {DOCUMENT: self.source, SECOND_DOCUMENT: SECOND_SOURCE}
        if (not self._presence_enabled or document not in sources
                or type(row) is not int or type(column) is not int or row < 0 or column < 0):
            raise ValueError('Invalid fixture peer position')
        lines = sources[document].split('\n')
        if row >= len(lines) or column > len(lines[row].encode('utf-16-le')) // 2:
            raise ValueError('Fixture peer position is outside the document')
        with self._condition:
            self._peer = dict(id=PEER, user_id='fixture-user', name='Fixture Collaborator',
                              doc_id=document, row=row, column=column)
            self._presence_generation += 1
            self._condition.notify_all()

    def remove_peer(self):
        if not self._presence_enabled:
            raise ValueError('Fixture presence transport is not enabled')
        with self._condition:
            self._peer = None
            self._presence_generation += 1
            self._condition.notify_all()

    def lose_connection(self):
        """Break the frontend's real loopback transport without a native command."""
        if not self._connection_events:
            raise ValueError('Connection-loss transport is not enabled')
        with self._condition:
            self._connection_navigation = None
            self._connection_generation += 1
            self._condition.notify_all()
            return self._connection_generation

    def depart_page(self):
        """Navigate the actual main frame without a preceding socket disconnect."""
        if not self._connection_events:
            raise ValueError('Page-departure transport is not enabled')
        self.page_requested.clear()
        with self._condition:
            self._connection_navigation = self.departure_url
            self._connection_generation += 1
            self._condition.notify_all()
            return self._connection_generation

    def restore_page(self):
        """Permit a fresh project load after its native Reconnect button is used."""
        if not self._connection_events:
            raise ValueError('Page-departure transport is not enabled')
        with self._condition:
            self._connection_navigation = None
            self._condition.notify_all()

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.release_assets.set()
        self.release_editor_chunk.set()
        if self.files is not None:
            self.files.stop()
        with self._condition:
            self._stopping = True
            self._condition.notify_all()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)

    def javascript(self):
        if self.files is not None:
            return self.files.javascript()
        documents = [dict(_id=DOCUMENT, name='main.txt')]
        sources = [[DOCUMENT, 'main.txt', self.source]]
        if self._presence_enabled:
            documents.append(dict(_id=SECOND_DOCUMENT, name='other.txt'))
            sources.append([SECOND_DOCUMENT, 'other.txt', SECOND_SOURCE])
        if self._preview_source is not None:
            documents.append(dict(_id=PREVIEW_DOCUMENT, name='preview.typ'))
            sources.append([PREVIEW_DOCUMENT, 'preview.typ', self._preview_source])
        tree = dict(_id=PROJECT, name=self.project_name, rootDoc_id=DOCUMENT,
                    rootFolder=[dict(_id=ROOT, name='', docs=documents,
                                     fileRefs=[dict(_id=ASSET, name='fixture.bin')], folders=[])])
        # No fixture code calls window.ipc or tiptoptypOverleaf.command. Native
        # commands arrive only through the real app, bridge, and WebView.
        with self._lock:
            generation = self._connection_generation
            presence_generation, peer = self._presence_generation, self._peer
        connection_watch = r'''let connected = true;
function watchConnection() {
  fetch('/fixture/connection?generation=%s').then(response => {
    if (!connected) return;
    if (!response.ok) { connected = false; emitServer('disconnect'); return; }
    return response.json().then(event => {
      if (!connected) return;
      if (event.navigate === DEPARTURE_URL) {
        departing = true; connected = false;
        window.location.assign(DEPARTURE_URL);
        return;
      }
      watchConnection();
    });
  }).catch(() => { if (connected) { connected = false; emitServer('disconnect'); } });
}
watchConnection();''' % generation if self._connection_events else ''
        presence_watch = r'''let presenceGeneration = %s;
let fixturePeer = %s;
function watchPresence() {
  fetch('/fixture/presence?generation=' + presenceGeneration).then(response => response.json()).then(event => {
    presenceGeneration = event.generation;
    if (event.peer) emitServer('clientTracking.clientUpdated', event.peer);
    else emitServer('clientTracking.clientDisconnected', %s);
    fixturePeer = event.peer;
    watchPresence();
  }).catch(() => {});
}
watchPresence();
''' % (presence_generation, json.dumps(peer), json.dumps(PEER)) if self._presence_enabled else 'let fixturePeer = null;'
        tree_source = r'''const documentSources = new Map(%s.map(([id, _name, source]) => [id, source]));
function mountTree() {
for (const [id, name] of %s) {
  const node = document.createElement('button');
  node.setAttribute('role', 'treeitem'); node.dataset.fileId = id;
  node.textContent = name;
  node.onclick = () => {
    values.set('editor.open_doc_id', id);
    view.setState(install(id === MAIN ? text : documentSources.get(id)));
    for (const fn of watchers.get('editor.open_doc_id') || []) fn(id);
    for (const fn of watchers.get('editor.view') || []) fn(view);
    socket.emit('joinDoc', id, () => {});
    window.dispatchEvent(new CustomEvent('doc:after-opened', {detail: {docId: id}}));
  };
  document.body.appendChild(node);
}
}
''' % (json.dumps(sources), json.dumps(sources)) if (self._presence_enabled or self._preview_source is not None) else 'const documentSources = new Map(); function mountTree() {}'
        return r'''(() => {
'use strict';
const MAIN = %s, TREE = %s, DEPARTURE_URL = %s;
let text = %s, version = 1, departing = false;
const listeners = new Map(), values = new Map(), watchers = new Map();
function emitServer(event, ...args) { for (const fn of listeners.get(event) || []) fn(...args); }
function install(source) { return {doc: {toString: () => source}, selection: {main: {anchor: 0, head: 0}}}; }
const socket = {
  publicId: 'storage-fixture-own-client',
  on(event, fn) { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn); },
  removeListener(event, fn) { listeners.get(event)?.delete(fn); },
  // A departing page deliberately cannot manufacture a disconnect event,
  // including when the production bridge handles pagehide during navigation.
  disconnect() { if (!departing) emitServer('disconnect'); },
  emit(event, ...args) {
    const callback = args.at(-1);
    if (event === 'clientTracking.getConnectedUsers' && typeof callback === 'function') callback(null, fixturePeer ? [{
      client_id: fixturePeer.id, user_id: fixturePeer.user_id, first_name: fixturePeer.name,
      cursorData: {doc_id: fixturePeer.doc_id, row: fixturePeer.row, column: fixturePeer.column}
    }] : []);
    if (event === 'joinDoc' && typeof callback === 'function') callback(null, [], 0, [], null, null, {canSkipLeaveDoc: true});
  }
};
const view = {
  state: install(text),
  setState(state) { this.state = state; },
  dispatch(transaction) {
    if (transaction.selection) { this.state.selection = {main: transaction.selection}; return; }
    const id = values.get('editor.open_doc_id');
    const {from, to, insert} = transaction.changes;
    const old = this.state.doc.toString(), next = old.slice(0, from) + insert + old.slice(to);
    if (id === MAIN) text = next;
    else documentSources.set(id, next);
    this.state = install(next);
    socket.emit('applyOtUpdate', id, {});
    fetch(id === MAIN ? '/fixture/source' : '/fixture/source/' + id, {method: 'POST', body: next}).then(response => {
      if (!response.ok) throw new Error('Fixture write failed');
      emitServer('otUpdateApplied', {doc: id, v: version++});
      window.dispatchEvent(new CustomEvent('doc:saved', {detail: {id}}));
    }).catch(() => emitServer('otUpdateError'));
  }
};
function mountEditor() {
  if (departing) return;
  values.set('editor.view', view); values.set('editor.open_doc_id', MAIN);
  for (const fn of watchers.get('editor.view') || []) fn(view);
  for (const fn of watchers.get('editor.open_doc_id') || []) fn(MAIN);
  mountTree();
  window.dispatchEvent(new CustomEvent('doc:after-opened', {detail: {docId: MAIN}}));
}
window.overleaf = {unstable: {store: {
  get(key) { return values.get(key); },
  watch(key, fn) { if (!watchers.has(key)) watchers.set(key, new Set()); watchers.get(key).add(fn); return () => watchers.get(key).delete(fn); }
}}};
window.io = {};
window.io.connect = () => socket;
window.io.connect();
%s
%s
%s
const lazyStartup = %s;
if (lazyStartup) {
  // The hosted IDE consumes its document query before its lazy source bundle
  // mounts. An eager bridge deep link would therefore reload this page again.
  const address = new URL(window.location.href);
  address.searchParams.delete('doc');
  window.history.replaceState(null, '', address.toString());
  window.fixtureMountEditor = () => { delete window.fixtureMountEditor; mountEditor(); };
} else mountEditor();
setTimeout(() => {
  emitServer('joinProjectResponse', {project: TREE, permissionsLevel: 'owner', protocolVersion: 2, publicId: socket.publicId});
  socket.emit('clientTracking.getConnectedUsers', () => {});
  socket.emit('joinDoc', MAIN, () => {});
  if (lazyStartup) {
    const chunk = document.createElement('script');
    chunk.src = '/fixture/editor-chunk.js';
    document.head.appendChild(chunk);
  }
}, 0);
})();
''' % (json.dumps(DOCUMENT), json.dumps(tree), json.dumps(self.departure_url),
       json.dumps(self.source), connection_watch, presence_watch, tree_source, json.dumps(self._lazy_startup))
