// Capture admission and sharp-render acknowledgements only. Pixels and gesture
// transforms belong to the native compositor, never this renderer's JS thread.
(() => {
  const MAX_PIXELS = 4_000_000;
  let enabled = false, readFrame, cache = null, capture = null, admission = null, timer = 0;
  let active = false, epoch = 0, sourceEpoch = 0, serial = 0, nativeRequest = null, nativeOwner = null, expectedScroll = null, handoffEpoch = null, frameEpoch = null;
  const counters = { captures: 0, hits: 0, misses: 0, failures: 0, handoffs: 0 };
  let currentOutline=null;
  const namespace = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  const post = message => window.tiptoptypCaptureZoomSnapshot?.(message);
  const usable = frame => frame?.ready && frame.svg?.isConnected !== false && frame.width > 0 && frame.height > 0 &&
    (!frame.geometry?.scroll || frame.geometry.scroll.every(value => Number.isFinite(value) && value >= 0));
  // Destination beacons are transient UI, never part of reusable zoom pixels.
  const captureReady = frame => usable(frame) && !window.tiptoptypPreviewRevealActive?.();
  const sameGeometry = (a, b) => {
    if (a === b) return true;
    if (!a || !b) return false;
    // The reader retains immutable page offsets until their layout changes.
    // Empty offsets carry no layout identity; fresh empty arrays are harmless.
    const pages = a.pages?.length ? a.pages : null, otherPages = b.pages?.length ? b.pages : null;
    const paper = a.paper ?? [], otherPaper = b.paper ?? [];
    return pages === otherPages && (a.source_scale ?? 0) === (b.source_scale ?? 0) &&
      paper.length === otherPaper.length && paper.every((value, index) => value === otherPaper[index]);
  };
  const same = (a, b) => a && b && a.doc === b.doc && a.svg === b.svg && a.scale === b.scale &&
    a.width === b.width && a.height === b.height && a.fitWidth === b.fitWidth && a.left === b.left && a.top === b.top &&
    sameGeometry(a.geometry, b.geometry);
  const invalidate = (abort = false, notify = true) => {
    epoch++; sourceEpoch++; clearTimeout(timer); timer = 0; cache = null; admission = null; if (abort) { nativeRequest = null; nativeOwner = null; } expectedScroll = null; handoffEpoch = null; frameEpoch = null; active = !!nativeRequest;
    if (notify) post({ op: 'invalidate', abort: !!abort });
  };
  const settle = () => {
    if (!enabled || active || capture || admission || timer || !readFrame) return;
    const frame = readFrame();
    if (!captureReady(frame)) return;
    if (frameEpoch !== epoch) { post({ op: 'frame', scale: frame.scale, width: frame.width, height: frame.height }); frameEpoch = epoch; if (currentOutline) post({op:'outline',...currentOutline}); }
    if (same(cache?.frame, frame) && !cache.refresh) return;
    timer = setTimeout(() => {
      timer = 0;
      if (!enabled || active || capture || admission) return;
      const start = readFrame();
      if (!captureReady(start)) return;
      const token = `${namespace}:${++serial}`;
      capture = { token, frame: start, epoch }; counters.captures++;
      const buffer = window.tiptoptypSnapshotBuffer?.(start) ?? null;
      const width = buffer?.width ?? start.width, height = buffer?.height ?? start.height;
      const density = Math.min((window.devicePixelRatio || 1) * 2, Math.sqrt(MAX_PIXELS / (width * height)));
      post({ op: 'capture', token, width: start.width, height: start.height, fitWidth: start.fitWidth ?? start.width,
        density, buffer, geometry: start.geometry,
        background: buffer?.paperBackground?.slice(0,3) ?? window.tiptoptypSnapshotBackdrop?.(start)?.slice(0,3) ?? start.background });
    }, 150);
  };
  const resolve = (token, result) => {
    if (capture?.token !== token) return;
    const request = capture; capture = null;
    if (!enabled || request.epoch !== epoch || active || !captureReady(readFrame()) || !same(request.frame, readFrame())) {
      post({ op: 'reject', token }); settle(); return;
    }
    if (!result || result.width <= 0 || result.height <= 0 || result.width * result.height > MAX_PIXELS) {
      counters.failures++; if (cache) cache.refresh = false; post({ op: 'reject', token }); return;
    }
    admission = { sourceEpoch, cache: { frame: request.frame, width: result.width, height: result.height, token } };
    post({ op: 'admit', token, scale: request.frame.scale });
  };
  const admitted = (token, accepted) => {
    if (admission?.cache.token !== token) {
      if (accepted && cache?.token !== token) post({ op: 'reject', token });
      return;
    }
    const pending = admission; admission = null;
    if (enabled && accepted && pending.sourceEpoch === sourceEpoch && captureReady(readFrame())) cache = pending.cache;
    else if (accepted) post({ op: 'reject', token });
    settle();
  };
  const begin = () => {
    // Also cancel capture on a cold-cache zoom. No image decode, upload, DOM
    // transform, extra animation frame, or screenshot is started by input.
    epoch++; clearTimeout(timer); timer = 0; handoffEpoch = null;
    return false;
  };
  // The browser may already fit a newer viewport while an older native request
  // occupies the serial slot. Readiness uses CURRENT live layout, never the
  // dispatched request's former width; native retains the mask for pending work.
  const matchesRequest = (frame, request) => usable(frame) && frame.scale === request?.scale;
  const rendered = () => {
    if (!active) { settle(); return; }
    const frame = readFrame(), request = nativeRequest;
    if (frame?.doc && nativeOwner && frame.doc !== nativeOwner) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
    if (!matchesRequest(frame, request)) return;
    const generation = epoch;
    if (handoffEpoch === generation) return;
    handoffEpoch = generation;
    requestAnimationFrame(() => {
      if (!active || epoch !== generation) return;
      const painted = readFrame();
      if (painted?.doc && nativeOwner && painted.doc !== nativeOwner) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
      if (!matchesRequest(painted, request)) { handoffEpoch = null; return; }
      // RAF runs before paint. Keep the native raster through a paint opportunity,
      // then release only if the latest vector geometry remained unchanged.
      requestAnimationFrame(() => {
        if (!active || epoch !== generation) return;
        handoffEpoch = null;
        const settled = readFrame();
        if (settled?.doc && nativeOwner && settled.doc !== nativeOwner) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
        if (!matchesRequest(settled, request)) return;
        if (!same(painted, settled)) { rendered(); return; }
        // Retain the final anchor so its deferred scroll event cannot retire
        // warmed pixels immediately after the sharp frame is presented.
        expectedScroll = { doc: settled.doc, svg: settled.svg, left: settled.left, top: settled.top };
        active = false; nativeRequest = null; nativeOwner = null;
        if (cache?.token === request.token) cache = { ...cache, frame: settled, refresh: true };
        post({ op: 'ready', token: request.token, revision: request.revision, scale: request.scale });
        counters.handoffs++; settle();
      });
    });
  };
  const scrolled = () => {
    const frame = readFrame?.();
    // The native zoom owns scroll anchoring until its sharp frame is ready.
    if (!active && cache && frame && cache.frame.doc === frame.doc && cache.frame.svg === frame.svg && (cache.frame.width !== frame.width || cache.frame.height !== frame.height)) {
      // A browser clamp can notify scroll before resize. Native layout owns
      // this transition; keep its source point and pixels until its handoff.
      window.tiptoptypZoomSnapshot.resize(); return;
    }
    if (active || (cache && frame && cache.frame.doc === frame.doc && cache.frame.svg === frame.svg && cache.frame.left === frame.left && cache.frame.top === frame.top) || (expectedScroll && frame && expectedScroll.doc === frame.doc && expectedScroll.svg === frame.svg && Math.abs(expectedScroll.left - frame.left) < 1 && Math.abs(expectedScroll.top - frame.top) < 1)) return;
    invalidate(); settle();
  };
  window.tiptoptypZoomSnapshot = {
    attach(reader) { readFrame = reader; settle(); },
    configure(value) {
      const next = !!value && typeof window.tiptoptypCaptureZoomSnapshot === 'function';
      if (next === enabled) return;
      enabled = next; invalidate(true); post({ op: 'configure', enabled }); if (enabled && currentOutline) post({op:'outline',...currentOutline}); settle();
    },
    outline(width,color) {
      const next={width,color};
      if (JSON.stringify(next) === JSON.stringify(currentOutline)) return;
      currentOutline=next; if (enabled) post({op:'outline',...next});
    },
    resize() {
      // A cold startup has no native image or acknowledgement to wait for.
      // Let Tinymist size itself; active gestures keep their native owner.
      const frame = readFrame?.();
      if (!enabled || !frame?.doc || (!cache && !active)) return false;
      // A notification for the just-acknowledged viewport requires no work.
      // Native bounds already match it and therefore generate no new request.
      if (!active && cache?.frame.width === frame.width && cache.frame.height === frame.height) return true;
      // AppKit can deliver the DOM resize after the matching native layout
      // request has already rendered. A repeated size notification sends no
      // replacement request; it must not cancel that flight's paint/ready RAF.
      // A genuinely newer native request advances the epoch in native().
      if (!active) begin();
      // Native layout may already have applied this size. Only native(request)
      // can open an active flight; an accepted notification is not an owner.
      post({ op: 'resize' }); return true;
    },
    request(factor, reset = false, point = {}) {
      if (!enabled) return false;
      settle();
      post({ op: 'zoom', factor, reset, x: point.x, y: point.y }); return true;
    },
    native(request, owner = readFrame?.()?.doc) {
      if (!enabled || !Number.isFinite(request.scale) || !Number.isSafeInteger(request.revision)) return false;
      // A request using the new token also proves native acceptance if the
      // renderer receives that request before the acknowledgement callback.
      if (admission?.cache.token === request.token && admission.sourceEpoch === sourceEpoch) {
        cache = admission.cache; admission = null;
      }
      if (cache && request.token !== cache.token) cache = null;
      begin(); nativeRequest = request; nativeOwner = owner; active = true;
      cache ? counters.hits++ : counters.misses++;
      return true;
    },
    owns(revision, owner) {
      return active && nativeRequest?.revision === revision && nativeOwner === owner;
    },
    retireDocument(owner) {
      if (nativeOwner !== owner || !nativeRequest) return false;
      return this.abandon(nativeRequest.revision);
    },
    abandon(revision) {
      if (nativeRequest?.revision !== revision) return false;
      post({ op: 'abandon', revision });
      return this.rejected(revision);
    },
    rejected(revision) {
      if (nativeRequest && nativeRequest.revision !== revision) return false;
      // Native already retired this exact request. Echoing an unversioned abort
      // could cancel a newer flight whose script is still queued in WebContent.
      invalidate(true, false); settle(); return true;
    },
    begin, rendered, settle, scrolled, invalidate, resolve, admitted,
    inspect() { const frame = readFrame?.(); return { enabled, active, capturing: !!capture, admitting: !!admission, cached: !!cache,
      pixels: cache ? [cache.width, cache.height] : null, request: nativeRequest ? { revision: nativeRequest.revision, scale: nativeRequest.scale, token: nativeRequest.token } : null,
      frame_ready: !!usable(frame), frame_scale: frame?.scale ?? null, handoff_pending: handoffEpoch !== null, ...counters }; }
  };
  window.addEventListener('pagehide', () => invalidate(true));
})();
