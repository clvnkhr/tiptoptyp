/* The hosted editor owns authentication, operational transformation, and saving.
 * See docs/overleaf-protocol.md for the upstream interface and its limitations. */
(() => {
  'use strict';
  if (window.top !== window || window.tiptoptypOverleaf) return;
  // This is Overleaf's documented-by-source browser-local project preference,
  // isolated to our WebView profile; it never changes account settings.
  const routeProject = window.location.pathname.match(/^\/project\/([a-f0-9]{24})(?:\/|$)/);
  if (routeProject) {
    try { window.localStorage.setItem('autocompile_enabled:' + routeProject[1], 'false'); }
    catch (_) { /* Storage can be disabled; collaboration remains available. */ }
  }
  const MAX_SOURCE = 4 * 1024 * 1024;
  const MAX_FILE = 8 * 1024 * 1024;
  const MAX_MESSAGE = 16 * 1024 * 1024;
  const MAX_ENTRIES = 10000;
  const ID = /^[a-f0-9]{24}$/;
  let epoch = Number.isSafeInteger(window.tiptoptypOverleafEpoch) ? window.tiptoptypOverleafEpoch : 0;
  let socket = null;
  // One authoritative phase replaces the connection flags. A terminal phase
  // cannot be revived by a late socket/store callback from this bridge epoch.
  const LinkPhase = Object.freeze({
    AwaitingProject: 'awaiting-project', Hydrating: 'hydrating', Connected: 'connected',
    Incompatible: 'incompatible', Stopped: 'stopped',
  });
  let link = { phase: LinkPhase.AwaitingProject };
  function isStopped() { return link.phase === LinkPhase.Stopped; }
  function bridgeCompatibilityError() { return link.phase === LinkPhase.Incompatible ? link.message : null; }
  function isConnected() { return link.phase === LinkPhase.Connected; }
  function hasProject() { return link.phase === LinkPhase.Hydrating || isConnected(); }
  function transitionLink(phase) {
    if (isStopped() || bridgeCompatibilityError()) return;
    link = { phase };
  }
  let storeGeneration = 0;
  let storeAttachment = 'detached';
  let socketGeneration = 0;
  let socketAttachment = 'detached';
  let restoreSocketEmit = null;
  let viewHook = null;
  const methodRestorers = [];
  let projectWideUpdates = null;
  let warnedDocRooms = false;
  const DOC_ROOMS_WARNING = 'Connected · refresh to sync other files on this server';
  let project = null;
  let canEdit = false;
  let canManageFiles = false;
  let fileOperation = null;
  let fileAccess = 'ready';
  let treeRevision = 0;
  let downloadQueue = new Set();
  const documentRevision = new Map();
  const dirty = new Set();
  const outgoingOt = new Map();
  const lastOwnAckVersion = new Map();
  const MAX_OUTGOING_OT = 1024;
  const authoritativeSources = new Map();
  const MAX_AUTHORITATIVE_BYTES = 16 * 1024 * 1024;
  let authoritativeBytes = 0;
  const rootIds = new Set();
  let entries = new Map();
  let parents = new Map();
  let store = null;
  let pending = null;
  let opening = null;
  let queuedOpen = null;
  let queuedEdit = null;
  let bootTimer = null;
  let bootAttempts = 0;
  let bootstrap = { phase: 'ready' };
  const unwatch = [];
  const listeners = [];
  const socketListeners = [];
  const downloads = new Set();
  const downloadingIds = new Set();
  const MAX_PEERS = 128;
  const PRESENCE_INTERVAL = 100;
  const peers = new Map();
  const viewSources = new WeakMap();
  let ownId = null;
  let selfUserId = null;
  let presenceTimer = null;
  let cursorTimer = null;
  let queuedCursor = null;
  let lastOwnPosition = '';
  let lastPresence = '';
  let lastPresenceHadPeers = false;
  let trackingRequest = 0;
  let trackingChanges = null;

  function boundedIdentity(value) {
    if (typeof value !== 'string' || value.length === 0 || value.length > 128 || /[\u0000-\u001f\u007f]/.test(value)) return null;
    for (const character of value) if (character.length === 1 && /[\uD800-\uDFFF]/.test(character)) return null;
    return value;
  }
  function refreshOwnIdentity() {
    ownId = ownId || boundedIdentity(socket?.publicId);
    selfUserId = boundedIdentity(document.querySelector('meta[name="ol-user_id"]')?.content);
    if (ownId) peers.delete(ownId);
  }
  function sourceInfo(source) {
    return { source, fingerprint: null, checkpoints: [0], indexedRow: 0, indexedOffset: 0, indexedEnd: false, recentLine: null, lineBounds: new Map() };
  }
  function fingerprint(info) {
    if (info.fingerprint !== null) return info.fingerprint;
    // FNV-1a 64, using two 32-bit words rather than per-byte BigInt allocation.
    let high = 0xcbf29ce4, low = 0x84222325;
    for (const byte of new TextEncoder().encode(info.source)) {
      low = (low ^ byte) >>> 0;
      const product = low * 0x1b3;
      high = (Math.imul(high, 0x1b3) + Math.imul(low, 0x100) + Math.floor(product / 0x100000000)) >>> 0;
      low = product >>> 0;
    }
    info.fingerprint = high.toString(16).padStart(8, '0') + low.toString(16).padStart(8, '0');
    return info.fingerprint;
  }
  function advanceLine(info) {
    const next = info.source.indexOf('\n', info.indexedOffset);
    if (next < 0) { info.indexedEnd = true; return false; }
    info.indexedOffset = next + 1;
    info.indexedRow++;
    if (info.indexedRow % 128 === 0) info.checkpoints.push(info.indexedOffset);
    return true;
  }
  function validOffset(source, offset) {
    return Number.isSafeInteger(offset) && offset >= 0 && offset <= source.length && !(offset > 0 && offset < source.length && /[\uD800-\uDBFF]/.test(source[offset - 1]) && /[\uDC00-\uDFFF]/.test(source[offset]));
  }
  function lineBounds(info, row) {
    let bounds = info.lineBounds.get(row);
    if (!bounds) {
      const block = Math.floor(row / 128);
      let from = info.checkpoints[block];
      for (let currentRow = block * 128; currentRow < row; currentRow++) from = info.source.indexOf('\n', from) + 1;
      let end = info.source.indexOf('\n', from);
      if (end < 0) end = info.source.length;
      bounds = { row, from, end };
      info.lineBounds.set(row, bounds);
      if (info.lineBounds.size > 32) info.lineBounds.delete(info.lineBounds.keys().next().value);
    }
    return bounds;
  }
  function positionOffset(info, row, column) {
    if (!Number.isSafeInteger(row) || row < 0 || row > MAX_SOURCE || !Number.isSafeInteger(column) || column < 0 || column > MAX_SOURCE) return null;
    while (info.indexedRow < row && !info.indexedEnd) advanceLine(info);
    if (info.indexedRow < row) return null;
    const bounds = lineBounds(info, row);
    const offset = bounds.from + column;
    return offset <= bounds.end && validOffset(info.source, offset) ? offset : null;
  }
  function offsetPosition(info, offset) {
    const recent = info.recentLine;
    if (recent && offset >= recent.from && offset <= recent.end) return { row: recent.row, column: offset - recent.from };
    while (!info.indexedEnd && info.indexedOffset < offset) {
      const next = info.source.indexOf('\n', info.indexedOffset);
      if (next < 0) { info.indexedEnd = true; break; }
      if (next + 1 > offset) break;
      advanceLine(info);
    }
    let low = 0, high = info.checkpoints.length;
    while (low + 1 < high) {
      const middle = (low + high) >>> 1;
      if (info.checkpoints[middle] <= offset) low = middle; else high = middle;
    }
    let row = low * 128, start = info.checkpoints[low];
    while (true) {
      const next = info.source.indexOf('\n', start);
      if (next < 0 || next + 1 > offset) break;
      row++; start = next + 1;
    }
    info.recentLine = lineBounds(info, row);
    return { row, column: offset - start };
  }
  function currentSourceInfo() {
    const id = read('editor.open_doc_id'), view = read('editor.view'), doc = view?.state?.doc;
    if (!ID.test(id || '') || entries.get(id)?.kind !== 'doc' || !doc || typeof doc.toString !== 'function' || typeof view.dispatch !== 'function') return null;
    let info = viewSources.get(doc);
    if (!info) {
      const source = doc.toString();
      if (source.length > MAX_SOURCE || new TextEncoder().encode(source).byteLength > MAX_SOURCE) return null;
      info = sourceInfo(source); viewSources.set(doc, info);
    }
    return { id, view, info };
  }
  function normalizePeer(value, initial = false) {
    if (!value || typeof value !== 'object') return null;
    const id = boundedIdentity(initial ? value.client_id : value.id);
    if (!id || id === ownId) return null;
    const cursor = initial ? value.cursorData : value;
    const rawName = initial ? [value.first_name, value.last_name].filter(part => typeof part === 'string').join(' ') : value.name;
    function label(text) {
      if (typeof text !== 'string') return '';
      let result = '';
      for (const character of text.slice(0, 512)) {
        if (/[\u0000-\u001f\u007f]/.test(character) || (character.length === 1 && /[\uD800-\uDFFF]/.test(character))) continue;
        if (result.length + character.length > 128) break;
        result += character;
      }
      return result.trim();
    }
    const name = label(rawName) || label(value.email) || 'Anonymous';
    const docId = ID.test(cursor?.doc_id || '') ? cursor.doc_id : null;
    return { id, user_id: boundedIdentity(value.user_id), name, doc_id: docId,
      row: Number.isSafeInteger(cursor?.row) && cursor.row >= 0 && cursor.row <= MAX_SOURCE ? cursor.row : null,
      column: Number.isSafeInteger(cursor?.column) && cursor.column >= 0 && cursor.column <= MAX_SOURCE ? cursor.column : null,
      revision: docId ? documentRevision.get(docId) || 0 : 0 };
  }
  function rememberTrackingChange(id, peer) {
    if (!trackingChanges) return;
    if (!trackingChanges.has(id) && trackingChanges.size >= MAX_PEERS * 2) {
      // Discard an excessively delayed initial reply, rather than resurrecting a
      // disconnected client after its bounded change history has been evicted.
      trackingRequest++; trackingChanges = null; return;
    }
    trackingChanges.set(id, peer);
  }
  function updatePeer(value) {
    if (!isConnected()) return;
    refreshOwnIdentity();
    const peer = normalizePeer(value);
    if (!peer) return;
    rememberTrackingChange(peer.id, peer);
    if (peers.has(peer.id) || peers.size < MAX_PEERS) peers.set(peer.id, peer);
    queuePresence();
  }
  function removePeer(id) {
    if (!boundedIdentity(id)) return;
    rememberTrackingChange(id, null);
    peers.delete(id); queuePresence();
  }
  function receiveConnectedUsers(values, request) {
    if (!isConnected() || request !== trackingRequest || !Array.isArray(values)) return;
    refreshOwnIdentity();
    const next = new Map();
    // Bound traversal too: a hostile response cannot make every update O(n).
    for (const value of values.slice(0, MAX_PEERS * 2)) {
      const peer = normalizePeer(value, true);
      if (peer && next.size < MAX_PEERS) next.set(peer.id, peer);
    }
    for (const [id, peer] of trackingChanges || []) {
      if (peer) { if (next.has(id) || next.size < MAX_PEERS) next.set(id, peer); }
      else next.delete(id);
    }
    trackingChanges = null;
    peers.clear();
    for (const [id, peer] of next) peers.set(id, peer);
    queuePresence();
  }
  function queuePresence() {
    if (isStopped() || presenceTimer !== null || (!peers.size && !lastPresenceHadPeers)) return;
    presenceTimer = setTimeout(() => { presenceTimer = null; flushPresence(); }, PRESENCE_INTERVAL);
  }
  function flushPresence() {
    refreshOwnIdentity();
    const documents = new Map(), visiblePeers = [];
    let active = null;
    for (const peer of isConnected() ? peers.values() : []) {
      if (peer.id === ownId) continue;
      const { revision, ...value } = peer;
      if (entries.get(value.doc_id)?.kind !== 'doc') { value.doc_id = null; value.row = null; value.column = null; }
      visiblePeers.push(value);
      if (value.doc_id === null || value.row === null || value.column === null || dirty.has(value.doc_id) || (outgoingOt.get(value.doc_id) || 0) !== 0 || pending?.id === value.doc_id || opening?.id === value.doc_id) continue;
      const source = authoritativeSources.get(value.doc_id);
      const currentRevision = documentRevision.get(value.doc_id) || 0;
      if (!source || source.revision !== currentRevision || revision !== currentRevision) continue;
      if (read('editor.open_doc_id') === value.doc_id) {
        active ??= currentSourceInfo();
        if (!active || active.id !== value.doc_id || fingerprint(active.info) !== fingerprint(source.info)) continue;
      }
      const offset = positionOffset(source.info, value.row, value.column);
      if (offset === null) continue;
      let document = documents.get(value.doc_id);
      if (!document) {
        document = { id: value.doc_id, source_fingerprint: fingerprint(source.info), source_utf16: source.source.length, cursors: [] };
        documents.set(value.doc_id, document);
      }
      // Overleaf publishes only the caret head; it does not expose selections.
      document.cursors.push({ id: peer.id, anchor: offset, head: offset });
    }
    const fields = { origin: window.location.origin, own_id: ownId, self_user_id: selfUserId, peers: visiblePeers, documents: [...documents.values()] };
    const encoded = JSON.stringify(fields);
    lastPresenceHadPeers = visiblePeers.length > 0;
    if (encoded !== lastPresence) { lastPresence = encoded; post('presence', fields); }
  }
  function clearPresence() {
    clearTimeout(presenceTimer); presenceTimer = null;
    clearTimeout(cursorTimer); cursorTimer = null;
    queuedCursor = null; lastOwnPosition = '';
    trackingRequest++; trackingChanges = null; peers.clear();
    if (!isStopped()) flushPresence();
  }
  function queueCursor(command) {
    if (!isConnected() || !ID.test(command.id || '') || !/^[a-f0-9]{16}$/.test(command.source_fingerprint || '') || !Number.isSafeInteger(command.anchor) || !Number.isSafeInteger(command.head)) return;
    queuedCursor = { id: command.id, source_fingerprint: command.source_fingerprint, anchor: command.anchor, head: command.head };
    if (cursorTimer === null) cursorTimer = setTimeout(() => {
      cursorTimer = null;
      const cursor = queuedCursor; queuedCursor = null;
      if (!isConnected() || !cursor || entries.get(cursor.id)?.kind !== 'doc') return;
      try {
        const active = opening ? null : currentSourceInfo();
        const server = authoritativeSources.get(cursor.id);
        const activeMatches = active?.id === cursor.id;
        const info = activeMatches ? active.info : server?.info;
        // A parked native tab can publish its caret without activating the
        // hosted editor, but only against our exact acknowledged server text.
        if (!info || (!activeMatches && (server.revision !== (documentRevision.get(cursor.id) || 0) || dirty.has(cursor.id) || pending?.id === cursor.id)) || fingerprint(info) !== cursor.source_fingerprint || !validOffset(info.source, cursor.anchor) || !validOffset(info.source, cursor.head)) return;
        if (activeMatches) {
          const selection = active.view.state.selection?.main;
          if (selection?.anchor !== cursor.anchor || selection?.head !== cursor.head) active.view.dispatch({ selection: { anchor: cursor.anchor, head: cursor.head } });
        }
        const position = { doc_id: cursor.id, ...offsetPosition(info, cursor.head) };
        const encoded = JSON.stringify(position);
        if (encoded !== lastOwnPosition) {
          // The server validates this exact strict schema: no selection extensions.
          socket.emit('clientTracking.updatePosition', position);
          lastOwnPosition = encoded;
        }
      } catch (_) { /* A destroyed/replaced editor can safely drop an ephemeral caret. */ }
    }, PRESENCE_INTERVAL);
  }

  function post(event, fields = {}) {
    if (isStopped()) return;
    const value = JSON.stringify({ event, epoch, ...fields });
    if (new TextEncoder().encode(value).byteLength > MAX_MESSAGE) {
      window.ipc.postMessage(JSON.stringify({ event: 'error', epoch, message: 'Overleaf response exceeds the document size limit.' }));
      return;
    }
    window.ipc.postMessage(value);
  }
  function status(state, message) {
    if (fileOperation && !['connected', 'saving', 'loading'].includes(state)) {
      cleanup(() => fileOperation.controller.abort());
      finishFileOperation(fileOperation, 'The Overleaf connection changed. Reconnect to inspect the project before retrying.');
    }
    if (state === 'connected' && !message && projectWideUpdates === false) message = DOC_ROOMS_WARNING;
    post('status', { state, ...(message ? { message } : {}) });
  }
  function read(key) {
    try { return store?.get(key); } catch (_) { return undefined; }
  }
  function current() {
    const id = read('editor.open_doc_id');
    const view = read('editor.view');
    if (!ID.test(id || '') || !view?.state?.doc || typeof view.dispatch !== 'function') return null;
    const entry = entries.get(id);
    if (!entry || entry.kind !== 'doc') return null;
    const source = view.state.doc.toString();
    if (typeof source !== 'string') throw new Error('Overleaf editor document toString() did not return text.');
    if (source.length > MAX_SOURCE || new TextEncoder().encode(source).byteLength > MAX_SOURCE) throw new Error('Overleaf document exceeds the editable size limit.');
    return { id, path: entry.path, source, view };
  }
  function cacheAuthoritative(id, source) {
    const bytes = new TextEncoder().encode(source).byteLength;
    const previous = authoritativeSources.get(id);
    if (previous) authoritativeBytes -= previous.bytes;
    authoritativeSources.delete(id);
    authoritativeSources.set(id, { source, bytes, revision: documentRevision.get(id) || 0, info: previous?.source === source ? previous.info : sourceInfo(source) });
    authoritativeBytes += bytes;
    for (const [key, value] of authoritativeSources) {
      if (authoritativeBytes <= MAX_AUTHORITATIVE_BYTES) break;
      authoritativeSources.delete(key); authoritativeBytes -= value.bytes;
    }
    queuePresence();
  }
  function dropAuthoritative(id) {
    const previous = authoritativeSources.get(id);
    if (previous) authoritativeBytes -= previous.bytes;
    authoritativeSources.delete(id);
  }
  function rejectEdit(command, message) {
    // A hosted view may contain edits not yet flushed to its socket. It can
    // veto a CAS, but cannot become a native authoritative baseline that way.
    const source = authoritativeSources.get(command.id)?.source ?? command.base;
    if (command.fileOperation === fileOperation && fileOperation) finishFileOperation(fileOperation, message);
    else post('conflict', { id: command.id, request: command.request, source, message });
    if (entries.has(command.id)) { downloadQueue.add(command.id); pumpDownloads(); }
  }
  function cleanup(action) {
    try { if (typeof action === 'function') action(); } catch (_) { /* A stale page API cannot interrupt other cleanup. */ }
  }
  function incompatible(message) {
    if (isStopped() || bridgeCompatibilityError()) return;
    link = { phase: LinkPhase.Incompatible, message: 'Overleaf interface changed: ' + message + ' Reconnect after updating the app.' };
    clearTimeout(bootTimer); bootTimer = null;
    cleanup(clearPresence);
    cleanup(() => status('error', bridgeCompatibilityError()));
  }
  function observe(action) {
    try { return action(); }
    catch (_) { incompatible('an editor subscription could not be observed.'); return undefined; }
  }
  function replaceMethod(object, name, wrapped) {
    const descriptor = Object.getOwnPropertyDescriptor(object, name);
    if (descriptor?.get || descriptor?.set) throw new Error('Accessor methods cannot be intercepted.');
    Object.defineProperty(object, name, descriptor ? { ...descriptor, value: wrapped } : {
      configurable: true, enumerable: false, writable: true, value: wrapped,
    });
    if (object[name] !== wrapped) throw new Error('The method cannot be intercepted.');
    return () => {
      if (object[name] !== wrapped) return;
      if (descriptor) Object.defineProperty(object, name, descriptor);
      else delete object[name];
    };
  }
  function rememberMethod(dispose) {
    methodRestorers.push(dispose);
    // Global factories/Manager instances normally contribute only a few hooks.
    // Retire older instrumentation rather than retaining arbitrary page objects.
    if (methodRestorers.length > 64) cleanup(methodRestorers.shift());
  }
  function listen(name, handler) {
    const observed = event => { if (!isStopped()) observe(() => handler(event)); };
    window.addEventListener(name, observed);
    listeners.push([name, observed]);
  }
  function firstSource() {
    const root = entries.get(project?.root_doc);
    return root?.kind === 'doc' ? root : [...entries.values()].find(entry => entry.kind === 'doc');
  }
  function bootstrapSource() {
    if (isStopped() || bridgeCompatibilityError() || !hasProject() || socketAttachment !== 'attached' || storeAttachment !== 'attached' || bootstrap.phase !== 'ready') return;
    // Let React's pending effects/microtasks mount the editor naturally before
    // changing the hosted selection. This is one bounded bootstrap, not polling.
    const timer = setTimeout(() => {
      bootstrap = { phase: 'ready' };
      if (isStopped() || bridgeCompatibilityError() || !hasProject() || socketAttachment !== 'attached' || storeAttachment !== 'attached' || isConnected()) return;
      if (wrapView()) { activateProject(); return; }
      if (bridgeCompatibilityError()) return;
      bootstrap = { phase: 'attempted' };
      // A source is already selected: its lazy editor is mounting naturally.
      // Clicking that same row cannot prove a new opened event, so just retain
      // the existing startup watchdog rather than manufacture an open receipt.
      if (entries.get(read('editor.open_doc_id'))?.kind === 'doc') return;
      const entry = firstSource();
      if (entry && !opening && !pending && !queuedEdit) openDocument(entry, true);
    }, 0);
    bootstrap = { phase: 'scheduled', timer };
  }
  function resetBootstrap() {
    if (bootstrap.phase === 'scheduled') clearTimeout(bootstrap.timer);
    bootstrap = { phase: 'ready' };
  }
  function activateProject() {
    if (isStopped() || bridgeCompatibilityError() || !hasProject() || !project || socketAttachment !== 'attached' || storeAttachment !== 'attached') return false;
    if (isConnected()) return true;
    // A binary-only workspace has no CodeMirror view to wait for. Its verified
    // socket/store/tree still support authoritative asset reads and Explorer.
    if (firstSource()) {
      if (!wrapView()) { if (!bridgeCompatibilityError()) bootstrapSource(); return false; }
      if (opening?.bootstrap) return false;
    }
    resetBootstrap();
    clearTimeout(bootTimer); bootTimer = null;
    transitionLink(LinkPhase.Connected);
    publishProject(); post('fileaccess', { writable: canManageFiles && fileAccess === 'ready' }); flushPresence(); status('connected'); pumpDownloads();
    return true;
  }
  function attachStore() {
    if (isStopped() || bridgeCompatibilityError() || storeAttachment === 'attaching') return false;
    try {
      const next = window.overleaf?.unstable?.store;
      if (!next) return false;
      if (typeof next.get !== 'function' || typeof next.watch !== 'function') {
        incompatible('the editor store requires get() and watch().'); return false;
      }
      if (next === store && storeAttachment === 'attached') { wrapView(); return !bridgeCompatibilityError(); }
      storeAttachment = 'attaching';
      const generation = ++storeGeneration;
      store = next;
      while (unwatch.length) cleanup(unwatch.pop());
      for (const key of ['editor.view', 'editor.open_doc_id']) {
        const dispose = next.watch(key, () => {
          if (isStopped() || storeAttachment === 'attaching' || storeAttachment !== 'attached' || store !== next || generation !== storeGeneration) return;
          observe(() => {
            wrapView(); activateProject(); queuePresence();
            if (opening) finishOpen();
          });
        });
        if (typeof dispose !== 'function') throw new Error('watch() must return an unsubscribe function.');
        unwatch.push(dispose);
      }
      storeAttachment = 'attached';
      wrapView();
      return !bridgeCompatibilityError();
    } catch (_) {
      storeGeneration++;
      storeAttachment = 'detached';
      while (unwatch.length) cleanup(unwatch.pop());
      incompatible('the editor store watch() subscription is unsupported.');
      return false;
    }
  }
  function wrapView() {
    if (isStopped() || bridgeCompatibilityError() || storeAttachment !== 'attached') return false;
    try {
      const view = store.get('editor.view');
      if (!view) return false;
      if (typeof view.dispatch !== 'function' || typeof view.setState !== 'function') {
        incompatible('the editor requires dispatch() and setState().'); return false;
      }
      if (!view.state?.doc) return false;
      if (typeof view.state.doc.toString !== 'function') {
        incompatible('the editor document requires toString().'); return false;
      }
      if (viewHook?.view === view && view.setState === viewHook.wrapped) return true;
      if (viewHook) { cleanup(viewHook.restore); viewHook = null; }
      const setState = view.setState;
      const wrapped = function (...args) {
        const result = setState.apply(this, args);
        if (!isStopped() && !bridgeCompatibilityError() && viewHook?.wrapped === wrapped) observe(() => {
          queuePresence();
          if (opening && this === view) {
            openingViewInstalled(opening, { view, state: view.state, doc: view.state?.doc });
            finishOpen();
          }
        });
        return result;
      };
      const restore = replaceMethod(view, 'setState', wrapped);
      viewHook = { view, wrapped, restore };
      return true;
    } catch (_) {
      incompatible('the editor setState() method cannot be observed.');
      return false;
    }
  }
  function validName(name) {
    return typeof name === 'string' && name.length > 0 && name !== '.' && name !== '..' && !/[\/\\\0]/.test(name);
  }
  function addEntry(id, name, kind, parentId, next, nextParents, folderPath) {
    if (!ID.test(id || '') || !validName(name) || next.has(id) || next.size >= MAX_ENTRIES) throw new Error('Unsupported Overleaf file tree.');
    const path = folderPath ? folderPath + '/' + name : name;
    next.set(id, { id, path, kind });
    nextParents.set(id, parentId);
    return path;
  }
  function setProject(payload) {
    try {
      const value = payload?.project;
      if (!value || !ID.test(value._id || '') || !Array.isArray(value.rootFolder)) throw new Error('Overleaf did not provide a supported project tree.');
      const route = window.location.pathname.match(/^\/project\/([a-f0-9]{24})(?:\/|$)/);
      if (!route || route[1] !== value._id) throw new Error('Overleaf returned a different project.');
      const next = new Map();
      const nextParents = new Map();
      const nextRootIds = new Set();
      function folder(node, parentPath, depth, root) {
        if (depth > 64 || !node || !ID.test(node._id || '')) throw new Error('Unsupported Overleaf folder.');
        const path = root ? '' : addEntry(node._id, node.name, 'folder', null, next, nextParents, parentPath);
        for (const kind of ['docs', 'fileRefs', 'folders']) if (!Array.isArray(node[kind])) throw new Error('Unsupported Overleaf folder entries.');
        for (const item of node.docs) addEntry(item._id, item.name, 'doc', node._id, next, nextParents, path);
        for (const item of node.fileRefs) addEntry(item._id, item.name, 'file', node._id, next, nextParents, path);
        for (const item of node.folders) {
          folder(item, path, depth + 1, false);
          nextParents.set(item._id, node._id);
        }
      }
      for (const root of value.rootFolder) { folder(root, '', 0, true); nextRootIds.add(root._id); }
      ownId = boundedIdentity(payload.publicId) || boundedIdentity(socket?.publicId);
      refreshOwnIdentity();
      project = { project_id: value._id, name: String(value.name || 'Overleaf'), root_doc: ID.test(value.rootDoc_id || '') ? value.rootDoc_id : null };
      entries = next;
      for (const map of [outgoingOt, lastOwnAckVersion, documentRevision]) for (const id of map.keys()) {
        if (!entries.has(id) && pending?.id !== id) map.delete(id);
      }
      for (const id of dirty) if (!entries.has(id) && pending?.id !== id) dirty.delete(id);
      for (const id of authoritativeSources.keys()) if (!entries.has(id) && pending?.id !== id) dropAuthoritative(id);
      parents = nextParents;
      rootIds.clear();
      for (const id of nextRootIds) rootIds.add(id);
      fileAccess = 'ready';
      canManageFiles = payload.permissionsLevel === 'owner' || payload.permissionsLevel === 'readAndWrite';
      canEdit = canManageFiles || payload.permissionsLevel === 'review';
      if (!hasProject()) resetBootstrap();
      transitionLink(LinkPhase.Hydrating);
      treeRevision++;
      downloadQueue = new Set([...entries.values()].filter(entry => entry.kind !== 'folder').map(entry => entry.id));
      if (attachStore()) activateProject();
    } catch (error) { transitionLink(LinkPhase.AwaitingProject); clearPresence(); status('error', error.message); }
  }
  function publishProject() {
    if (isConnected() && project) post('project', { ...project, entries: [...entries.values()] });
  }
  function treeChanged(action, ...args) {
    try {
      if (!project) return;
      const [id, value] = args;
      if (action === 'create') {
        const [parentId, item, kind] = args;
        if (!rootIds.has(parentId) && entries.get(parentId)?.kind !== 'folder') throw new Error('Overleaf added a file to an unknown folder. Refresh the project.');
        const existing = entries.get(item?._id);
        const expectedPath = (entries.get(parentId)?.path ? entries.get(parentId).path + '/' : '') + item?.name;
        // The authenticated HTTP receipt and socket event can arrive in either
        // order. Both identify the same server entity, never a second create.
        if (existing?.kind === kind && existing.path === expectedPath && parents.get(item._id) === parentId) return;
        addEntry(item?._id, item?.name, kind, parentId, entries, parents, entries.get(parentId)?.path || '');
        if (kind !== 'folder') downloadQueue.add(item._id);
      } else {
        const old = entries.get(id);
        if (!old) return;
        const oldPath = old.path;
        if (action === 'remove') {
          for (const [key, entry] of entries) if (key === id || entry.path.startsWith(oldPath + '/')) {
            entries.delete(key); parents.delete(key); downloadQueue.delete(key); documentRevision.delete(key);
            if (pending?.id !== key) { outgoingOt.delete(key); lastOwnAckVersion.delete(key); dirty.delete(key); dropAuthoritative(key); }
          }
        } else {
          let newPath;
          if (action === 'rename') {
            if (!validName(value)) throw new Error('Overleaf supplied an unsupported file name.');
            const slash = oldPath.lastIndexOf('/');
            newPath = oldPath.slice(0, slash + 1) + value;
          } else {
            if (!rootIds.has(value) && entries.get(value)?.kind !== 'folder') throw new Error('Overleaf moved a file to an unknown folder. Refresh the project.');
            if (value === id || entries.get(value)?.path.startsWith(oldPath + '/')) throw new Error('Overleaf supplied a cyclic folder move.');
            const name = oldPath.slice(oldPath.lastIndexOf('/') + 1);
            newPath = entries.has(value) ? entries.get(value).path + '/' + name : name;
            parents.set(id, value);
          }
          for (const [key, entry] of entries) if (key === id || entry.path.startsWith(oldPath + '/')) {
            entries.set(key, { ...entry, path: newPath + entry.path.slice(oldPath.length) });
            if (entry.kind !== 'folder') downloadQueue.add(key);
          }
        }
      }
      treeRevision++;
      publishProject();
      queuePresence();
      pumpDownloads();
    } catch (error) { status('error', error.message); }
  }
  function invalidateDocument(id) {
    if (!entries.has(id)) return;
    documentRevision.set(id, (documentRevision.get(id) || 0) + 1);
    queuePresence();
    downloadQueue.add(id); pumpDownloads();
  }
  function bindSocket(value) {
    if (isStopped() || !value || value === socket) return value;
    try {
      if (typeof value.on !== 'function' || typeof value.emit !== 'function') {
        incompatible('the collaboration socket requires on() and emit().'); return value;
      }
      const old = socket;
      socketGeneration++; socketAttachment = 'detached';
      for (const [event, handler] of socketListeners.splice(0)) cleanup(() => old?.removeListener?.(event, handler));
      cleanup(restoreSocketEmit); restoreSocketEmit = null;
      transitionLink(LinkPhase.AwaitingProject);
      resetBootstrap();
      clearPresence(); ownId = null;
      socket = value;
      const generation = socketGeneration;
      const live = () => !isStopped() && socket === value && socketGeneration === generation;

      projectWideUpdates = null;
      warnedDocRooms = false;
      function on(event, handler) {
        const observed = (...args) => { if (live() && !bridgeCompatibilityError()) observe(() => handler(...args)); };
        value.on(event, observed); socketListeners.push([event, observed]);
      }
      on('joinProjectResponse', setProject);
      on('clientTracking.clientUpdated', updatePeer);
      on('clientTracking.clientDisconnected', removePeer);
      on('disconnect', () => { transitionLink(LinkPhase.AwaitingProject); clearPresence(); status('disconnected'); });
      on('connectionRejected', () => { transitionLink(LinkPhase.AwaitingProject); clearPresence(); status('signed-out', 'Sign in to Overleaf again.'); });
      on('project:access:revoked', () => { transitionLink(LinkPhase.AwaitingProject); clearPresence(); status('error', 'Overleaf project access was revoked.'); });
      on('otUpdateApplied', update => {
        const ownAck = update && !Object.prototype.hasOwnProperty.call(update, 'op') && Number.isSafeInteger(update.v) && update.v >= 0;
        if (ownAck && (entries.has(update.doc) || pending?.id === update.doc) && update.v > (lastOwnAckVersion.get(update.doc) ?? -1)) {
          lastOwnAckVersion.set(update.doc, update.v);
          const count = outgoingOt.get(update.doc) || 0;
          if (count > 0) outgoingOt.set(update.doc, count - 1);
          if (pending?.id === update.doc && pending.phase === 'sent') pending.phase = 'acknowledged';
        }
        invalidateDocument(update?.doc);
      });
      on('otUpdateError', () => { transitionLink(LinkPhase.AwaitingProject); clearPresence(); status('error', 'Overleaf could not save the document. Your local changes are retained.'); });
      on('reciveNewDoc', (parent, item) => treeChanged('create', parent, item, 'doc'));
      on('reciveNewFile', (parent, item) => treeChanged('create', parent, item, 'file'));
      on('reciveNewFolder', (parent, item) => treeChanged('create', parent, item, 'folder'));
      on('reciveEntityRename', (id, name) => treeChanged('rename', id, name));
      on('reciveEntityMove', (id, folder) => treeChanged('move', id, folder));
      on('removeEntity', id => treeChanged('remove', id));
      const emit = value.emit;
      const wrapped = function (event, ...args) {
        if (!live() || bridgeCompatibilityError()) return emit.call(this, event, ...args);
        observe(() => {
        if (event === 'clientTracking.getConnectedUsers' && typeof args.at(-1) === 'function') {
          const callback = args.at(-1), request = ++trackingRequest;
          trackingChanges = new Map();
          args[args.length - 1] = function (...reply) {
            if (!reply[0] && live() && !bridgeCompatibilityError()) observe(() => receiveConnectedUsers(reply[1], request));
            return callback.apply(this, reply);
          };
        }
        if (event === 'joinDoc' && typeof args.at(-1) === 'function') {
          const callback = args.at(-1);
          args[args.length - 1] = function (...reply) {
            if (!reply[0] && live() && !bridgeCompatibilityError()) observe(() => {
              projectWideUpdates = reply[6]?.canSkipLeaveDoc === true;
              if (!projectWideUpdates && !warnedDocRooms) {
                warnedDocRooms = true;
                if (isConnected()) status('connected');
              }
            });
            return callback.apply(this, reply);
          };
        }
        if (event === 'applyOtUpdate' && (entries.has(args[0]) || pending?.id === args[0])) {
          const id = args[0];
          dirty.add(id); queuePresence();
          const count = outgoingOt.get(id) || 0;
          if (count >= MAX_OUTGOING_OT || count === -1) {
            outgoingOt.set(id, -1);
            if (count !== -1) status('error', 'Overleaf save tracking is uncertain. Reconnect to reconcile your local changes.');
          } else outgoingOt.set(id, count + 1);
          if (pending?.id === args[0] && pending.phase === 'editing') pending.phase = 'sent';
        }
        });
        return emit.call(this, event, ...args);
      };
      restoreSocketEmit = replaceMethod(value, 'emit', wrapped);
      socketAttachment = 'attached';
      activateProject();
    } catch (_) {
      socketGeneration++; socketAttachment = 'detached';
      for (const [event, handler] of socketListeners.splice(0)) cleanup(() => value.removeListener?.(event, handler));
      cleanup(restoreSocketEmit); restoreSocketEmit = null;
      incompatible('the collaboration socket cannot be observed.');
    }
    return value;
  }
  const watchedIo = new WeakSet();
  function interceptMethod(object, name, wrap) {
    try {
      const descriptor = Object.getOwnPropertyDescriptor(object, name);
      let original = object[name];
      if (descriptor?.get || descriptor?.set) throw new Error('Accessor factories are unsupported.');
      const transform = value => {
        if (isStopped() || typeof value !== 'function') return value;
        try { return wrap(value); }
        catch (_) { incompatible('the collaboration ' + name + '() factory cannot be observed.'); return value; }
      };
      let method = transform(original);
      if (descriptor && !descriptor.configurable) {
        if (typeof original === 'function') rememberMethod(replaceMethod(object, name, method));
        else if (!descriptor.writable) incompatible('the collaboration ' + name + ' factory cannot be observed.');
        return;
      }
      const get = () => method;
      Object.defineProperty(object, name, {
        configurable: true, enumerable: descriptor?.enumerable ?? true,
        get,
        set(value) { original = value; method = transform(value); },
      });
      rememberMethod(() => {
        if (Object.getOwnPropertyDescriptor(object, name)?.get !== get) return;
        Object.defineProperty(object, name, descriptor ? { ...descriptor, value: original } : {
          configurable: true, enumerable: true, writable: true, value: original,
        });
      });
    } catch (_) { incompatible('the collaboration ' + name + '() factory cannot be observed.'); }
  }
  function wrapIo(io) {
    if (isStopped() || !io || (typeof io !== 'object' && typeof io !== 'function') || watchedIo.has(io)) return io;
    watchedIo.add(io);
    // Socket.IO assigns an empty global before populating its factories.
    interceptMethod(io, 'connect', original => function (...args) {
      return bindSocket(original.apply(this, args));
    });
    interceptMethod(io, 'Manager', manager => {
      function Manager(...args) {
        const instance = new.target
          ? Reflect.construct(manager, args, new.target === Manager ? manager : new.target)
          : Reflect.apply(manager, this, args);
        interceptMethod(instance, 'socket', original => function (...socketArgs) {
          return bindSocket(original.apply(this, socketArgs));
        });
        return instance;
      }
      Object.setPrototypeOf(Manager, manager);
      Manager.prototype = manager.prototype;
      return Manager;
    });
    return io;
  }
  let ioValue;
  try {
    ioValue = window.io;
    const ioDescriptor = Object.getOwnPropertyDescriptor(window, 'io');
    if (!ioDescriptor || ioDescriptor.configurable) {
      const get = () => ioValue;
      Object.defineProperty(window, 'io', {
        configurable: true, enumerable: ioDescriptor?.enumerable ?? true,
        get,
        set(value) { ioValue = wrapIo(value); },
      });
      rememberMethod(() => {
        if (Object.getOwnPropertyDescriptor(window, 'io')?.get !== get) return;
        Object.defineProperty(window, 'io', {
          configurable: true, enumerable: ioDescriptor?.enumerable ?? true, writable: true, value: ioValue,
        });
      });
    }
    wrapIo(ioValue);
  } catch (_) { incompatible('the collaboration global cannot be observed.'); }

  function openingInstalledView(request) {
    return request.progress.phase === 'view-installed' || request.progress.phase === 'opened-with-view'
      ? request.progress.installed : null;
  }
  function openingViewInstalled(request, installed) {
    request.progress = request.progress.phase === 'opened' || request.progress.phase === 'opened-with-view'
      ? { phase: 'opened-with-view', installed } : { phase: 'view-installed', installed };
  }
  function finishOpen() {
    if (isStopped() || bridgeCompatibilityError() || !opening || read('editor.open_doc_id') !== opening.id) return;
    const view = read('editor.view');
    // A selection/configuration transaction can replace EditorState while the
    // previous document is still installed. Only an observed full setState (it
    // may precede the ID notification), or fresh immutable text, certifies the
    // new document. The full-state path also supports shared empty Text values.
    const installed = openingInstalledView(opening);
    const observedState = installed !== null && installed.view === view && installed.doc === view?.state?.doc;
    const freshText = view?.state?.doc && view.state.doc !== opening.initialDoc;
    const opened = opening.progress.phase === 'opened' || opening.progress.phase === 'opened-with-view';
    if (!opened || (!observedState && !freshText)) return;
    const request = opening;
    clearTimeout(request.timer);
    clearTimeout(request.stepTimer);
    clearTimeout(request.finishTimer);
    opening = null;
    activateProject();
    queuePresence();
    // Initial hydration already queued every file before the bootstrap. Do not
    // enqueue a second root download after activation starts that first read.
    if (!request.bootstrap) downloadQueue.add(request.id);
    pumpDownloads();
    if (queuedEdit?.id === request.id) {
      const command = queuedEdit; queuedEdit = null;
      try { edit(command); } catch (error) { post('error', { message: error.message }); }
    }
    if (!pending) drainQueuedOpen();
  }
  function drainQueuedOpen() {
    if (!queuedOpen) return;
    const entry = entries.get(queuedOpen.id); queuedOpen = null;
    if (entry) openDocument(entry);
  }
  function rejectQueuedEdit(message) {
    if (!queuedEdit) return;
    const command = queuedEdit; queuedEdit = null;
    rejectEdit(command, message);
  }
  function openDocument(entry, bootstrap = false) {
    if (pending || (queuedEdit && queuedEdit.id !== entry.id)) { queuedOpen = entry; return; }
    if (!opening && read('editor.open_doc_id') === entry.id && current()) {
      downloadQueue.add(entry.id); pumpDownloads();
      if (queuedEdit?.id === entry.id) { const command = queuedEdit; queuedEdit = null; edit(command); }
      return;
    }
    if (opening) { clearTimeout(opening.timer); clearTimeout(opening.stepTimer); clearTimeout(opening.finishTimer); }
    const initialView = read('editor.view');
    opening = { id: entry.id, bootstrap, initialView, initialDoc: initialView?.state?.doc, progress: { phase: 'waiting' }, timer: setTimeout(() => {
      opening = null;
      rejectQueuedEdit('Overleaf did not open the selected document. Your local edit is retained.');
      post('error', { message: 'Overleaf did not open the selected document.' });
    }, 15000) };
    const ancestors = [];
    let parent = parents.get(entry.id);
    for (let count = 0; entries.has(parent) && count < 64; count++) { ancestors.unshift(parent); parent = parents.get(parent); }
    function clickId(id) {
      return document.querySelector('[data-file-id="' + id + '"]')?.closest('[role="treeitem"]');
    }
    const request = opening;
    function activate(index = 0) {
      if (isStopped() || opening !== request) return;
      if (index < ancestors.length) {
        const node = clickId(ancestors[index]);
        if (node?.getAttribute('aria-expanded') === 'false') node.click();
        request.stepTimer = setTimeout(() => { request.stepTimer = null; activate(index + 1); }, 0);
        return;
      }
      const node = clickId(entry.id);
      if (node) node.click();
      else if (queuedEdit?.fileOperation === fileOperation && fileOperation) {
        // The successful create may precede React mounting its tree row. Wait
        // only under the existing 15-second open deadline; never reload an
        // internal copy continuation or send a synthetic write probe.
        request.stepTimer = setTimeout(() => { request.stepTimer = null; activate(index); }, 50);
      } else if (queuedEdit) {
        clearTimeout(request.timer); opening = null;
        rejectQueuedEdit('Open this Overleaf document before saving it. Your local edit is retained.');
      } else if (bootstrap) {
        // The store/socket precede the asynchronously loaded editor. Review or
        // a collapsed sidebar can leave the tree unmounted during that load.
        // A deep-link reload cancels those assets; Overleaf consumes ?doc on
        // startup, so its presence cannot prevent the next page reloading too.
        // No click occurred: leave natural mounting to the startup watchdog.
        clearTimeout(request.timer); clearTimeout(request.stepTimer); clearTimeout(request.finishTimer);
        opening = null;
      } else {
        // Explicit file opens can use the official deep link for an unmounted row.
        const url = new URL(window.location.href);
        url.searchParams.set('doc', entry.id);
        window.location.assign(url.toString());
      }
    }
    activate();
  }
  function requestEdit(command) {
    if (opening || read('editor.open_doc_id') !== command.id) {
      const entry = entries.get(command.id);
      if (!entry || entry.kind !== 'doc' || pending || queuedEdit) {
        rejectEdit(command, 'Overleaf cannot activate this document for saving. Your local edit is retained.');
        return;
      }
      queuedEdit = command;
      if (!opening || opening.id !== command.id) openDocument(entry);
      return;
    }
    edit(command);
  }
  function pumpDownloads() {
    if (isStopped() || !isConnected()) return;
    for (const id of downloadQueue) {
      if (downloads.size >= 2) break;
      if (downloadingIds.has(id) || dirty.has(id) || (pending?.id === id && pending.phase !== 'confirming') || opening?.id === id) continue;
      const entry = entries.get(id);
      downloadQueue.delete(id);
      if (entry) download(entry).catch(error => post('error', { message: error.message }));
    }
  }
  async function download(entry) {
    const controller = new AbortController();
    const revision = documentRevision.get(entry.id) || 0;
    const tree = treeRevision;
    const confirming = pending?.id === entry.id && pending.phase === 'confirming' ? pending : null;
    downloads.add(controller);
    downloadingIds.add(entry.id);
    const timeout = setTimeout(() => controller.abort(), 30000);
    try {
      const suffix = entry.kind === 'doc' ? '/doc/' + entry.id + '/download' : '/file/' + entry.id;
      const response = await fetch('/project/' + project.project_id + suffix, { credentials: 'same-origin', redirect: 'error', signal: controller.signal });
      if (!response.ok) throw new Error('Overleaf could not download ' + entry.path + '.');
      if (entry.kind === 'doc' && (response.headers.get('content-type') || '').split(';', 1)[0].trim().toLowerCase() !== 'text/plain') {
        throw new Error('Overleaf did not return plain text for this document. Sign in or reconnect before saving.');
      }
      const limit = entry.kind === 'doc' ? MAX_SOURCE : MAX_FILE;
      const declared = Number(response.headers.get('content-length') || 0);
      if (declared > limit) throw new Error('Overleaf file exceeds the download size limit.');
      if (!response.body?.getReader) throw new Error('This browser cannot stream Overleaf downloads.');
      const reader = response.body.getReader();
      const chunks = [];
      let length = 0;
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        length += value.byteLength;
        if (length > limit) { await reader.cancel(); throw new Error('Overleaf file exceeds the download size limit.'); }
        chunks.push(value);
      }
      if (isStopped() || !isConnected() || !entries.has(entry.id)) return;
      if (tree !== treeRevision || revision !== (documentRevision.get(entry.id) || 0) || dirty.has(entry.id) || (pending?.id === entry.id && pending !== confirming)) {
        downloadQueue.add(entry.id);
        return;
      }
      if (entry.kind === 'doc') {
        const bytes = new Uint8Array(length);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        const source = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
        cacheAuthoritative(entry.id, source);
        if (confirming && pending === confirming) {
          clearTimeout(pending.timer);
          const request = pending.request;
          const operation = pending.fileOperation;
          pending = null;
          if (operation) {
            post('document', { id: entry.id, path: entry.path, source });
            finishFileOperation(operation, null, entry);
          } else post('ack', { id: entry.id, request, source });
          status('connected');
          drainQueuedOpen();
        } else post('document', { id: entry.id, path: entry.path, source });
      } else {
        let binary = '';
        for (const chunk of chunks) for (let start = 0; start < chunk.length; start += 8192) binary += String.fromCharCode(...chunk.subarray(start, start + 8192));
        post('file', { id: entry.id, path: entry.path, base64: btoa(binary) });
      }
    } finally {
      clearTimeout(timeout);
      downloads.delete(controller);
      downloadingIds.delete(entry.id);
      pumpDownloads();
    }
  }
  function edit(command) {
    const value = current();
    if (!value || value.id !== command.id) {
      rejectEdit(command, 'The selected Overleaf document is no longer active. Your local edit is retained.');
      return;
    }
    if (!canEdit) {
      rejectEdit(command, 'This Overleaf project is read-only. Your local edit is retained.');
      return;
    }
    if (!isConnected() || pending || opening || dirty.has(value.id) || value.source !== command.base) {
      rejectEdit(command, 'Overleaf changed before this edit could be applied. Your local edit is retained.');
      return;
    }
    if (typeof command.source !== 'string' || command.source.length > MAX_SOURCE || new TextEncoder().encode(command.source).byteLength > MAX_SOURCE || !Number.isSafeInteger(command.request) || command.request < 0) throw new Error('Invalid Overleaf edit.');
    pending = { id: value.id, request: command.request, fileOperation: command.fileOperation === fileOperation ? fileOperation : null, phase: command.source === value.source ? 'confirming' : 'editing', timer: setTimeout(() => {
        status('saving', 'Still waiting for Overleaf to acknowledge this edit.');
      }, 30000) };
    status('saving');
    if (pending.phase === 'confirming') {
      downloadQueue.add(value.id); pumpDownloads();
      return;
    }
    let from = 0;
    let oldEnd = value.source.length;
    let newEnd = command.source.length;
    while (from < oldEnd && from < newEnd && value.source[from] === command.source[from]) from++;
    // Keep UTF-16 surrogate pairs intact for both CodeMirror and ShareJS.
    if (from > 0 && /[\uD800-\uDBFF]/.test(value.source[from - 1])) from--;
    while (oldEnd > from && newEnd > from && value.source[oldEnd - 1] === command.source[newEnd - 1]) { oldEnd--; newEnd--; }
    if (oldEnd < value.source.length && /[\uDC00-\uDFFF]/.test(value.source[oldEnd])) { oldEnd++; newEnd++; }
    try { value.view.dispatch({ changes: { from, to: oldEnd, insert: command.source.slice(from, newEnd) } }); }
    catch (error) {
      // Dispatch may have changed the hosted view before throwing. Retain the
      // request and suppress optimistic snapshots until a real save or reload.
      clearTimeout(pending.timer);
      throw error;
    }
  }
  function saved(event) {
    const id = event.detail?.id;
    if ((outgoingOt.get(id) || 0) !== 0) return;
    dirty.delete(id);
    queuePresence();
    pumpDownloads();
    if (!pending || event.detail?.id !== pending.id || pending.phase !== 'acknowledged') return;
    // doc:saved can be delayed past a newer, still-debounced hosted edit.
    // Only a revision-checked server download can certify the saved text.
    pending.phase = 'confirming';
    downloadQueue.add(id);
    pumpDownloads();
  }
  function fileParent(path) {
    if (path === '') {
      if (rootIds.size !== 1) throw new Error('Overleaf did not identify one project root. Reconnect before changing files.');
      return [...rootIds][0];
    }
    const entry = [...entries.values()].find(entry => entry.path === path);
    if (entry?.kind !== 'folder') throw new Error('This Overleaf folder is no longer in the project.');
    return entry.id;
  }
  function fileName(name) {
    if (!validName(name) || name.length >= 150 || name.trim() !== name || /^\.(?:tiptoptyp|git)$/i.test(name) || /[\*\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(name) || /^(?:prototype|constructor|toString|toLocaleString|valueOf|hasOwnProperty|isPrototypeOf|propertyIsEnumerable|__defineGetter__|__lookupGetter__|__defineSetter__|__lookupSetter__|__proto__)$/.test(name)) {
      throw new Error('Use a file name of 1–149 characters without slashes, control characters or surrounding spaces.');
    }
    return name;
  }
  function representableFilePath(path, kind) {
    if (new TextEncoder().encode(path).byteLength > 4096 || path.split('/').length > (kind === 'folder' ? 64 : 65)) {
      throw new Error('This file operation would make an Overleaf path too long or too deeply nested. Choose a shorter name or a shallower folder.');
    }
  }
  function vacantFileName(parent, name, except, kind = 'doc') {
    const path = (entries.get(parent)?.path ? entries.get(parent).path + '/' : '') + name;
    const original = entries.get(except);
    representableFilePath(path, original?.kind || kind);
    if (original?.kind === 'folder') {
      // A short destination can still push a descendant beyond the mirror or
      // startup tree limits. Reject the whole operation before its first write.
      for (const entry of entries.values()) if (entry.path.startsWith(original.path + '/')) {
        representableFilePath(path + entry.path.slice(original.path.length), entry.kind);
      }
    }
    const folded = path.normalize('NFC').toLowerCase();
    for (const entry of entries.values()) if (entry.id !== except && entry.path.normalize('NFC').toLowerCase() === folded) {
      throw new Error('An Overleaf file or folder with this name already exists.');
    }
    return path;
  }
  function finishFileOperation(operation, error, entry) {
    if (fileOperation !== operation || operation.phase === 'done' || isStopped()) return;
    const uncertain = operation.phase === 'sent';
    operation.phase = 'done';
    clearTimeout(operation.timer);
    fileOperation = null;
    if (error && uncertain) {
      fileAccess = 'reconcile-required';
      post('fileaccess', { writable: false });
    }
    if (error && operation.created) error += ' The new file ' + operation.created.path.slice(operation.created.path.lastIndexOf('/') + 1) + ' may be incomplete; inspect it before retrying.';
    post('fileaction', { request: operation.request, ...(error ? { error } : { id: entry?.id || null, path: entry?.path || null }) });
  }
  async function fileJSON(operation, suffix, method, body) {
    const csrf = document.querySelector('meta[name="ol-csrfToken"]')?.content;
    if (typeof csrf !== 'string' || !csrf || csrf.length > 4096 || /[\u0000-\u001f\u007f]/.test(csrf)) throw new Error('Overleaf did not provide a security token. Sign in or reconnect before changing files.');
    operation.phase = 'sent';
    const response = await fetch('/project/' + project.project_id + suffix, {
      method, credentials: 'same-origin', redirect: 'error', signal: operation.controller.signal,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json', 'X-Csrf-Token': csrf },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!response.ok) {
      if (response.status >= 400 && response.status < 500) operation.phase = 'rejected';
      if (response.status === 403 || response.status === 401) throw new Error('Overleaf denied this file operation. Sign in again or check project write access.');
      if (response.status === 429) throw new Error('Overleaf is rate limiting file changes. Wait before trying again.');
      throw new Error('Overleaf could not change this file (HTTP ' + response.status + '). Reconnect to check the project before retrying.');
    }
    const type = (response.headers.get('content-type') || '').split(';', 1)[0].trim().toLowerCase();
    if (type && type !== 'application/json' && type !== 'text/plain') throw new Error('Overleaf returned an unexpected file-operation response. Sign in or reconnect to check the project.');
    if (isStopped() || !isConnected() || fileOperation !== operation || operation.phase === 'done' || operation.generation !== socketGeneration) throw new Error('The Overleaf connection changed. Reconnect to check whether this operation completed.');
    if (type === 'application/json') {
      const declared = Number(response.headers.get('content-length') || 0);
      if (declared > 65536 || !response.body?.getReader) throw new Error('Overleaf returned an unsupported file-operation response.');
      const reader = response.body.getReader(); const chunks = []; let length = 0;
      while (true) { const part = await reader.read(); if (part.done) break; length += part.value.byteLength; if (length > 65536) { await reader.cancel(); throw new Error('Overleaf file-operation response is too large.'); } chunks.push(part.value); }
      const bytes = new Uint8Array(length); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      if (isStopped() || !isConnected() || fileOperation !== operation || operation.phase === 'done' || operation.generation !== socketGeneration) throw new Error('The Overleaf connection changed. Reconnect to inspect the project.');
      const value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
      if (value && typeof value === 'object' && (value.success === false || Object.prototype.hasOwnProperty.call(value, 'error'))) {
        operation.phase = 'rejected';
        throw new Error('Overleaf rejected this file operation. Check the project and try again.');
      }
      return value;
    }
    return null;
  }
  async function duplicateSource(operation, entry) {
    const revision = documentRevision.get(entry.id) || 0;
    if (dirty.has(entry.id) || (outgoingOt.get(entry.id) || 0)) throw new Error('Wait for the original document to finish saving before duplicating it.');
    const response = await fetch('/project/' + project.project_id + '/doc/' + entry.id + '/download', { credentials: 'same-origin', redirect: 'error', signal: operation.controller.signal });
    if (!response.ok || (response.headers.get('content-type') || '').split(';', 1)[0].trim().toLowerCase() !== 'text/plain' || !response.body?.getReader) throw new Error('Overleaf could not confirm the document to duplicate. Reconnect before retrying.');
    if (Number(response.headers.get('content-length') || 0) > MAX_SOURCE) throw new Error('This Overleaf document exceeds the editable size limit.');
    const reader = response.body.getReader(); const chunks = []; let length = 0;
    while (true) { const part = await reader.read(); if (part.done) break; length += part.value.byteLength; if (length > MAX_SOURCE) { await reader.cancel(); throw new Error('This Overleaf document exceeds the editable size limit.'); } chunks.push(part.value); }
    if (isStopped() || !isConnected() || fileOperation !== operation || operation.phase === 'done' || entries.get(entry.id)?.path !== entry.path || revision !== (documentRevision.get(entry.id) || 0) || dirty.has(entry.id) || operation.generation !== socketGeneration) throw new Error('The original document changed while duplicating it. Try again after it finishes saving.');
    const bytes = new Uint8Array(length); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  }
  async function manageFiles(command) {
    if (!Number.isSafeInteger(command.request) || command.request < 1) { post('error', { message: 'Invalid Overleaf file-operation request.' }); return; }
    if (fileOperation) { post('fileaction', { request: command.request, error: 'Wait for the current Overleaf file operation to finish.' }); return; }
    if (fileAccess === 'reconcile-required') { post('fileaction', { request: command.request, error: 'Reconnect to inspect the project before making more file changes.' }); return; }
    const operation = { request: command.request, generation: socketGeneration, controller: new AbortController(), phase: 'preparing', timer: null };
    fileOperation = operation;
    operation.timer = setTimeout(() => {
      cleanup(() => operation.controller.abort());
      finishFileOperation(operation, 'Overleaf has not confirmed the file operation. Reconnect to inspect the project before retrying.');
    }, 45000);
    try {
      if (!isConnected() || !canManageFiles) throw new Error('This Overleaf project does not allow file changes.');
      if (pending || opening || queuedEdit) throw new Error('Wait for the current Overleaf document to finish opening or saving.');
      const action = command.action;
      if (!['createfile', 'createfolder', 'duplicate', 'rename', 'delete', 'move'].includes(action)) throw new Error('Unsupported Overleaf file operation.');
      const creating = action === 'createfile' || action === 'createfolder';
      const entry = creating ? null : entries.get(command.id);
      if (!creating && (!entry || entry.path !== command.path)) throw new Error('This Overleaf file changed. Reopen its menu before changing it.');
      const parent = creating ? fileParent(command.parent) : parents.get(entry.id);
      if (!ID.test(parent || '')) throw new Error('Overleaf did not identify the file’s parent folder. Reconnect before changing files.');
      if (creating || action === 'duplicate') {
        const name = fileName(command.name);
        const kind = action === 'createfolder' ? 'folder' : 'doc';
        vacantFileName(parent, name, undefined, kind);
        if (entries.size >= MAX_ENTRIES) throw new Error('This Overleaf project contains too many files.');
        if (action === 'duplicate' && entry.kind !== 'doc') throw new Error('Only editable Overleaf documents can be duplicated safely in the app.');
        const source = action === 'duplicate' ? await duplicateSource(operation, entry) : null;
        vacantFileName(parent, name, undefined, kind);
        const result = await fileJSON(operation, '/' + kind, 'POST', { parent_folder_id: parent, name });
        if (!result || !ID.test(result._id || '') || result.name !== name) throw new Error('Overleaf did not confirm the new file’s identity. Reconnect to check the project before retrying.');
        treeChanged('create', parent, { _id: result._id, name }, kind);
        const created = entries.get(result._id);
        if (!created || created.path !== vacantFileName(parent, name, result._id)) throw new Error('The new Overleaf file changed. Reconnect to inspect it.');
        if (action === 'duplicate' && source) {
          operation.created = created;
          requestEdit({ id: created.id, base: '', source, request: operation.request, fileOperation: operation });
          return;
        }
        finishFileOperation(operation, null, created);
      } else {
        let suffix = '/' + entry.kind + '/' + entry.id;
        let body;
        if (action === 'rename') { const name = fileName(command.name); vacantFileName(parent, name, entry.id); suffix += '/rename'; body = { name }; }
        if (action === 'move') { const folder = fileParent(command.parent); vacantFileName(folder, entry.path.slice(entry.path.lastIndexOf('/') + 1), entry.id); if (entry.kind === 'folder' && (folder === entry.id || entries.get(folder)?.path.startsWith(entry.path + '/'))) throw new Error('A folder cannot be moved inside itself.'); suffix += '/move'; body = { folder_id: folder }; }
        await fileJSON(operation, suffix, action === 'delete' ? 'DELETE' : 'POST', body);
        const current = entries.get(entry.id);
        if (current?.path === entry.path) {
          if (action === 'delete') treeChanged('remove', entry.id);
          else treeChanged(action, entry.id, action === 'rename' ? body.name : body.folder_id);
        }
        finishFileOperation(operation, null, entries.get(entry.id));
      }
    } catch (error) { finishFileOperation(operation, error.message || 'Overleaf could not complete this file operation.'); }
  }
  function stop() {
    if (isStopped()) return;
    link = { phase: LinkPhase.Stopped };
    clearTimeout(bootTimer); bootTimer = null;
    resetBootstrap();
    if (fileOperation) { clearTimeout(fileOperation.timer); cleanup(() => fileOperation.controller.abort()); fileOperation = null; }
    clearPresence();
    if (pending) clearTimeout(pending.timer);
    if (opening) { clearTimeout(opening.timer); clearTimeout(opening.stepTimer); clearTimeout(opening.finishTimer); }
    pending = null; opening = null; queuedOpen = null; queuedEdit = null;
    downloadQueue.clear();
    for (const controller of downloads) cleanup(() => controller.abort());
    storeGeneration++; storeAttachment = 'detached';
    while (unwatch.length) cleanup(unwatch.pop());
    for (const [event, handler] of listeners.splice(0)) cleanup(() => window.removeEventListener(event, handler));
    socketGeneration++; socketAttachment = 'detached';
    for (const [event, handler] of socketListeners.splice(0)) cleanup(() => socket?.removeListener?.(event, handler));
    cleanup(restoreSocketEmit); restoreSocketEmit = null;
    if (viewHook) { cleanup(viewHook.restore); viewHook = null; }
    while (methodRestorers.length) cleanup(methodRestorers.pop());
    cleanup(() => socket?.disconnect?.());
  }
  window.tiptoptypOverleaf = {
    command(command) {
      if (isStopped() || !command || typeof command !== 'object') return;
      if (command.epoch !== epoch) return;
      try {
        if (command.command === 'disconnect') { stop(); return; }
        if (command.command !== 'refresh') {
          const subscribed = attachStore();
          const projectReady = subscribed && !bridgeCompatibilityError() && socketAttachment === 'attached' && hasProject() && project;
          const needsView = command.command === 'edit' || command.command === 'cursor';
          if (!projectReady || (needsView && !wrapView())) {
            if (!bridgeCompatibilityError()) post('error', { message: 'Overleaf editor is not ready. Your local edit is retained.' });
            return;
          }
        }
        activateProject();
        switch (command.command) {
          case 'fileaction': void manageFiles(command); break;
          case 'edit':
            if (fileOperation) throw new Error('Wait for the Overleaf file operation to finish.');
            requestEdit(command); break;
          case 'cursor': queueCursor(command); break;
          case 'open': {
            const entry = entries.get(command.id);
            if (!entry || entry.path !== command.path) throw new Error('This Overleaf file is no longer in the project.');
            if (entry.kind === 'doc') openDocument(entry);
            else if (entry.kind === 'file') { downloadQueue.add(entry.id); pumpDownloads(); }
            break;
          }
          case 'download': {
            const entry = entries.get(command.id);
            if (!entry || entry.path !== command.path || entry.kind === 'folder') throw new Error('This Overleaf file is no longer in the project.');
            downloadQueue.add(entry.id);
            pumpDownloads();
            break;
          }
          case 'refresh':
            if (pending) throw new Error('Wait for the current Overleaf edit to finish saving.');
            window.location.reload();
            break;
          case 'disconnect': stop(); break;
          default: throw new Error('Unknown Overleaf command.');
        }
      } catch (error) { post('error', { message: error.message }); }
    },
  };
  listen('doc:after-opened', event => {
    if (opening && event.detail?.docId === opening.id) {
      const request = opening;
      const installed = openingInstalledView(request);
      request.progress = installed ? { phase: 'opened-with-view', installed } : { phase: 'opened' };
      if (request.finishTimer == null) request.finishTimer = setTimeout(() => {
        request.finishTimer = null;
        if (opening === request) finishOpen();
      }, 0);
    }
    else {
      const id = event.detail?.docId;
      if (entries.has(id)) { downloadQueue.add(id); pumpDownloads(); }
    }
  });
  listen('doc:saved', saved);
  listen('pagehide', stop);
  function boot() {
    if (isStopped() || bridgeCompatibilityError()) return;
    observe(() => wrapIo(window.io));
    if (attachStore() && activateProject()) { clearTimeout(bootTimer); bootTimer = null; return; }
    if (bridgeCompatibilityError()) return;
    if (++bootAttempts >= 120) {
      bootTimer = null;
      status(window.location.pathname.startsWith('/login') ? 'signed-out' : 'error', 'Overleaf editor is not ready. Sign in, or refresh the connection.');
      return;
    }
    bootTimer = setTimeout(boot, 250);
  }
  if (!bridgeCompatibilityError()) status('loading');
  boot();
})();
