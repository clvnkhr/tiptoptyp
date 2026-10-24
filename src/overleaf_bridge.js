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
  let stopped = false;
  let socket = null;
  let connected = false;
  let projectWideUpdates = null;
  let warnedDocRooms = false;
  const DOC_ROOMS_WARNING = 'Connected · refresh to sync other files on this server';
  let project = null;
  let canEdit = false;
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
  const unwatch = [];
  const listeners = [];
  const socketListeners = [];
  const downloads = new Set();
  const downloadingIds = new Set();
  const wrappedViews = new WeakSet();
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
    if (!connected) return;
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
    if (!connected || request !== trackingRequest || !Array.isArray(values)) return;
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
    if (stopped || presenceTimer !== null || (!peers.size && !lastPresenceHadPeers)) return;
    presenceTimer = setTimeout(() => { presenceTimer = null; flushPresence(); }, PRESENCE_INTERVAL);
  }
  function flushPresence() {
    refreshOwnIdentity();
    const documents = new Map(), visiblePeers = [];
    let active = null;
    for (const peer of connected ? peers.values() : []) {
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
    flushPresence();
  }
  function queueCursor(command) {
    if (!connected || !ID.test(command.id || '') || !/^[a-f0-9]{16}$/.test(command.source_fingerprint || '') || !Number.isSafeInteger(command.anchor) || !Number.isSafeInteger(command.head)) return;
    queuedCursor = { id: command.id, source_fingerprint: command.source_fingerprint, anchor: command.anchor, head: command.head };
    if (cursorTimer === null) cursorTimer = setTimeout(() => {
      cursorTimer = null;
      const cursor = queuedCursor; queuedCursor = null;
      if (!connected || !cursor || entries.get(cursor.id)?.kind !== 'doc') return;
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
    if (stopped) return;
    const value = JSON.stringify({ event, epoch, ...fields });
    if (new TextEncoder().encode(value).byteLength > MAX_MESSAGE) {
      window.ipc.postMessage(JSON.stringify({ event: 'error', epoch, message: 'Overleaf response exceeds the document size limit.' }));
      return;
    }
    window.ipc.postMessage(value);
  }
  function status(state, message) {
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
    post('conflict', { id: command.id, request: command.request, source, message });
    if (entries.has(command.id)) { downloadQueue.add(command.id); pumpDownloads(); }
  }
  function listen(name, handler) {
    window.addEventListener(name, handler);
    listeners.push([name, handler]);
  }
  function attachStore() {
    const next = window.overleaf?.unstable?.store;
    if (!next || typeof next.get !== 'function' || typeof next.watch !== 'function') return false;
    if (next === store) { wrapView(); return true; }
    while (unwatch.length) unwatch.pop()();
    store = next;
    for (const key of ['editor.view', 'editor.open_doc_id']) {
      unwatch.push(store.watch(key, () => {
        wrapView(); queuePresence();
        if (opening && read('editor.open_doc_id') === opening.id) finishOpen();
      }));
    }
    wrapView();
    return true;
  }
  function wrapView() {
    const view = read('editor.view');
    if (!view || wrappedViews.has(view) || typeof view.setState !== 'function') return;
    wrappedViews.add(view);
    const setState = view.setState;
    view.setState = function (...args) {
      const result = setState.apply(this, args);
      queuePresence();
      if (opening && read('editor.open_doc_id') === opening.id) { opening.installed = true; finishOpen(); }
      return result;
    };
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
      canEdit = payload.permissionsLevel === 'owner' || payload.permissionsLevel === 'readAndWrite' || payload.permissionsLevel === 'review';
      connected = true;
      treeRevision++;
      downloadQueue = new Set([...entries.values()].filter(entry => entry.kind !== 'folder').map(entry => entry.id));
      publishProject();
      flushPresence();
      status('connected');
      attachStore();
      pumpDownloads();
    } catch (error) { connected = false; clearPresence(); status('error', error.message); }
  }
  function publishProject() {
    if (project) post('project', { ...project, entries: [...entries.values()] });
  }
  function treeChanged(action, ...args) {
    try {
      if (!project) return;
      const [id, value] = args;
      if (action === 'create') {
        const [parentId, item, kind] = args;
        if (!rootIds.has(parentId) && entries.get(parentId)?.kind !== 'folder') throw new Error('Overleaf added a file to an unknown folder. Refresh the project.');
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
    if (!value || typeof value.on !== 'function' || value === socket) return value;
    if (socket) for (const [event, handler] of socketListeners.splice(0)) socket.removeListener?.(event, handler);
    connected = false; clearPresence(); ownId = null;
    socket = value;
    projectWideUpdates = null;
    warnedDocRooms = false;
    function on(event, handler) { value.on(event, handler); socketListeners.push([event, handler]); }
    on('joinProjectResponse', setProject);
    on('clientTracking.clientUpdated', updatePeer);
    on('clientTracking.clientDisconnected', removePeer);
    on('disconnect', () => { connected = false; clearPresence(); status('disconnected'); });
    on('connectionRejected', () => { connected = false; clearPresence(); status('signed-out', 'Sign in to Overleaf again.'); });
    on('project:access:revoked', () => { connected = false; clearPresence(); status('error', 'Overleaf project access was revoked.'); });
    on('otUpdateApplied', update => {
      const ownAck = update && !Object.prototype.hasOwnProperty.call(update, 'op') && Number.isSafeInteger(update.v) && update.v >= 0;
      if (ownAck && (entries.has(update.doc) || pending?.id === update.doc) && update.v > (lastOwnAckVersion.get(update.doc) ?? -1)) {
        lastOwnAckVersion.set(update.doc, update.v);
        const count = outgoingOt.get(update.doc) || 0;
        if (count > 0) outgoingOt.set(update.doc, count - 1);
        if (pending?.id === update.doc && pending.sent) pending.acknowledged = true;
      }
      invalidateDocument(update?.doc);
    });
    on('otUpdateError', () => { connected = false; clearPresence(); status('error', 'Overleaf could not save the document. Your local changes are retained.'); });
    on('reciveNewDoc', (parent, item) => treeChanged('create', parent, item, 'doc'));
    on('reciveNewFile', (parent, item) => treeChanged('create', parent, item, 'file'));
    on('reciveNewFolder', (parent, item) => treeChanged('create', parent, item, 'folder'));
    on('reciveEntityRename', (id, name) => treeChanged('rename', id, name));
    on('reciveEntityMove', (id, folder) => treeChanged('move', id, folder));
    on('removeEntity', id => treeChanged('remove', id));
    const emit = value.emit;
    value.emit = function (event, ...args) {
      if (event === 'clientTracking.getConnectedUsers' && typeof args.at(-1) === 'function') {
        const callback = args.at(-1), request = ++trackingRequest;
        trackingChanges = new Map();
        args[args.length - 1] = function (...reply) {
          if (!reply[0] && socket === value && !stopped) receiveConnectedUsers(reply[1], request);
          return callback.apply(this, reply);
        };
      }
      if (event === 'joinDoc' && typeof args.at(-1) === 'function') {
        const callback = args.at(-1);
        args[args.length - 1] = function (...reply) {
          if (!reply[0] && socket === value && !stopped) {
            projectWideUpdates = reply[6]?.canSkipLeaveDoc === true;
            if (!projectWideUpdates && !warnedDocRooms) {
              warnedDocRooms = true;
              status('connected');
            }
          }
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
        if (pending?.id === args[0]) pending.sent = true;
      }
      return emit.call(this, event, ...args);
    };
    return value;
  }
  const watchedIo = new WeakSet();
  function interceptMethod(object, name, wrap) {
    const descriptor = Object.getOwnPropertyDescriptor(object, name);
    let method = object[name];
    if (descriptor && !descriptor.configurable) {
      if (typeof method === 'function' && descriptor.writable) object[name] = wrap(method);
      return;
    }
    if (typeof method === 'function') method = wrap(method);
    Object.defineProperty(object, name, {
      configurable: true, enumerable: descriptor?.enumerable ?? true,
      get: () => method,
      set: value => { method = typeof value === 'function' ? wrap(value) : value; },
    });
  }
  function wrapIo(io) {
    if (!io || (typeof io !== 'object' && typeof io !== 'function') || watchedIo.has(io)) return io;
    watchedIo.add(io);
    // The actual 0.9 bundle assigns global io={} before assigning io.connect.
    // Observe that later method assignment synchronously, before IDE bootstrap.
    interceptMethod(io, 'connect', original => function (...args) {
      return bindSocket(original.apply(this, args));
    });
    // Socket.IO v2 uses Manager directly in Overleaf's shim.
    interceptMethod(io, 'Manager', manager => function (...args) {
      const instance = manager.apply(this, args);
      interceptMethod(instance, 'socket', original => function (...socketArgs) {
        return bindSocket(original.apply(this, socketArgs));
      });
      return instance;
    });
    return io;
  }
  let ioValue = window.io;
  const ioDescriptor = Object.getOwnPropertyDescriptor(window, 'io');
  if (!ioDescriptor || ioDescriptor.configurable) {
    Object.defineProperty(window, 'io', {
      configurable: true, enumerable: true,
      get: () => ioValue,
      set: value => { ioValue = wrapIo(value); },
    });
  }
  wrapIo(ioValue);

  function finishOpen() {
    if (!opening || read('editor.open_doc_id') !== opening.id) return;
    // doc:after-opened can precede the React effect that installs the new state.
    // Require the matching opened event and wait for the browser's event tick.
    if (!opening.opened || !opening.installed) return;
    const request = opening;
    clearTimeout(request.timer);
    clearTimeout(request.stepTimer);
    opening = null;
    queuePresence();
    downloadQueue.add(request.id); pumpDownloads();
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
  function openDocument(entry) {
    if (pending || (queuedEdit && queuedEdit.id !== entry.id)) { queuedOpen = entry; return; }
    if (!opening && read('editor.open_doc_id') === entry.id && current()) {
      downloadQueue.add(entry.id); pumpDownloads();
      if (queuedEdit?.id === entry.id) { const command = queuedEdit; queuedEdit = null; edit(command); }
      return;
    }
    if (opening) { clearTimeout(opening.timer); clearTimeout(opening.stepTimer); }
    opening = { id: entry.id, opened: false, installed: false, timer: setTimeout(() => {
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
      if (stopped || opening !== request) return;
      if (index < ancestors.length) {
        const node = clickId(ancestors[index]);
        if (node?.getAttribute('aria-expanded') === 'false') node.click();
        request.stepTimer = setTimeout(() => { request.stepTimer = null; activate(index + 1); }, 0);
        return;
      }
      const node = clickId(entry.id);
      if (node) node.click();
      else if (queuedEdit) {
        clearTimeout(request.timer); opening = null;
        rejectQueuedEdit('Open this Overleaf document before saving it. Your local edit is retained.');
      } else {
        // Official deep-link API for a document whose tree row is not mounted.
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
    if (stopped || !connected) return;
    for (const id of downloadQueue) {
      if (downloads.size >= 2) break;
      if (downloadingIds.has(id) || dirty.has(id) || pending?.id === id || opening?.id === id) continue;
      const entry = entries.get(id);
      downloadQueue.delete(id);
      if (entry) download(entry).catch(error => post('error', { message: error.message }));
    }
  }
  async function download(entry) {
    const controller = new AbortController();
    const revision = documentRevision.get(entry.id) || 0;
    const tree = treeRevision;
    downloads.add(controller);
    downloadingIds.add(entry.id);
    const timeout = setTimeout(() => controller.abort(), 30000);
    try {
      const suffix = entry.kind === 'doc' ? '/doc/' + entry.id + '/download' : '/file/' + entry.id;
      const response = await fetch('/project/' + project.project_id + suffix, { credentials: 'same-origin', redirect: 'error', signal: controller.signal });
      if (!response.ok) throw new Error('Overleaf could not download ' + entry.path + '.');
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
      if (stopped || !entries.has(entry.id)) return;
      if (tree !== treeRevision || revision !== (documentRevision.get(entry.id) || 0) || dirty.has(entry.id) || pending?.id === entry.id) {
        downloadQueue.add(entry.id);
        return;
      }
      if (entry.kind === 'doc') {
        const bytes = new Uint8Array(length);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        const source = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
        cacheAuthoritative(entry.id, source);
        post('document', { id: entry.id, path: entry.path, source });
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
    if (!connected || pending || opening || dirty.has(value.id) || value.source !== command.base) {
      rejectEdit(command, 'Overleaf changed before this edit could be applied. Your local edit is retained.');
      return;
    }
    if (typeof command.source !== 'string' || command.source.length > MAX_SOURCE || new TextEncoder().encode(command.source).byteLength > MAX_SOURCE || !Number.isSafeInteger(command.request) || command.request < 0) throw new Error('Invalid Overleaf edit.');
    if (command.source === value.source) {
      post('ack', { id: value.id, request: command.request, source: value.source });
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
    pending = { id: value.id, request: command.request, sent: false, acknowledged: false, timer: setTimeout(() => {
      status('saving', 'Still waiting for Overleaf to acknowledge this edit.');
    }, 30000) };
    status('saving');
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
    if (!pending || event.detail?.id !== pending.id || !pending.sent || !pending.acknowledged) return;
    const value = current();
    if (!value || value.id !== pending.id) return;
    const request = pending.request;
    clearTimeout(pending.timer);
    pending = null;
    cacheAuthoritative(value.id, value.source);
    post('ack', { id: value.id, request, source: value.source });
    status('connected');
    pumpDownloads();
    drainQueuedOpen();
  }
  function stop() {
    connected = false; clearPresence();
    clearTimeout(bootTimer);
    if (pending) clearTimeout(pending.timer);
    if (opening) { clearTimeout(opening.timer); clearTimeout(opening.stepTimer); }
    for (const controller of downloads) controller.abort();
    while (unwatch.length) unwatch.pop()();
    for (const [event, handler] of listeners) window.removeEventListener(event, handler);
    for (const [event, handler] of socketListeners) socket?.removeListener?.(event, handler);
    socket?.disconnect();
    stopped = true;
  }
  window.tiptoptypOverleaf = {
    command(command) {
      if (stopped || !command || typeof command !== 'object') return;
      if (command.epoch !== epoch) return;
      try {
        attachStore();
        switch (command.command) {
          case 'edit': requestEdit(command); break;
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
    if (opening && event.detail?.docId === opening.id) { opening.opened = true; setTimeout(finishOpen, 0); }
    else {
      const id = event.detail?.docId;
      if (entries.has(id)) { downloadQueue.add(id); pumpDownloads(); }
    }
  });
  listen('doc:saved', saved);
  listen('pagehide', stop);
  function boot() {
    if (stopped) return;
    wrapIo(window.io);
    if (attachStore()) { bootTimer = null; return; }
    if (++bootAttempts >= 120) {
      bootTimer = null;
      status(window.location.pathname.startsWith('/login') ? 'signed-out' : 'error', 'Overleaf editor is not ready. Sign in, or refresh the connection.');
      return;
    }
    bootTimer = setTimeout(boot, 250);
  }
  status('loading');
  boot();
})();
