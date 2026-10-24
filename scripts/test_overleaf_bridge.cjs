'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');
const bridge = fs.readFileSync(path.join(__dirname, '../src/overleaf_bridge.js'), 'utf8');
const id = char => char.repeat(24);
const PID = id('a'), ROOT = id('b'), MAIN = id('c'), OTHER = id('d'), FOLDER = id('e'), ASSET = id('f');
function harness(options = {}) {
  const messages = [], events = new Map(), timers = new Map(), socketEvents = new Map();
  const watches = new Map(), requests = [], dispatches = [], nodes = new Map();
  let nextTimer = 0, inflight = 0, maxInflight = 0;
  const sources = new Map([[MAIN, 'Main source.'], [OTHER, 'Other source.'], [ASSET, new Uint8Array([0, 255, 64])]]);
  const values = new Map();
  const storageWrites = new Map();
  const socket = {
    on(event, handler) { if (!socketEvents.has(event)) socketEvents.set(event, new Set()); socketEvents.get(event).add(handler); },
    removeListener(event, handler) { socketEvents.get(event)?.delete(handler); },
    emit(event, ...args) { if (options.emit) return options.emit(event, args); },
    disconnect() { server('disconnect'); },
  };
  function server(event, ...args) { for (const handler of socketEvents.get(event) || []) handler(...args); }
  function trigger(event, detail) { for (const handler of events.get(event) || []) handler({ detail }); }
  function set(key, value) { values.set(key, value); for (const handler of watches.get(key) || []) handler(value); }
  const view = {
    state: { doc: { toString: () => 'Main source.' } },
    setState(state) { this.state = state; },
    dispatch(value) {
      dispatches.push(value);
      const { from, to, insert } = value.changes;
      const text = this.state.doc.toString();
      this.state = { doc: { toString: () => text.slice(0, from) + insert + text.slice(to) } };
      socket.emit('applyOtUpdate', values.get('editor.open_doc_id'), {});
    },
  };
  values.set('editor.view', view); values.set('editor.open_doc_id', MAIN);
  const window = {
    tiptoptypOverleafEpoch: 7,
    ipc: { postMessage(value) { messages.push(JSON.parse(value)); } },
    localStorage: { setItem(key, value) { if (options.storageDenied) throw new Error('Storage disabled'); storageWrites.set(key, value); } },
    location: { pathname: options.pathname || '/project/' + PID, href: 'https://www.overleaf.com/project/' + PID, reload() { window.reloaded = true; }, assign(value) { window.navigated = value; } },
    overleaf: { unstable: { store: {
      get(key) { return values.get(key); },
      watch(key, fn) { if (!watches.has(key)) watches.set(key, new Set()); watches.get(key).add(fn); return () => watches.get(key).delete(fn); },
    } } },
    addEventListener(event, fn) { if (!events.has(event)) events.set(event, new Set()); events.get(event).add(fn); },
    removeEventListener(event, fn) { events.get(event)?.delete(fn); },
  };
  window.top = window;
  const context = vm.createContext({
    window, document: { querySelector(selector) { const match = selector.match(/data-file-id="([a-f0-9]+)"/); const node = nodes.get(match?.[1]); return node ? { closest() { return node; } } : null; } },
    setTimeout(fn, delay) { const timer = ++nextTimer; timers.set(timer, { fn, delay }); return timer; },
    clearTimeout(timer) { timers.delete(timer); },
    URL, URLSearchParams, TextDecoder, TextEncoder, Uint8Array, AbortController,
    btoa(value) { return Buffer.from(value, 'binary').toString('base64'); },
    async fetch(url, init) {
      requests.push({ url, init });
      inflight++; maxInflight = Math.max(maxInflight, inflight);
      try {
        if (options.fetch) return await options.fetch(url, init, sources);
        const entity = url.match(/\/(?:doc|file)\/([a-f0-9]{24})/)[1];
        const value = sources.get(entity);
        const bytes = typeof value === 'string' ? new TextEncoder().encode(value) : value;
        let used = false;
        return { ok: value !== undefined, headers: { get() { return String(bytes?.byteLength || 0); } }, body: { getReader() { return { async read() { if (used) return { done: true }; used = true; return { done: false, value: bytes }; }, async cancel() {} }; } } };
      } finally { inflight--; }
    },
  });
  vm.runInContext(bridge, context, { filename: 'overleaf_bridge.js' });
  window.io = { connect() { return socket; } };
  window.io.connect();
  const tree = { _id: PID, name: 'Paper', rootDoc_id: MAIN, rootFolder: [{ _id: ROOT, name: '', docs: [{ _id: MAIN, name: 'main.tex' }], fileRefs: [{ _id: ASSET, name: 'figure.png' }], folders: [{ _id: FOLDER, name: 'sections', docs: [{ _id: OTHER, name: 'intro.tex' }], fileRefs: [], folders: [] }] }] };
  function project(permissionsLevel = 'owner', value = tree) { server('joinProjectResponse', { project: value, permissionsLevel, protocolVersion: 2 }); }
  function command(value) { window.tiptoptypOverleaf.command({ epoch: 7, ...value }); }
  function flush(delay = 0) {
    for (let count = 0; count < 100; count++) {
      const next = [...timers].find(([, timer]) => timer.delay <= delay);
      if (!next) return;
      timers.delete(next[0]); next[1].fn();
    }
    throw new Error('Timer loop did not settle');
  }
  async function settle() { for (let i = 0; i < 25; i++) await Promise.resolve(); flush(); }
  return { window, messages, events, timers, socketEvents, watches, storageWrites, socket, server, trigger, set, view, project, command, flush, settle, sources, requests, dispatches, nodes, tree, maxInflight: () => maxInflight };
}
const ofType = (h, event) => h.messages.filter(message => message.event === event);

test('official project payload hydrates nested text and binary files with bounded concurrency', async () => {
  const h = harness(); h.project(); await h.settle();
  const project = ofType(h, 'project')[0];
  assert.equal(project.epoch, 7); assert.equal(project.project_id, PID);
  assert.deepEqual(project.entries.map(entry => entry.path).sort(), ['figure.png', 'main.tex', 'sections', 'sections/intro.tex']);
  assert.equal(ofType(h, 'document').length, 2);
  assert.equal(ofType(h, 'file')[0].base64, 'AP9A');
  assert.ok(h.maxInflight() <= 2);
  assert.equal([...h.timers.values()].filter(timer => timer.delay === 250).length, 0, 'no idle boot poll after store attached');
});
test('exact base check rejects a concurrent remote change without dispatch', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Stale source.', source: 'Overwrite.', request: 1 });
  assert.equal(h.dispatches.length, 0);
  assert.equal(ofType(h, 'conflict')[0].source, 'Main source.');
});
test('save requires official doc:saved and acknowledges transformed canonical source', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'New main.', request: 2 });
  assert.equal(h.dispatches.length, 1); assert.equal(ofType(h, 'ack').length, 0);
  h.trigger('doc:saved', { id: OTHER }); assert.equal(ofType(h, 'ack').length, 0);
  h.view.state = { doc: { toString: () => 'New main. Collaborator.' } };
  h.trigger('doc:changed', { id: MAIN }); h.flush();
  assert.equal(ofType(h, 'document').length, 0, 'optimistic browser state is never a server baseline');
  h.trigger('doc:saved', { id: MAIN });
  assert.equal(ofType(h, 'ack').length, 0, 'a delayed saved notification cannot replace the server ACK');
  h.server('otUpdateApplied', { doc: MAIN, v: 12 });
  h.trigger('doc:saved', { id: MAIN });
  assert.deepEqual(ofType(h, 'ack'), [{ event: 'ack', epoch: 7, id: MAIN, request: 2, source: 'New main. Collaborator.' }]);
});
test('a second native edit cannot replace a pending write', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'New main.', request: 3 });
  h.command({ command: 'edit', id: MAIN, base: 'New main.', source: 'Another.', request: 4 });
  assert.equal(h.dispatches.length, 1); assert.equal(ofType(h, 'conflict').at(-1).request, 4);
});
test('surrogate pairs remain intact in the minimal CodeMirror transaction', async () => {
  const h = harness(); h.project(); await h.settle();
  h.view.state = { doc: { toString: () => 'A😀B' } };
  h.command({ command: 'edit', id: MAIN, base: 'A😀B', source: 'A😁B', request: 5 });
  const change = h.dispatches[0].changes;
  assert.equal(change.from, 1); assert.equal(change.to, 3); assert.equal(change.insert, '😁');
});
test('read-only permissions reject edits before browser mutation', async () => {
  const h = harness(); h.project('readOnly'); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Forbidden.', request: 6 });
  assert.equal(h.dispatches.length, 0); assert.match(ofType(h, 'conflict').at(-1).message, /read-only/);
});
test('stale epoch and mismatched remote paths cannot activate another file', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', epoch: 6, id: MAIN, base: 'Main source.', source: 'No.', request: 7 });
  h.command({ command: 'open', id: OTHER, path: 'wrong.tex' });
  assert.equal(h.dispatches.length, 0); assert.equal(h.window.navigated, undefined);
  assert.match(ofType(h, 'error').at(-1).message, /no longer in the project/);
});
test('document activation waits for the new CodeMirror state, not just changed ID', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  h.nodes.set(FOLDER, { getAttribute() { return 'true'; }, click() {} });
  h.nodes.set(OTHER, { click() { h.set('editor.open_doc_id', OTHER); h.trigger('doc:after-opened', { docId: OTHER }); } });
  h.command({ command: 'open', id: OTHER, path: 'sections/intro.tex' }); h.flush();
  assert.equal(ofType(h, 'document').length, 0, 'old text must not be labelled as the newly selected file');
  h.view.setState({ doc: { toString: () => 'Other source.' } }); await h.settle();
  assert.ok(h.requests.some(value => value.url.endsWith('/doc/' + OTHER + '/download')));
  assert.equal(ofType(h, 'document').at(-1).source, 'Other source.');
  assert.equal(ofType(h, 'document').at(-1).id, OTHER);
});
test('unmounted nested document uses official same-project doc deep link', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'open', id: OTHER, path: 'sections/intro.tex' }); h.flush();
  assert.equal(new URL(h.window.navigated).searchParams.get('doc'), OTHER);
  assert.equal(new URL(h.window.navigated).origin, 'https://www.overleaf.com');
});
test('nonactive collaborator updates refresh a server snapshot', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  h.sources.set(OTHER, 'Remote correction.'); h.server('otUpdateApplied', { doc: OTHER, v: 10 }); await h.settle();
  assert.equal(ofType(h, 'document').at(-1).source, 'Remote correction.');
});
test('folder renames update descendants and deleted files leave the authoritative tree', async () => {
  const h = harness(); h.project(); await h.settle();
  h.server('reciveEntityRename', FOLDER, 'chapters'); await h.settle();
  assert.ok(ofType(h, 'project').at(-1).entries.some(entry => entry.path === 'chapters/intro.tex'));
  h.server('removeEntity', FOLDER);
  assert.ok(!ofType(h, 'project').at(-1).entries.some(entry => entry.id === OTHER));
});
test('invalid paths and cross-project payloads fail closed', async () => {
  const h = harness(); const malformed = structuredClone(h.tree); malformed.rootFolder[0].docs[0].name = '../escape.tex';
  h.project('owner', malformed); assert.equal(ofType(h, 'project').length, 0); assert.ok(ofType(h, 'status').some(value => value.state === 'error'));
  const wrong = structuredClone(h.tree); wrong._id = id('1'); h.project('owner', wrong);
  assert.equal(ofType(h, 'project').length, 0);
});
test('disconnect cleans event subscriptions and all bridge timers', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Waiting.', request: 8 });
  h.command({ command: 'disconnect' });
  assert.equal(h.timers.size, 0);
  assert.equal([...h.watches.values()].reduce((sum, set) => sum + set.size, 0), 0);
  assert.equal([...h.events.values()].reduce((sum, set) => sum + set.size, 0), 0);
});

test('an in-flight old download cannot overwrite a newer collaborator revision', async () => {
  let release;
  let held = true;
  const h = harness({ async fetch(url, init, sources) {
    const entity = url.match(/\/(?:doc|file)\/([a-f0-9]{24})/)[1];
    const snapshot = sources.get(entity);
    if (entity === OTHER && held) { held = false; await new Promise(resolve => { release = resolve; }); }
    const bytes = typeof snapshot === 'string' ? new TextEncoder().encode(snapshot) : snapshot;
    let used = false;
    return { ok: true, headers: { get() { return String(bytes.byteLength); } }, body: { getReader() { return { async read() { if (used) return { done: true }; used = true; return { done: false, value: bytes }; }, async cancel() {} }; } } };
  } });
  h.project(); await h.settle();
  assert.equal(typeof release, 'function');
  h.sources.set(OTHER, 'Newest remote source.');
  h.server('otUpdateApplied', { doc: OTHER, v: 11 });
  release(); await h.settle();
  const snapshots = ofType(h, 'document').filter(value => value.id === OTHER);
  assert.deepEqual(snapshots.map(value => value.source), ['Newest remote source.']);
});
test('failed downloads do not retry forever or start an idle polling loop', async () => {
  const h = harness({ async fetch() { return { ok: false }; } });
  h.project(); await h.settle();
  assert.equal(h.requests.length, 3); assert.equal(ofType(h, 'error').length, 3);
  assert.equal(h.timers.size, 0);
});
test('wrong active document returns a request-specific conflict without leaking other text', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: OTHER, base: 'Other source.', source: 'Local edit.', request: 9 }); h.flush();
  assert.equal(h.dispatches.length, 0);
  assert.deepEqual(ofType(h, 'conflict').at(-1), { event: 'conflict', epoch: 7, id: OTHER, request: 9, source: 'Other source.', message: 'Open this Overleaf document before saving it. Your local edit is retained.' });
});
test('UTF-8 source limit rejects multibyte input before optimistic dispatch', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: '漢'.repeat(1500000), request: 10 });
  assert.equal(h.dispatches.length, 0); assert.match(ofType(h, 'error').at(-1).message, /Invalid Overleaf edit/);
});

test('parked document save activates the official editor before checking its base', async () => {
  const h = harness(); h.project(); await h.settle();
  h.nodes.set(FOLDER, { getAttribute() { return 'true'; }, click() {} });
  h.nodes.set(OTHER, { click() { h.set('editor.open_doc_id', OTHER); h.trigger('doc:after-opened', { docId: OTHER }); } });
  h.command({ command: 'edit', id: OTHER, base: 'Other source.', source: 'Edited other.', request: 11 }); h.flush();
  assert.equal(h.dispatches.length, 0, 'old editor state must not accept the parked edit');
  h.view.setState({ doc: { toString: () => 'Other source.' } }); h.flush();
  assert.equal(h.dispatches.length, 1); assert.equal(h.view.state.doc.toString(), 'Edited other.');
  h.server('otUpdateApplied', { doc: OTHER, v: 1 }); h.trigger('doc:saved', { id: OTHER });
  assert.equal(ofType(h, 'ack').at(-1).request, 11);
});
test('remote update during parked document activation rejects the stale native base', async () => {
  const h = harness(); h.project(); await h.settle();
  h.nodes.set(FOLDER, { getAttribute() { return 'true'; }, click() {} });
  h.nodes.set(OTHER, { click() { h.set('editor.open_doc_id', OTHER); h.trigger('doc:after-opened', { docId: OTHER }); } });
  h.command({ command: 'edit', id: OTHER, base: 'Other source.', source: 'Stale local edit.', request: 12 }); h.flush();
  h.sources.set(OTHER, 'Collaborator changed this.');
  h.server('otUpdateApplied', { doc: OTHER, op: [{ p: 0, i: 'changed' }], v: 3 });
  h.view.setState({ doc: { toString: () => 'Collaborator changed this.' } }); h.flush();
  assert.equal(h.dispatches.length, 0);
  assert.equal(ofType(h, 'conflict').at(-1).source, 'Other source.', 'only previously server-confirmed text can be returned before the refresh completes');
  await h.settle();
  assert.equal(ofType(h, 'document').filter(value => value.id === OTHER).at(-1).source, 'Collaborator changed this.');
});
test('file clicks while saving keep only the latest requested file', async () => {
  const h = harness(); h.project(); await h.settle();
  let otherClicks = 0;
  h.nodes.set(OTHER, { click() { otherClicks++; } });
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Saved main.', request: 13 });
  h.command({ command: 'open', id: OTHER, path: 'sections/intro.tex' });
  h.command({ command: 'open', id: MAIN, path: 'main.tex' });
  assert.equal(otherClicks, 0);
  h.server('otUpdateApplied', { doc: MAIN, v: 2 }); h.trigger('doc:saved', { id: MAIN }); h.flush();
  assert.equal(otherClicks, 0); assert.equal(h.window.navigated, undefined);
});

test('official Socket.IO empty-global bootstrap is intercepted before any setup timer', async () => {
  const h = harness();
  const handlers = new Map();
  const freshSocket = {
    on(event, handler) { if (!handlers.has(event)) handlers.set(event, new Set()); handlers.get(event).add(handler); },
    removeListener(event, handler) { handlers.get(event)?.delete(handler); }, emit() {}, disconnect() {},
  };
  // The official dist/socket.io.js writes var io={}, then this.io={}, and only
  // afterward its IIFE populates exports.connect. The IDE immediately connects.
  h.window.io = {};
  const exports = (h.window.io = {});
  exports.version = '0.9.17-overleaf-6';
  exports.connect = function () { return freshSocket; };
  h.window.io.connect();
  assert.ok(handlers.has('joinProjectResponse'), 'the real socket must be observed synchronously');
  for (const handler of handlers.get('joinProjectResponse')) handler({ project: h.tree, permissionsLevel: 'owner', protocolVersion: 2 });
  await h.settle();
  assert.equal(ofType(h, 'project').at(-1).project_id, PID);
  assert.equal(ofType(h, 'document').length, 2);
});
test('late Socket.IO Manager assignment observes the official v2 shim connection', () => {
  const h = harness();
  const handlers = new Map();
  const freshSocket = { on(event, handler) { handlers.set(event, handler); }, removeListener() {}, emit() {}, disconnect() {} };
  h.window.io = {};
  h.window.io.Manager = function () { return { socket() { return freshSocket; } }; };
  h.window.io.Manager('https://www.overleaf.com', {}).socket('/', {});
  assert.equal(typeof handlers.get('joinProjectResponse'), 'function');
});

test('current project-wide document broadcast capability keeps full live sync', async () => {
  const originalReply = [null, ['content'], 42, [], {}, 'history-ot', { canSkipLeaveDoc: true }];
  let received;
  const h = harness({ emit(event, args) { if (event === 'joinDoc') args.at(-1)(...originalReply); } });
  h.project(); await h.settle();
  h.socket.emit('joinDoc', MAIN, { supportsHistoryOT: true }, (...reply) => { received = reply; });
  assert.deepEqual(received, originalReply, 'official editor callback receives its exact unmodified ACK');
  assert.ok(!ofType(h, 'status').some(value => /refresh to sync/.test(value.message || '')));
  h.sources.set(OTHER, 'Current-server background update.');
  h.server('otUpdateApplied', { doc: OTHER, op: [{ p: 0, i: 'x' }], v: 3 }); await h.settle();
  assert.equal(ofType(h, 'document').at(-1).source, 'Current-server background update.');
});
test('older doc-room servers give one explicit limitation warning without going offline', async () => {
  const reply = [null, ['content'], 3, [], {}, 'sharejs-text-ot'];
  const h = harness({ emit(event, args) { if (event === 'joinDoc') args.at(-1)(...reply); } });
  h.project(); await h.settle();
  let callbackCount = 0;
  h.socket.emit('joinDoc', MAIN, () => { callbackCount++; });
  h.socket.emit('joinDoc', MAIN, 3, { encodeRanges: true }, () => { callbackCount++; });
  const warnings = ofType(h, 'status').filter(value => /refresh to sync/.test(value.message || ''));
  assert.equal(warnings.length, 1); assert.equal(warnings[0].state, 'connected'); assert.equal(callbackCount, 2);
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Safe active-file save.', request: 14 });
  h.server('otUpdateApplied', { doc: MAIN, v: 3 }); h.trigger('doc:saved', { id: MAIN });
  assert.equal(ofType(h, 'ack').at(-1).request, 14, 'the active file remains editable and server-acknowledged');
});
test('a rejected document join does not claim a server broadcast capability', async () => {
  const h = harness({ emit(event, args) { if (event === 'joinDoc') args.at(-1)({ message: 'not authorized' }); } });
  h.project(); await h.settle();
  let received;
  h.socket.emit('joinDoc', MAIN, error => { received = error; });
  assert.equal(received.message, 'not authorized');
  assert.ok(!ofType(h, 'status').some(value => /refresh to sync/.test(value.message || '')));
});

test('headless auto-compile preference is browser-local and limited to the exact project key', () => {
  const h = harness();
  assert.deepEqual([...h.storageWrites], [['autocompile_enabled:' + PID, 'false']]);
  assert.equal(ofType(h, 'project').length, 0, 'preference is set before the official editor boot/join');
  assert.equal(h.requests.length, 0, 'no account preference or server mutation request');
});
test('disabled browser storage does not prevent project synchronization', async () => {
  const h = harness({ storageDenied: true }); h.project(); await h.settle();
  assert.equal(h.storageWrites.size, 0); assert.equal(ofType(h, 'document').length, 2);
});
test('authentication pages do not receive any project preference writes', () => {
  const h = harness({ pathname: '/login' });
  assert.equal(h.storageWrites.size, 0);
});

test('a later hosted-browser edit blocks a delayed saved event until every outgoing OT is acknowledged', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Native change.', request: 15 });
  h.sources.set(MAIN, 'Native change.'); h.server('otUpdateApplied', { doc: MAIN, v: 1 });
  const end = h.view.state.doc.toString().length;
  h.view.dispatch({ changes: { from: end, to: end, insert: ' Browser addition.' } });
  h.trigger('doc:changed', { id: MAIN });
  h.trigger('doc:saved', { id: MAIN }); await h.settle();
  assert.equal(ofType(h, 'ack').length, 0, 'the native request must wait for the later hosted write too');
  assert.equal(ofType(h, 'document').length, 0, 'optimistic hosted text must not become a server baseline');
  h.server('otUpdateApplied', { doc: MAIN, v: 1 }); h.trigger('doc:saved', { id: MAIN });
  assert.equal(ofType(h, 'ack').length, 0, 'duplicate prior ACK cannot decrement a newer outstanding send');
  h.sources.set(MAIN, 'Native change. Browser addition.');
  h.server('otUpdateApplied', { doc: MAIN, v: 2 }); h.trigger('doc:saved', { id: MAIN }); await h.settle();
  assert.deepEqual(ofType(h, 'ack'), [{ event: 'ack', epoch: 7, id: MAIN, request: 15, source: 'Native change. Browser addition.' }]);
  assert.ok(ofType(h, 'document').every(value => value.source === 'Native change. Browser addition.'));
});
test('unflushed hosted edits never publish a CodeMirror snapshot as an authoritative document', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  // The official client can debounce OT flushing after its CodeMirror state has
  // already changed. No actual applyOtUpdate emit has happened yet.
  h.view.state = { doc: { toString: () => 'Unsent hosted text.' } };
  h.trigger('doc:changed', { id: MAIN }); await h.settle();
  assert.equal(ofType(h, 'document').length, 0);
  h.sources.set(MAIN, 'Server-confirmed collaborator text.');
  h.server('otUpdateApplied', { doc: MAIN, op: [{ p: 0, i: 'remote' }], v: 8 }); await h.settle();
  assert.deepEqual(ofType(h, 'document').map(value => value.source), ['Server-confirmed collaborator text.']);
});
test('retried sends that cannot be reconciled never produce a false native acknowledgement', async () => {
  const h = harness(); h.project(); await h.settle();
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Retried edit.', request: 16 });
  h.socket.emit('applyOtUpdate', MAIN, { v: 0, dupIfSource: ['old-client'] });
  h.server('otUpdateApplied', { doc: MAIN, v: 0 }); h.trigger('doc:saved', { id: MAIN });
  h.server('otUpdateApplied', { doc: MAIN, v: 0 }); h.trigger('doc:saved', { id: MAIN });
  assert.equal(ofType(h, 'ack').length, 0, 'uncertain retry accounting requires reconnection, never assumed success');
});

test('CAS rejection of an unflushed hosted edit uses the authoritative baseline, never its optimistic text', async () => {
  const h = harness(); h.project(); await h.settle(); h.messages.length = 0;
  h.view.state = { doc: { toString: () => 'Hosted edit still waiting for a debounce.' } };
  h.trigger('doc:changed', { id: MAIN });
  h.command({ command: 'edit', id: MAIN, base: 'Main source.', source: 'Native draft retained.', request: 17 });
  assert.equal(h.dispatches.length, 0);
  const conflict = ofType(h, 'conflict').at(-1);
  assert.equal(conflict.request, 17); assert.equal(conflict.source, 'Main source.');
  await h.settle();
  assert.ok(ofType(h, 'document').every(value => value.source === 'Main source.'));
});
