// Capture admission and sharp-render acknowledgements only. Pixels and gesture
// transforms belong to the native compositor, never this renderer's JS thread.
(() => {
  const MAX_PIXELS = 4_000_000;
  let enabled = false, captureDeferred = false, readFrame, cache = null;
  // Warming owns exactly one scheduled capture, capture reply or admission.
  // Native rendering is independent: an in-flight capture may need rejection
  // after a gesture starts, while retained pixels still belong to that gesture.
  let warming = { phase: 'idle' }, flight = { phase: 'idle' };
  let epoch = 0, sourceEpoch = 0, serial = 0, expectedScroll = null, frameEpoch = null;
  const captureRequest = () => warming.phase === 'capturing' ? warming.request : null;
  const admissionRequest = () => warming.phase === 'admitting' ? warming.request : null;
  const warmingBusy = () => warming.phase !== 'idle';
  const isActive = () => flight.phase !== 'idle';
  const flightRequest = () => isActive() ? flight.request : null;
  const flightOwner = () => isActive() ? flight.owner : null;
  const resumeRendering = () => { if (isActive()) flight = { phase: 'rendering', request: flight.request, owner: flight.owner }; };
  const cancelWarmTimer = () => {
    if (warming.phase === 'scheduled') { clearTimeout(warming.timer); warming = { phase: 'idle' }; }
  };
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
    epoch++; sourceEpoch++; cancelWarmTimer(); cache = null;
    if (warming.phase === 'admitting') warming = { phase: 'idle' };
    if (abort) flight = { phase: 'idle' }; else resumeRendering();
    expectedScroll = null; frameEpoch = null;
    if (notify) post({ op: 'invalidate', abort: !!abort });
  };
  const settle = () => {
    if (!enabled || isActive() || !readFrame) return;
    // Once reported, blocked warming must remain cheap through editor frames.
    if (frameEpoch === epoch && (captureDeferred || warmingBusy())) return;
    const frame = readFrame();
    if (!usable(frame)) return;
    // Gesture readiness belongs to the live renderer, not optional screenshots.
    // Pixel preparation, editor feedback and reveal beacons may delay warming
    // without leaving the native controller uninitialized on a ready document.
    if (frameEpoch !== epoch) { post({ op: 'frame', scale: frame.scale, width: frame.width, height: frame.height }); frameEpoch = epoch; if (currentOutline) post({op:'outline',...currentOutline}); }
    if (captureDeferred || warmingBusy() || !captureReady(frame)) return;
    if (same(cache?.frame, frame) && cache.phase !== 'refresh-needed') return;
    const timer = setTimeout(() => {
      if (warming.phase !== 'scheduled' || warming.timer !== timer) return;
      warming = { phase: 'idle' };
      if (!enabled || captureDeferred || isActive() || captureRequest() || admissionRequest()) return;
      const start = readFrame();
      if (!captureReady(start)) return;
      const token = `${namespace}:${++serial}`;
      warming = { phase: 'capturing', request: { token, frame: start, epoch } }; counters.captures++;
      const buffer = window.tiptoptypSnapshotBuffer?.(start) ?? null;
      const width = buffer?.width ?? start.width, height = buffer?.height ?? start.height;
      const density = Math.min((window.devicePixelRatio || 1) * 2, Math.sqrt(MAX_PIXELS / (width * height)));
      post({ op: 'capture', token, width: start.width, height: start.height, fitWidth: start.fitWidth ?? start.width,
        density, buffer, geometry: start.geometry,
        background: buffer?.paperBackground?.slice(0,3) ?? window.tiptoptypSnapshotBackdrop?.(start)?.slice(0,3) ?? start.background });
    }, 150);
    warming = { phase: 'scheduled', timer };
  };
  const resolve = (token, result) => {
    if (captureRequest()?.token !== token) return;
    const request = captureRequest(); warming = { phase: 'idle' };
    if (!enabled || request.epoch !== epoch || isActive() || !captureReady(readFrame()) || !same(request.frame, readFrame())) {
      post({ op: 'reject', token }); settle(); return;
    }
    if (!result || result.width <= 0 || result.height <= 0 || result.width * result.height > MAX_PIXELS) {
      counters.failures++; if (cache) cache.phase = 'fresh'; post({ op: 'reject', token }); return;
    }
    warming = { phase: 'admitting', request: { sourceEpoch, cache: { phase: 'fresh', frame: request.frame, width: result.width, height: result.height, token } } };
    post({ op: 'admit', token, scale: request.frame.scale });
  };
  const admitted = (token, accepted) => {
    if (admissionRequest()?.cache.token !== token) {
      if (accepted && cache?.token !== token) post({ op: 'reject', token });
      return;
    }
    const pending = admissionRequest(); warming = { phase: 'idle' };
    if (enabled && accepted && pending.sourceEpoch === sourceEpoch && captureReady(readFrame())) cache = pending.cache;
    else if (accepted) post({ op: 'reject', token });
    settle();
  };
  const begin = () => {
    // Also cancel capture on a cold-cache zoom. No image decode, upload, DOM
    // transform, extra animation frame, or screenshot is started by input.
    epoch++; cancelWarmTimer(); resumeRendering();
    return false;
  };
  // The browser may already fit a newer viewport while an older native request
  // occupies the serial slot. Readiness uses CURRENT live layout, never the
  // dispatched request's former width; native retains the mask for pending work.
  const matchesRequest = (frame, request) => usable(frame) && frame.scale === request?.scale;
  const rendered = () => {
    if (!isActive()) { settle(); return; }
    const frame = readFrame(), request = flightRequest();
    if (frame?.doc && flightOwner() && frame.doc !== flightOwner()) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
    if (!matchesRequest(frame, request)) return;
    const generation = epoch;
    if (flight.phase === 'handoff' && flight.epoch === generation) return;
    flight = { phase: 'handoff', request, owner: flightOwner(), epoch: generation };
    requestAnimationFrame(() => {
      if (!isActive() || epoch !== generation) return;
      const painted = readFrame();
      if (painted?.doc && flightOwner() && painted.doc !== flightOwner()) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
      if (!matchesRequest(painted, request)) { resumeRendering(); return; }
      // RAF runs before paint. Keep the native raster through a paint opportunity,
      // then release only if the latest vector geometry remained unchanged.
      requestAnimationFrame(() => {
        if (!isActive() || epoch !== generation) return;
        resumeRendering();
        const settled = readFrame();
        if (settled?.doc && flightOwner() && settled.doc !== flightOwner()) { window.tiptoptypZoomSnapshot.abandon(request.revision); return; }
        if (!matchesRequest(settled, request)) return;
        if (!same(painted, settled)) { rendered(); return; }
        // Retain the final anchor so its deferred scroll event cannot retire
        // warmed pixels immediately after the sharp frame is presented.
        expectedScroll = { doc: settled.doc, svg: settled.svg, left: settled.left, top: settled.top };
        flight = { phase: 'idle' };
        if (cache?.token === request.token) cache = { ...cache, frame: settled, phase: 'refresh-needed' };
        // Retained pixels start the next gesture from the actual rounded SVG
        // position, not the previous gesture's predicted scroll position.
        const geometry = settled.geometry;
        post({ op: 'ready', token: request.token, revision: request.revision, scale: request.scale,
          position: geometry && { x: geometry.paper?.[0], y: geometry.paper?.[1],
            scale: geometry.source_scale, width: settled.width, height: settled.height } });
        counters.handoffs++; settle();
      });
    });
  };
  const scrolled = () => {
    const frame = readFrame?.();
    // The native zoom owns scroll anchoring until its sharp frame is ready.
    if (!isActive() && cache && frame && cache.frame.doc === frame.doc && cache.frame.svg === frame.svg && (cache.frame.width !== frame.width || cache.frame.height !== frame.height)) {
      // A browser clamp can notify scroll before resize. Native layout owns
      // this transition; keep its source point and pixels until its handoff.
      window.tiptoptypZoomSnapshot.resize(); return;
    }
    if (isActive() || (cache && frame && cache.frame.doc === frame.doc && cache.frame.svg === frame.svg && cache.frame.left === frame.left && cache.frame.top === frame.top) || (expectedScroll && frame && expectedScroll.doc === frame.doc && expectedScroll.svg === frame.svg && Math.abs(expectedScroll.left - frame.left) < 1 && Math.abs(expectedScroll.top - frame.top) < 1)) return;
    invalidate(); settle();
  };
  window.tiptoptypZoomSnapshot = {
    attach(reader) { readFrame = reader; settle(); },
    deferCaptures(value) {
      const next = !!value;
      if (next === captureDeferred) return;
      captureDeferred = next;
      cancelWarmTimer();
      if (!next) settle();
    },
    deferred(token) {
      if (captureRequest()?.token !== token) return;
      warming = { phase: 'idle' };
      settle();
    },
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
      if (!enabled || !frame?.doc || (!cache && !isActive())) return false;
      // A notification for the just-acknowledged viewport requires no work.
      // Native bounds already match it and therefore generate no new request.
      if (!isActive() && cache?.frame.width === frame.width && cache.frame.height === frame.height) return true;
      // AppKit can deliver the DOM resize after the matching native layout
      // request has already rendered. A repeated size notification sends no
      // replacement request; it must not cancel that flight's paint/ready RAF.
      // A genuinely newer native request advances the epoch in native().
      if (!isActive()) begin();
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
      if (admissionRequest()?.cache.token === request.token && admissionRequest().sourceEpoch === sourceEpoch) {
        cache = admissionRequest().cache; warming = { phase: 'idle' };
      }
      if (cache && request.token !== cache.token) cache = null;
      begin(); flight = { phase: 'rendering', request, owner };
      cache ? counters.hits++ : counters.misses++;
      return true;
    },
    owns(revision, owner) {
      return isActive() && flightRequest()?.revision === revision && flightOwner() === owner;
    },
    failed(revision, owner) {
      if (!this.owns(revision, owner)) return false;
      // Preserve the live renderer's applied scale, even if its SVG update
      // failed partway through. The next user input can retry; idle does not.
      post({ op: 'render-failed', revision, owner: flightRequest().owner, scale: owner.currentScaleRatio });
      invalidate(true, false); settle(); return true;
    },
    failDocument(owner) {
      return flightRequest() ? this.failed(flightRequest().revision, owner) : false;
    },
    retireDocument(owner) {
      if (flightOwner() !== owner || !flightRequest()) return false;
      return this.abandon(flightRequest().revision);
    },
    abandon(revision) {
      if (flightRequest()?.revision !== revision) return false;
      post({ op: 'abandon', revision });
      return this.rejected(revision);
    },
    rejected(revision) {
      if (flightRequest() && flightRequest().revision !== revision) return false;
      // Native already retired this exact request. Echoing an unversioned abort
      // could cancel a newer flight whose script is still queued in WebContent.
      invalidate(true, false); settle(); return true;
    },
    retired(tokens) {
      // Native may retire pixels without DOM motion. Do not cancel an active
      // vector request or echo invalidate: only matching image metadata is old.
      let changed = false;
      if (cache && tokens.includes(cache.token)) { cache = null; changed = true; }
      if (admissionRequest() && tokens.includes(admissionRequest().cache.token)) { warming = { phase: 'idle' }; changed = true; }
      if (changed) settle();
    },
    begin, rendered, settle, scrolled, invalidate, resolve, admitted,
    inspect() { const frame = readFrame?.(); return { warming_phase: warming.phase, native_phase: flight.phase, cache_phase: cache?.phase ?? 'empty', enabled, capture_deferred: captureDeferred, active: isActive(), capturing: !!captureRequest(), admitting: !!admissionRequest(), cached: !!cache,
      pixels: cache ? [cache.width, cache.height] : null, request: flightRequest() ? { revision: flightRequest().revision, scale: flightRequest().scale, token: flightRequest().token } : null,
      frame_ready: !!usable(frame), frame_scale: frame?.scale ?? null, handoff_pending: flight.phase === 'handoff', ...counters }; }
  };
  window.addEventListener('pagehide', () => invalidate(true));
})();
