// Capture admission and sharp-render acknowledgements only. Pixels and gesture
// transforms belong to the native compositor, never this renderer's JS thread.
(() => {
  const MAX_PIXELS = 4_000_000;
  let enabled = false, readFrame, cache = null, capture = null, admission = null, timer = 0;
  let active = false, epoch = 0, sourceEpoch = 0, serial = 0, nativeRequest = null, expectedScroll = null, handoffEpoch = null, frameEpoch = null;
  const counters = { captures: 0, hits: 0, misses: 0, failures: 0, handoffs: 0 };
  const namespace = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  const post = message => window.tiptoptypCaptureZoomSnapshot?.(message);
  const usable = frame => frame?.ready && frame.svg?.isConnected !== false && frame.width > 0 && frame.height > 0;
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
  const invalidate = (abort = false) => {
    epoch++; sourceEpoch++; clearTimeout(timer); timer = 0; cache = null; admission = null; if (abort) nativeRequest = null; expectedScroll = null; handoffEpoch = null; frameEpoch = null; active = !!nativeRequest;
    post({ op: 'invalidate', abort: !!abort });
  };
  const settle = () => {
    if (!enabled || active || capture || admission || timer || !readFrame) return;
    const frame = readFrame();
    if (!usable(frame)) return;
    if (frameEpoch !== epoch) { post({ op: 'frame', scale: frame.scale, width: frame.width, height: frame.height }); frameEpoch = epoch; }
    if (same(cache?.frame, frame) && !cache.refresh) return;
    timer = setTimeout(() => {
      timer = 0;
      if (!enabled || active || capture || admission) return;
      const start = readFrame();
      if (!usable(start)) return;
      const token = `${namespace}:${++serial}`;
      capture = { token, frame: start, epoch }; counters.captures++;
      const buffer = window.tiptoptypSnapshotBuffer?.(start) ?? null;
      const width = buffer?.width ?? start.width, height = buffer?.height ?? start.height;
      const density = Math.min((window.devicePixelRatio || 1) * 2, Math.sqrt(MAX_PIXELS / (width * height)));
      post({ op: 'capture', token, width: start.width, height: start.height, fitWidth: start.fitWidth ?? start.width,
        density, buffer, geometry: start.geometry, background: start.background });
    }, 150);
  };
  const resolve = (token, result) => {
    if (capture?.token !== token) return;
    const request = capture; capture = null;
    if (!enabled || request.epoch !== epoch || active || !same(request.frame, readFrame())) {
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
    if (enabled && accepted && pending.sourceEpoch === sourceEpoch) cache = pending.cache;
    else if (accepted) post({ op: 'reject', token });
    settle();
  };
  const begin = () => {
    // Also cancel capture on a cold-cache zoom. No image decode, upload, DOM
    // transform, extra animation frame, or screenshot is started by input.
    epoch++; clearTimeout(timer); timer = 0; handoffEpoch = null;
    return false;
  };
  const rendered = () => {
    if (!active) { settle(); return; }
    const frame = readFrame(), request = nativeRequest;
    if (!usable(frame) || frame.scale !== request?.scale) return;
    const generation = epoch;
    if (handoffEpoch === generation) return;
    handoffEpoch = generation;
    requestAnimationFrame(() => {
      if (!active || epoch !== generation) return;
      handoffEpoch = null;
      const settled = readFrame();
      if (!usable(settled) || settled.scale !== request.scale) return;
      // The renderer can finish its anchor scroll at this display boundary.
      // Retain that final position so its deferred scroll event cannot retire
      // the warmed image immediately after a successful sharp handoff.
      expectedScroll = { left: settled.left, top: settled.top };
      active = false; nativeRequest = null;
      // Keep usable pixels through the idle recapture gap. Source/palette/scroll
      // invalidation still retires them immediately; a refresh replaces them.
      if (cache?.token === request.token) cache = { ...cache, frame: settled, refresh: true };
      post({ op: 'ready', token: request.token, revision: request.revision, scale: request.scale });
      counters.handoffs++; settle();
    });
  };
  const scrolled = () => {
    const frame = readFrame?.();
    // The native zoom owns scroll anchoring until its sharp frame is ready.
    if (active || (expectedScroll && frame && Math.abs(expectedScroll.left - frame.left) < 1 && Math.abs(expectedScroll.top - frame.top) < 1)) return;
    invalidate(); settle();
  };
  window.tiptoptypZoomSnapshot = {
    attach(reader) { readFrame = reader; settle(); },
    configure(value) {
      const next = !!value && typeof window.tiptoptypCaptureZoomSnapshot === 'function';
      if (next === enabled) return;
      enabled = next; invalidate(true); post({ op: 'configure', enabled }); settle();
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
      if (!active) { begin(); active = true; }
      post({ op: 'resize' }); return true;
    },
    request(factor, reset = false, point = {}) {
      if (!enabled) return false;
      settle();
      post({ op: 'zoom', factor, reset, x: point.x, y: point.y }); return true;
    },
    native(request) {
      if (!enabled || !Number.isFinite(request.scale) || !Number.isSafeInteger(request.revision)) return false;
      // A request using the new token also proves native acceptance if the
      // renderer receives that request before the acknowledgement callback.
      if (admission?.cache.token === request.token && admission.sourceEpoch === sourceEpoch) {
        cache = admission.cache; admission = null;
      }
      if (cache && request.token !== cache.token) cache = null;
      begin(); nativeRequest = request; active = true;
      cache ? counters.hits++ : counters.misses++;
      return true;
    },
    begin, rendered, settle, scrolled, invalidate, resolve, admitted,
    inspect() { const frame = readFrame?.(); return { enabled, active, capturing: !!capture, admitting: !!admission, cached: !!cache,
      pixels: cache ? [cache.width, cache.height] : null, request: nativeRequest ? { revision: nativeRequest.revision, scale: nativeRequest.scale, token: nativeRequest.token } : null,
      frame_ready: !!usable(frame), frame_scale: frame?.scale ?? null, handoff_pending: handoffEpoch !== null, ...counters }; }
  };
  window.addEventListener('pagehide', () => invalidate(true));
})();
