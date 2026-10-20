// Adapter for the pinned Tinymist SVG frontend. Retain one pending input/frame;
// viewport demand stays current without the frontend's 500 ms scroll debounce.
(() => {
  let frame = 0;
  let pending = null;
  let lastNativeRequest = null;
  let sourceContextRequest = null;
  const prepared = new WeakSet();
  // The optional renderer keeps SVG ink only for the viewport and one viewport
  // on either side, snapped outwards to whole pages. Tinymist still supplies
  // every paper node, so navigation and document height remain unchanged.
  const partialDocs = new WeakMap();
  const svgLinkWindows = new WeakSet();
  const partialMode = doc => doc.partialRendering && doc.renderMode === 'svg' && doc.previewMode === 0 && !doc.isContentPreview;
  const fullMode = doc => !doc.partialRendering && doc.renderMode === 'svg' && doc.previewMode === 0 && !doc.isContentPreview;
  const connected = doc => document.getElementById('typst-container')?.documents?.some(value => value.impl === doc);
  // Only an explicit navigation request owns a beacon. Automatic source-follow
  // updates still scroll through the same frontend hook, without a typing pulse.
  const revealWindows = new WeakMap();
  let reveal = null, revealFrame = 0;
  const defaultRevealOptions = Object.freeze({enabled:true, style:"Rings", duration_ms:900, size_percent:100});
  let revealOptions = {...defaultRevealOptions};
  const revealDebug = { intents: 0, explicit: 0, locations: 0, retired: null, last_intent: null, last_location: null };
  const revealViewport = state => {
    const scroll = state?.doc?.hookedElem.parentElement;
    return [0, 0, window.innerWidth ?? scroll?.clientWidth ?? 0, window.innerHeight ?? scroll?.clientHeight ?? 0];
  };
  const retireReveal = (recapture = true, reason = 'finished') => {
    if (!reveal) return;
    if (revealFrame) cancelAnimationFrame(revealFrame);
    revealFrame = 0;
    reveal.element?.remove();
    revealDebug.retired = reason;
    reveal = null;
    window.tiptoptypZoomSnapshot?.invalidate();
    if (recapture) window.tiptoptypZoomSnapshot?.settle();
  };
  window.tiptoptypSetRevealOptions = options => {
    const bounded = (value, fallback, min, max) => Number.isFinite(value) ? Math.max(min, Math.min(max, Math.round(value))) : fallback;
    const next = {
      enabled: options?.enabled !== false,
      style: ['Rings','Pulse','Spotlight'].includes(options?.style) ? options.style : 'Rings',
      duration_ms: bounded(options?.duration_ms,900,250,2000),
      size_percent: bounded(options?.size_percent,100,50,200),
    };
    if (Object.keys(next).every(key => next[key] === revealOptions[key])) return;
    revealOptions = next;
    // Changing a preference retires the old cue instead of extending its timer.
    // No DOM, snapshot or frame work is needed when no cue is currently active.
    if (reveal) retireReveal(true, 'options-changed');
  };
  const revealPage = state => {
    const svg = state.doc.hookedElem.firstElementChild;
    if (!svg || svg.isConnected === false) return null;
    const cached = partialDocs.get(state.doc);
    if (cached?.nodeSVG === svg && cached.nodes) return cached.nodes[state.page] ?? null;
    if (state.svg !== svg || state.node?.isConnected === false) {
      state.svg = svg;
      // A renderer without a geometry cache is scanned once per replaced SVG,
      // never once per animation frame and never through the document ink.
      state.node = [...svg.children].find(node => node.classList?.contains('typst-page') &&
        Number(node.getAttribute('data-page-number')) === state.page) ?? null;
    }
    return state.node;
  };
  const drawReveal = state => {
    if (!state.element) {
      const element = document.createElement('div');
      element.className = 'tiptoptyp-reveal-beacon';
      element.setAttribute('aria-hidden', 'true');
      element.setAttribute('data-style', revealOptions.style);
      element.style.transform = `scale(${revealOptions.size_percent / 100})`;
      element.style.setProperty('--tiptoptyp-reveal-duration', `${revealOptions.duration_ms}ms`);
      element.style.setProperty('--tiptoptyp-reveal-second-duration', `${revealOptions.duration_ms * 74 / 90}ms`);
      element.style.setProperty('--tiptoptyp-reveal-second-delay', `${revealOptions.duration_ms * 16 / 90}ms`);
      for (const name of ['halo', 'ring', 'ring second', 'center']) {
        const part = document.createElement('span');
        part.className = `tiptoptyp-reveal-${name.replace(' ', ' tiptoptyp-reveal-')}`;
        element.append(part);
      }
      document.body.append(element);
      state.element = element;
    }
    state.element.style.left = `${state.center[0]}px`;
    state.element.style.top = `${state.center[1]}px`;
  };
  const revealExpired = (state, now) => now - (state.started ?? state.armed) >= (state.started === null ? 3000 : revealOptions.duration_ms);
  const currentReveal = now => reveal && !revealExpired(reveal, now) ? reveal : null;
  const tickReveal = () => {
    revealFrame = 0;
    const state = reveal;
    if (!state) return;
    const now = performance.now();
    if ((state.doc && (!connected(state.doc) || state.doc.hookedElem.isConnected === false || partialDocs.get(state.doc)?.disposed)) ||
        revealExpired(state, now)) {
      retireReveal(true, state.started === null ? 'arrival-timeout-or-disconnected' : 'finished-or-disconnected'); return;
    }
    if (state.page !== null) {
      const page = revealPage(state), geometry = page && sourcePageGeometry(page);
      if (geometry && page.getAttribute('data-dummy') !== '1') {
        const [, width, height, a, b, c, d, e, f] = geometry;
        const point = [a * state.x + c * state.y + e, b * state.x + d * state.y + f];
        if (state.started !== null) state.center = point;
        const rect = state.doc.hookedElem.parentElement.getBoundingClientRect();
        const viewport = revealViewport(state);
        const visible = state.x >= 0 && state.y >= 0 && state.x <= width && state.y <= height &&
          point[0] >= Math.max(0, rect.left) && point[0] <= Math.min(viewport[2], rect.right ?? rect.left + state.doc.hookedElem.parentElement.clientWidth) &&
          point[1] >= Math.max(0, rect.top) && point[1] <= Math.min(viewport[3], rect.bottom ?? rect.top + state.doc.hookedElem.parentElement.clientHeight);
        if (visible) {
          state.center = point;
          state.started ??= now;
          drawReveal(state);
          state.element.style.visibility = 'visible';
        } else if (state.element) state.element.style.visibility = 'hidden';
      } else if (state.element) state.element.style.visibility = 'hidden';
    }
    revealFrame = requestAnimationFrame(tickReveal);
  };
  const installRevealWindow = windowElem => {
    const original = windowElem?.handleTypstLocation;
    if (typeof original !== 'function' || revealWindows.get(windowElem) === original) return;
    const handler = function(elem, page, x, y, ...args) {
      const state = reveal;
      revealDebug.locations = Math.min(Number.MAX_SAFE_INTEGER, revealDebug.locations + 1);
      revealDebug.last_location = performance.now();
      if (state && state.page === null && Number.isSafeInteger(page) && page > 0 && [x, y].every(Number.isFinite)) {
        const docs = [...new Set((windowElem.documents ?? []).map(value => value.impl))];
        const doc = docs.find(value => value.hookedElem.firstElementChild === elem || value.hookedElem.firstElementChild?.contains?.(elem)) ??
          (docs.length === 1 ? docs[0] : null);
        if (doc && (!state.doc || state.doc === doc)) {
          state.doc = doc; state.page = page - 1; state.x = x; state.y = y;
        }
      }
      try { return original.call(this, elem, page, x, y, ...args); }
      catch (error) { retireReveal(true, 'handler-error'); throw error; }
    };
    revealWindows.set(windowElem, handler);
    windowElem.handleTypstLocation = handler;
    if (window.handleTypstLocation === original) window.handleTypstLocation = handler;
  };
  window.tiptoptypPreviewRevealIntent = explicit => {
    const wasActive = reveal !== null;
    revealDebug.intents = Math.min(Number.MAX_SAFE_INTEGER, revealDebug.intents + 1);
    revealDebug.explicit += explicit && revealDebug.explicit < Number.MAX_SAFE_INTEGER ? 1 : 0;
    revealDebug.last_intent = performance.now();
    retireReveal(false, explicit ? 'superseded' : 'automatic-follow');
    if (!explicit || !revealOptions.enabled) { if (wasActive) window.tiptoptypZoomSnapshot?.settle(); return; }
    const doc = documentRenderer();
    if (doc) installRevealWindow(doc.windowElem);
    reveal = { doc, page: null, x: 0, y: 0, armed: performance.now(), started: null, center: null, element: null, svg: null, node: null };
    window.tiptoptypZoomSnapshot?.invalidate();
    revealFrame = requestAnimationFrame(tickReveal);
  };
  window.tiptoptypPreviewRevealActive = () => currentReveal(performance.now()) !== null;
  window.tiptoptypPreviewRevealState = () => {
    const now = performance.now(), state = currentReveal(now);
    return {
      active: state !== null, pending: state !== null && state.started === null, page: state?.page ?? null,
      center: state?.center?.slice() ?? null, viewport: revealViewport(state),
      elapsed_ms: state ? now - (state.started ?? state.armed) : 0, options: {...revealOptions},
      debug: { intents: revealDebug.intents, explicit: revealDebug.explicit, locations: revealDebug.locations, retired: revealDebug.retired,
        intent_age_ms: revealDebug.last_intent === null ? null : now - revealDebug.last_intent,
        location_age_ms: revealDebug.last_location === null ? null : now - revealDebug.last_location },
    };
  };
  const pageGeometry = (doc, state) => {
    if (state.pages) return state.pages;
    let y = 0, sourceY = 0, width = 0;
    const pages = doc.kModule.retrievePagesInfo().map(page => {
      const sourceHeight = Math.max(0, Number(page.height) || 0);
      // The pinned WASM SVG exporter rounds each paper dimension upwards.
      // Layout/zoom must match that SVG, while render bounds remain in the
      // unrounded compiled-document coordinates (e.g. A4 is 595.276×841.89).
      const height = Math.ceil(sourceHeight);
      const item = { y, height, sourceY, sourceHeight, width: Math.ceil(Math.max(0, Number(page.width) || 0)) };
      y += height;
      sourceY += sourceHeight;
      width = Math.max(width, item.width);
      return item;
    });
    state.pages = { pages, width, height: y };
    return state.pages;
  };
  const partialLayout = doc => {
    const scroll = doc.hookedElem.parentElement, svg = doc.hookedElem.firstElementChild;
    const domState = doc.retrieveDOMState();
    return { domState, viewport: domState.boundingRect ?? scroll.getBoundingClientRect(),
      paper: svg?.getBoundingClientRect() ?? doc.hookedElem.getBoundingClientRect?.(),
      height: scroll.clientHeight, scrollTop: scroll.scrollTop };
  };
  const partialViewport = (doc, state, layout = partialLayout(doc)) => {
    const geometry = pageGeometry(doc, state);
    const scale = (layout.domState.width / (geometry.width || 1)) * doc.currentScaleRatio;
    const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
    const svg = doc.hookedElem.firstElementChild;
    const top = layout.paper ? (layout.viewport.top - layout.paper.top) / safeScale : layout.scrollTop / safeScale;
    const height = layout.height / safeScale;
    return { layout, geometry, scale: safeScale, top: Number.isFinite(top) ? top : 0,
      height: Number.isFinite(height) && height > 0 ? height : geometry.pages[0]?.height || 1,
      gap: 10 / safeScale, offsets: state.nodeSVG === svg && Math.abs(state.offsetScale - safeScale) < 1e-6 ? state.offsets : null };
  };
  const pageRange = (view, margin) => {
    const pages = view.geometry.pages;
    if (!pages.length) return { first: 0, last: -1 };
    const top = view.top - margin * view.height, bottom = view.top + (1 + margin) * view.height;
    const y = index => view.offsets?.[index] ?? pages[index].y + index * view.gap;
    // Two binary searches avoid scanning a long document on every scroll event.
    let lo = 0, hi = pages.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (y(mid) + pages[mid].height <= top) lo = mid + 1;
      else hi = mid;
    }
    const first = Math.min(lo, pages.length - 1);
    lo = first; hi = pages.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (y(mid) < bottom) lo = mid + 1;
      else hi = mid;
    }
    return { first, last: Math.max(first, lo - 1) };
  };
  const uncovered = (doc, state, margin = 0, view = partialViewport(doc, state)) => {
    if (!doc.moduleInitialized) return false;
    const range = pageRange(view, margin), coverage = state.coverage;
    return !coverage || coverage.svg !== doc.hookedElem.firstElementChild ||
      Math.abs(coverage.scale - view.scale) > 1e-6 ||
      range.first < coverage.first || range.last > coverage.last;
  };
  const acknowledgePartialPatch = (doc, state) => {
    const svg = doc.hookedElem.firstElementChild;
    if (svg) setAttributeIfChanged(svg, 'preserveAspectRatio', 'xMinYMin');
    state.coverage = { ...state.requested, svg };
    state.nodeSVG = svg;
    state.nodes = svg ? [...svg.children].filter(node => node.classList.contains('typst-page')) : [];
    const geometry = pageGeometry(doc, state);
    // Decorated paper offsets are authoritative after a successful patch. The
    // cached array makes scroll lookup logarithmic even for long documents.
    const offsets = state.nodes.map(node => Number(node.getAttribute('data-y')));
    const matches = state.nodes.length === geometry.pages.length && state.nodes.every((node, index) =>
      Number(node.getAttribute('data-page-width')) === geometry.pages[index].width &&
      Number(node.getAttribute('data-page-height')) === geometry.pages[index].height &&
      Number.isFinite(offsets[index]) && (index === 0 || offsets[index] >= offsets[index - 1]));
    state.offsets = matches ? offsets : null;
    state.offsetScale = state.requested.scale;
    state.cursorPosition = doc.cursorPosition; state.cursorPaths = doc.cursorPaths; state.pageColor = doc.pageColor;
    // The pinned patcher creates leaf cursor decorations only while applying a
    // patch with cursorPaths. Discover once, rather than scanning all ink on
    // every zoom frame; a later patch replaces this small list.
    state.ellipses = doc.cursorPaths ? [...(svg?.querySelectorAll?.('.typst-svg-cursor') ?? [])] : [];
  };
  const svgCoordinates = (svg, fallback) => {
    const matrix = svg?.getScreenCTM?.();
    if (matrix && [matrix.a, matrix.d, matrix.e, matrix.f].every(Number.isFinite) && matrix.a > 0 && matrix.d > 0) {
      return { x: matrix.e, y: matrix.f, sx: matrix.a, sy: matrix.d };
    }
    const rect = svg.getBoundingClientRect();
    return { x: rect.left, y: rect.top, sx: fallback, sy: fallback };
  };
  const invalidateFullGeometry = state => {
    state.fullDirty = true; state.pages = null; state.nodeSVG = null; state.nodes = null;
    state.offsets = null; state.coverage = null; state.ellipses = [];
  };
  const adoptFullGeometry = (doc, state) => {
    const svg = doc.hookedElem.firstElementChild;
    if (!svg) return false;
    const nodes = [...svg.children].filter(node => node.classList.contains('typst-page'));
    if (!nodes.length || nodes.length > 10000) return false;
    let y = 0, width = 0;
    const pages = nodes.map((node, index) => {
      const height = Number(node.getAttribute('data-page-height')), pageWidth = Number(node.getAttribute('data-page-width'));
      const page = { y, sourceY: y, height, sourceHeight: height, width: pageWidth };
      y += height; width = Math.max(width, pageWidth);
      return Number(node.getAttribute('data-page-number')) === index && height > 0 && pageWidth > 0 &&
        Number.isFinite(height) && Number.isFinite(pageWidth) ? page : null;
    });
    const offsets = nodes.map(node => Number(node.getAttribute('data-y')));
    if (pages.some(page => !page) || offsets.some((value, index) => !Number.isFinite(value) || (index && value < offsets[index - 1]))) return false;
    const nominal = doc.cachedDOMState.width / width * doc.currentScaleRatio;
    state.pages = {pages, width, height: y}; state.nodeSVG = svg; state.nodes = nodes;
    const unchanged = state.snapshotPages?.length === pages.length && state.snapshotPages.every((page, index) =>
      page[0] === offsets[index] && page[1] === pages[index].height);
    state.offsets = unchanged ? state.snapshotOffsets : offsets;
    state.offsetScale = svgCoordinates(svg, nominal).sy;
    state.coverage = {svg, first: 0, last: nodes.length - 1, scale: state.offsetScale};
    state.cursorPosition = doc.cursorPosition; state.cursorPaths = doc.cursorPaths; state.pageColor = doc.pageColor;
    state.ellipses = doc.cursorPaths ? [...(svg.querySelectorAll?.('.typst-svg-cursor') ?? [])] : [];
    state.fullDirty = false;
    return true;
  };
  const paperLayout = (geometry, scale) => ({
    positions: geometry.pages.map((page, index) => ({ x: (geometry.width - page.width) / 2, y: page.y + index * 10 / scale })),
    width: geometry.width, height: geometry.height + Math.max(0, geometry.pages.length - 1) * 10 / scale,
  });
  const sourcePoint = (geometry, offsets, y) => {
    let lo = 0, hi = offsets.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (offsets[mid] <= y) lo = mid + 1; else hi = mid; }
    const before = lo - 1, after = lo;
    if (before >= 0 && y <= offsets[before] + geometry.pages[before].height) return { page: before, local: y - offsets[before] };
    if (before >= 0 && after < offsets.length) {
      const end = offsets[before] + geometry.pages[before].height;
      return { before, after, fraction: (y - end) / (offsets[after] - end) };
    }
    const page = before < 0 ? 0 : before;
    return { page, local: y - offsets[page] };
  };
  const projectedPoint = (geometry, offsets, point) => {
    const validPage = index => Number.isInteger(index) && index >= 0 && index < offsets.length && index < geometry.pages.length;
    if (!point || typeof point !== 'object') return NaN;
    if (point.page !== undefined) return validPage(point.page) && Number.isFinite(point.local) ? offsets[point.page] + point.local : NaN;
    if (!validPage(point.before) || !validPage(point.after) || point.after <= point.before || !Number.isFinite(point.fraction)) return NaN;
    const start = offsets[point.before] + geometry.pages[point.before].height;
    return start + point.fraction * (offsets[point.after] - start);
  };
  const restorePartialZoomPoint = (doc, state, point, scale = state?.offsetScale) => {
    if (!point || !state?.offsets?.length || state.coverage?.svg !== doc.hookedElem.firstElementChild ||
        ![point.x, point.clientX, point.clientY, scale].every(Number.isFinite) || scale <= 0) return false;
    const geometry = pageGeometry(doc, state), svg = state.coverage.svg;
    const y = projectedPoint(geometry, state.offsets, point.y);
    if (!Number.isFinite(y)) return false;
    const rect = fullMode(doc) ? null : svg.getBoundingClientRect();
    const coordinates = rect ? {x: rect.left, y: rect.top, sx: scale, sy: scale} : svgCoordinates(svg, scale);
    const dx = coordinates.x + point.x * coordinates.sx - point.clientX;
    const dy = coordinates.y + y * coordinates.sy - point.clientY;
    // Even a no-op scrollBy cancels an in-flight WebKit smooth navigation.
    // Rounded SVG bounds can leave subpixel residue without a visible change.
    if (Math.abs(dx) > 0.5 || Math.abs(dy) > 0.5) doc.hookedElem.parentElement.scrollBy(dx, dy);
    // This explicit source point owns the transition. A saved frontend resize
    // anchor must not override it during a later paper-gap repair/rescale.
    doc.clearSvgResizeAnchor?.();
    return true;
  };
  const samePageRange = (left, right) => left.first === right.first && left.last === right.last;
  const setAttributeIfChanged = (node, name, value) => {
    const text = String(value);
    if (node.getAttribute(name) !== text) node.setAttribute(name, text);
  };
  const geometryRoots = (doc, state) => {
    const svg = doc.hookedElem.firstElementChild, geometry = pageGeometry(doc, state);
    if (!svg || svg !== state.nodeSVG || !geometry.pages.length || state.nodes.length !== geometry.pages.length ||
        Number(svg.getAttribute('data-width')) !== geometry.width ||
        doc.cursorPosition !== state.cursorPosition || doc.cursorPaths !== state.cursorPaths || doc.pageColor !== state.pageColor) return null;
    const backgrounds = new Map(), outers = [], circles = [];
    for (const node of svg.children) {
      if (node.classList.contains('typst-page-inner')) {
        const index = Number(node.getAttribute('data-page-number'));
        if (!Number.isInteger(index) || index < 0 || index >= geometry.pages.length || backgrounds.has(index)) return null;
        backgrounds.set(index, node);
      } else if (node.classList.contains('typst-page-outer')) outers.push(node);
      else if (node.tagName.toLowerCase() === 'circle' && node.getAttribute('fill') === '#86C166CC') circles.push(node);
    }
    if (backgrounds.size !== geometry.pages.length || outers.length !== 1) return null;
    for (let index = 0; index < geometry.pages.length; index++) {
      const page = geometry.pages[index], ink = state.nodes[index], background = backgrounds.get(index);
      if (ink.parentElement !== svg || background.parentElement !== svg ||
          Number(ink.getAttribute('data-page-number')) !== index ||
          Number(ink.getAttribute('data-page-width')) !== page.width || Number(ink.getAttribute('data-page-height')) !== page.height ||
          Number(background.getAttribute('width')) !== Math.floor(page.width * 100) ||
          Number(background.getAttribute('height')) !== Math.floor(page.height * 100) ||
          (index >= state.coverage.first && index <= state.coverage.last && ink.getAttribute('data-dummy') === '1')) return null;
    }
    const position = doc.cursorPosition, cursorPage = position?.[0] - 1;
    const hasCursor = Number.isInteger(cursorPage) && cursorPage >= 0 && cursorPage < geometry.pages.length;
    if (circles.length !== (hasCursor ? 1 : 0)) return null;
    if (hasCursor && (Number(circles[0].getAttribute('cx')) !== position[1] * 100 ||
        Number(circles[0].getAttribute('cy')) !== position[2] * 100)) return null;
    const ellipses = state.ellipses;
    const attached = node => { while (node && node !== svg) node = node.parentElement; return node === svg; };
    if (ellipses.some(node => !attached(node) || !Number.isFinite(Number(node.getAttribute('rx'))) ||
        !Number.isFinite(Number(node.getAttribute('ry'))) || Number(node.getAttribute('rx')) <= 0 || Number(node.getAttribute('ry')) <= 0)) return null;
    return { svg, backgrounds, outer: outers[0], circle: circles[0], cursorPage, ellipses };
  };
  const applyPaperLayout = (state, view, plan, offsets, roots, alignOrigin = false) => {
    const oldScale = state.offsetScale;
    for (let index = 0; index < state.nodes.length; index++) {
      const position = plan.positions[index], translate = `translate(${position.x}, ${position.y})`;
      setAttributeIfChanged(state.nodes[index], 'data-x', position.x);
      setAttributeIfChanged(state.nodes[index], 'data-y', position.y);
      setAttributeIfChanged(state.nodes[index], 'transform', translate);
      setAttributeIfChanged(roots.backgrounds.get(index), 'transform', `${translate} scale(0.01)`);
    }
    setAttributeIfChanged(roots.outer, 'data-page-height', Math.ceil(plan.height));
    setAttributeIfChanged(roots.outer, 'height', Math.ceil(plan.height));
    setAttributeIfChanged(roots.svg, 'data-height', plan.height);
    setAttributeIfChanged(roots.svg, 'viewBox', `0 0 ${plan.width} ${plan.height}`);
    // SVG's default centered alignment adds fractional top padding when its
    // CSS height is rounded. Keep ink at the paper origin so cached page slices
    // and source anchors survive resize without that extra letterbox offset.
    if (alignOrigin) setAttributeIfChanged(roots.svg, 'preserveAspectRatio', 'xMinYMin');
    if (roots.circle) {
      const position = plan.positions[roots.cursorPage];
      setAttributeIfChanged(roots.circle, 'r', 500 / view.scale);
      setAttributeIfChanged(roots.circle, 'transform', `translate(${position.x}, ${position.y}) scale(0.01)`);
    }
    for (const ellipse of roots.ellipses) {
      setAttributeIfChanged(ellipse, 'rx', Number(ellipse.getAttribute('rx')) * oldScale / view.scale);
      setAttributeIfChanged(ellipse, 'ry', Number(ellipse.getAttribute('ry')) * oldScale / view.scale);
    }
    // Ink is unchanged; only its paper placement and total scale changed.
    // Publish those offsets before the existing semantic resize anchor resolves.
    state.offsets = offsets;
    state.offsetScale = view.scale;
    state.coverage = { ...state.coverage, scale: view.scale };
  };
  const normalizeFullGeometry = (doc, state) => {
    if (!fullMode(doc)) return null;
    if ((state.fullDirty || state.nodeSVG !== doc.hookedElem.firstElementChild || !state.offsets) && !adoptFullGeometry(doc, state)) return null;
    const roots = geometryRoots(doc, state);
    if (!roots) { state.offsets = null; return null; }
    const geometry = state.pages, scale = doc.cachedDOMState.width / geometry.width * doc.currentScaleRatio;
    if (!Number.isFinite(scale) || scale <= 0) return null;
    const plan = paperLayout(geometry, scale), candidate = plan.positions.map(position => position.y);
    const offsets = candidate.length === state.offsets.length && candidate.every((value, index) => value === state.offsets[index]) ? state.offsets : candidate;
    // Cursor/span patches can arrive during the frontend's smooth source jump.
    // Only a real paper layout change owns viewport anchoring; restoring the
    // same layout would cancel that navigation as soon as it advances a pixel.
    let point = null;
    if (offsets !== state.offsets || Math.abs(scale - state.offsetScale) > 1e-6) {
      const viewport = doc.hookedElem.parentElement.getBoundingClientRect(), coordinates = svgCoordinates(roots.svg, state.offsetScale);
      point = { x: (viewport.left - coordinates.x) / coordinates.sx,
        y: sourcePoint(geometry, state.offsets, (viewport.top - coordinates.y) / coordinates.sy), clientX: viewport.left, clientY: viewport.top };
    }
    applyPaperLayout(state, {scale}, plan, offsets, roots);
    return {point, scale};
  };
  const reusePartialGeometry = (doc, state, view, point) => {
    const coverage = state.coverage;
    if (!coverage || coverage.svg !== doc.hookedElem.firstElementChild || !state.offsets) return null;
    const plan = paperLayout(view.geometry, view.scale), offsets = plan.positions.map(position => position.y);
    // A zoom can arrive before the first rescale. Predict its pointer-anchored
    // destination from page-local coordinates so geometry and scroll publish in
    // one layout, without briefly scaling the old paper gaps first.
    const target = point ? { ...view, offsets,
      top: projectedPoint(view.geometry, offsets, point.y) - (point.clientY - view.layout.viewport.top) / view.scale } : view;
    if (!samePageRange(pageRange(target, 1), coverage)) return null;
    const roots = geometryRoots(doc, state);
    if (!roots) return null;
    applyPaperLayout(state, view, plan, offsets, roots, true);
    doc.r.rescale();
    restorePartialZoomPoint(doc, state, point);
    const after = partialViewport(doc, state);
    // Anchoring can cross a page boundary. Refill before paint in that case.
    return samePageRange(pageRange(after, 1), state.coverage) ? after : null;
  };
  const cancelPartialFrame = state => {
    if (state.frame) cancelAnimationFrame(state.frame);
    state.frame = 0;
  };
  const schedulePartial = (doc, state) => {
    if (state.frame || state.running) return;
    doc.isRendering = true;
    state.frame = requestAnimationFrame(() => { state.frame = 0; flushPartial(doc, state); });
  };
  const flushPartial = (doc, state, view = null) => {
    cancelPartialFrame(state);
    if (!connected(doc) || !partialMode(doc) || state.disposed) { doc.isRendering = false; return; }
    if (state.running) return;
    state.running = true;
    doc.isRendering = true;
    let processed = false, rendered = false;
    const start = performance.now();
    try {
      const layout = view?.layout ?? partialLayout(doc);
      doc.cachedDOMState = layout.domState;
      prepare(doc);
      const geometryInput = state.geometryInput, zoomPoint = state.zoomPoint;
      let sourceChanged = false, render = state.force;
      state.force = false; state.geometryInput = false; state.zoomPoint = null;
      // Keep the frontend's source FIFO and processQueue merge/reset semantics.
      while (doc.patchQueue.length) {
        const change = doc.patchQueue.shift();
        render = doc.processQueue(change) || render;
        processed = true;
        if (change[0] === 'new' || change[0] === 'diff-v1') sourceChanged = true;
      }
      if (sourceChanged) { state.pages = null; state.coverage = null; state.offsets = null; }
      const currentView = view ?? (doc.moduleInitialized ? partialViewport(doc, state, layout) : null);
      const reused = doc.moduleInitialized && geometryInput && !processed && !render
        ? reusePartialGeometry(doc, state, currentView, zoomPoint) : null;
      if (!reused && doc.moduleInitialized && (geometryInput || render || uncovered(doc, state, 0.25, currentView))) {
        doc.r.rescale();
        // Deferred zoom did not scale/anchor the old SVG yet. Anchor its existing
        // offsets at the new scale before selecting a render window, otherwise
        // a distant page can be fetched once here and again after gap repair.
        if (!processed) restorePartialZoomPoint(doc, state, zoomPoint, currentView.scale);
        // The pinned SVG patcher is synchronous. Canvas fallback is disabled
        // below, so a distant scroll can refill its destination before paint.
        // Rescale can change the SVG's bounds or restore its scroll anchor.
        // Capture after that mutation and share the snapshot with fetch.
        state.renderView = partialViewport(doc, state);
        doc.cachedDOMState = state.renderView.layout.domState;
        try { doc.r.rerender(); } finally { state.renderView = null; }
        doc.r.rescale();
        acknowledgePartialPatch(doc, state);
        // A normal patch also recomputes fixed CSS paper gaps. Restore the same
        // source point after that geometry changes, unless queued source work
        // means the captured point may no longer identify the same content.
        if (!processed) restorePartialZoomPoint(doc, state, zoomPoint);
        rendered = true;
        // Source/resize anchoring can move the scroll position while patching.
        // Repair an uncovered destination once, in this same frame.
        const repairView = partialViewport(doc, state);
        if (uncovered(doc, state, 0, repairView)) {
          state.renderView = repairView;
          doc.cachedDOMState = repairView.layout.domState;
          try { doc.r.rerender(); } finally { state.renderView = null; }
          doc.r.rescale();
          acknowledgePartialPatch(doc, state);
          if (!processed) restorePartialZoomPoint(doc, state, zoomPoint);
        }
      }
      doc.sampledRenderTime = performance.now() - start;
    } catch (error) {
      state.coverage = null;
      window.tiptoptypZoomSnapshot?.failDocument?.(doc);
      console.error(error);
      processed = true;
    } finally {
      state.running = false;
      doc.isRendering = false;
      if (processed || rendered) doc.postprocessChanges();
      // Reentrant source messages are real work; viewport-only notifications
      // caused by rescale do not create an empty follow-up frame.
      if (doc.patchQueue.length || state.force) schedulePartial(doc, state);
      // A prefetch can become unnecessary before its RAF runs. Its busy flag
      // may have blocked the zoom's paint observation, so publish completion
      // even when there was no patch/postprocessing to notify that observer.
      if (!processed && !rendered) window.tiptoptypZoomSnapshot?.rendered();
    }
  };
  const requestPartialViewport = (doc, immediate = false, geometryInput = false, view = null, zoomPoint = null) => {
    const state = partialDocs.get(doc);
    if (!state || !partialMode(doc)) return false;
    view ??= partialViewport(doc, state);
    doc.cachedDOMState = view.layout.domState;
    state.geometryInput ||= geometryInput;
    if (zoomPoint) state.zoomPoint = zoomPoint;
    if (immediate || doc.patchQueue.length || uncovered(doc, state, 0, view)) flushPartial(doc, state, view);
    else if (state.force || state.geometryInput || uncovered(doc, state, 0.25, view)) schedulePartial(doc, state);
    return true;
  };
  const installPartialDocument = doc => {
    if (!doc || partialDocs.has(doc) || typeof doc.fetchSvgDataByDocMode !== 'function' ||
        typeof doc.kModule?.retrievePagesInfo !== 'function' || typeof doc.processQueue !== 'function' ||
        typeof doc.triggerUpdate !== 'function' || typeof doc.addChangement !== 'function') return;
    const state = { frame: 0, running: false, force: false, geometryInput: false, zoomPoint: null, pages: null, coverage: null, requested: null,
      nodeSVG: null, nodes: null, offsets: null, ellipses: [], offsetScale: 0, renderView: null, disposed: false, fullDirty: true };
    partialDocs.set(doc, state);
    const windowElem = doc.windowElem;
    installRevealWindow(windowElem);
    if (typeof windowElem?.initTypstSvg === 'function' && !svgLinkWindows.has(windowElem)) {
      svgLinkWindows.add(windowElem);
      const initialize = windowElem.initTypstSvg, initializedLinks = new WeakSet();
      windowElem.initTypstSvg = function(root, ...args) {
        const isDocumentSvg = root?.tagName?.toLowerCase() === 'svg' && windowElem.documents?.some(({impl}) =>
          impl.renderMode === 'svg' && impl.previewMode === 0 && !impl.isContentPreview &&
          impl.hookedElem.firstElementChild === root);
        if (!isDocumentSvg) return initialize.call(this, root, ...args);
        const links = [...root.getElementsByClassName('pseudo-link')].filter(link => !initializedLinks.has(link));
        if (!links.length) return;
        // The pinned initializer only enumerates pseudo-links before attaching
        // its hover handlers. Supply new nodes without cloning/reparenting ink.
        const result = initialize.call(this, { getElementsByClassName: () => links }, ...args);
        links.forEach(link => initializedLinks.add(link));
        return result;
      };
    }
    const fetch = doc.fetchSvgDataByDocMode;
    doc.fetchSvgDataByDocMode = function(...args) {
      if (!partialMode(this)) return fetch.apply(this, args);
      const view = state.renderView ?? partialViewport(this, state), range = pageRange(view, 1), pages = view.geometry.pages;
      state.requested = { ...range, scale: view.scale };
      const first = pages[range.first], last = pages[range.last];
      return this.kModule.render_in_window(0, first?.sourceY ?? 0, view.geometry.width,
        last ? last.sourceY + last.sourceHeight : 0);
    };
    const canvas = doc.shouldMixinCanvas;
    if (typeof canvas === 'function') doc.shouldMixinCanvas = function(...args) {
      return partialMode(this) ? false : canvas.apply(this, args);
    };
    const postprocess = doc.postprocessChanges;
    if (typeof postprocess === 'function') doc.postprocessChanges = function(...args) {
      if (fullMode(this) && connected(this) && this.moduleInitialized) {
        // A patch may reuse the SVG and preserve its total dimensions while
        // changing individual page heights, ink associations or cursor leaves.
        invalidateFullGeometry(state); prepare(this);
      }
      const result = postprocess.apply(this, args);
      // Full SVG rendering clears isRendering in a later RAF after its final
      // DOM mutation. Observe the frontend's completion, never poll for idle.
      if (!state.disposed && connected(this)) window.tiptoptypZoomSnapshot?.rendered();
      return result;
    };
    const trigger = doc.triggerUpdate;
    doc.triggerUpdate = function(...args) {
      if (!partialMode(this)) return trigger.apply(this, args);
      schedulePartial(this, state);
    };
    const add = doc.addChangement;
    doc.addChangement = function(change, ...args) {
      if (change[0] === 'new' || change[0] === 'diff-v1') {
        if (sourceContextRequest?.doc === this) sourceContextRequest = null;
        // scrollPreview also emits tiny diff-v1 cursor/span decorations after
        // its jump. Retain the real page point through incremental patches;
        // the next animation frame rebinds its geometry to the current SVG.
        if (change[0] === 'new' && reveal?.doc === this) retireReveal(true, 'source-replacement');
      }
      if (change[0] !== 'viewport-change') window.tiptoptypZoomSnapshot?.invalidate();
      if (!partialMode(this)) {
        if (fullMode(this) && (change[0] === 'new' || change[0] === 'diff-v1')) invalidateFullGeometry(state);
        return add.call(this, change, ...args);
      }
      if (this.vpTimeout !== undefined) { clearTimeout(this.vpTimeout); this.vpTimeout = undefined; }
      if (change[0] === 'viewport-change') state.force = true;
      else {
        if (change[0] === 'new') this.patchQueue.length = 0;
        this.patchQueue.push(change);
      }
      schedulePartial(this, state);
    };
    const reset = doc.reset;
    if (typeof reset === 'function') doc.reset = function(...args) {
      if (sourceContextRequest?.doc === this) sourceContextRequest = null;
      if (reveal?.doc === this) retireReveal(false, 'reset');
      if (!window.tiptoptypZoomSnapshot?.retireDocument?.(this)) window.tiptoptypZoomSnapshot?.invalidate();
      if (partialMode(this)) { state.pages = null; state.coverage = null; state.offsets = null; }
      else if (fullMode(this)) invalidateFullGeometry(state);
      return reset.apply(this, args);
    };
    doc.disposeList?.push(() => {
      if (sourceContextRequest?.doc === doc) sourceContextRequest = null;
      if (reveal?.doc === doc) retireReveal(false, 'dispose');
      if (!window.tiptoptypZoomSnapshot?.retireDocument?.(doc)) window.tiptoptypZoomSnapshot?.invalidate();
      state.disposed = true; cancelPartialFrame(state);
      if (partialMode(doc)) doc.isRendering = false;
    });
  };
  // This runs at document start, before Tinymist pushes its document and before
  // the socket's first source message. No SVG or initialized WASM is required.
  const watchedContainers = new WeakSet(), watchedArrays = new WeakSet();
  const watchDocuments = values => {
    if (!Array.isArray(values) || watchedArrays.has(values)) return values;
    watchedArrays.add(values);
    const identities = new Set();
    let count = 0;
    for (const value of values) {
      if (identities.has(value.impl)) continue;
      identities.add(value.impl); values[count++] = value; installPartialDocument(value.impl);
    }
    values.length = count;
    const push = values.push;
    values.push = function(...items) {
      for (const value of items) {
        if (this.some(existing => existing.impl === value.impl)) continue;
        installPartialDocument(value.impl); push.call(this, value);
      }
      return this.length;
    };
    return values;
  };
  const watchContainer = () => {
    const container = document.getElementById('typst-container');
    if (!container) return false;
    if (watchedContainers.has(container)) return true;
    watchedContainers.add(container);
    let values = watchDocuments(container.documents);
    Object.defineProperty(container, 'documents', { configurable: true, enumerable: true,
      get: () => values, set: next => { values = watchDocuments(next); } });
    return true;
  };
  if (!watchContainer()) {
    const observer = new MutationObserver(() => { if (watchContainer()) observer.disconnect(); });
    observer.observe(document.documentElement ?? document, { childList: true, subtree: true });
  }
  const documentRenderer = () => {
    const docs = document.getElementById('typst-container')?.documents;
    if (!docs?.length) return null;
    // Pinned Tinymist registers the same document again on socket reconnect.
    // Accept one renderer identity; genuinely distinct documents stay ambiguous.
    const doc = docs[0].impl;
    if (docs.some(value => value.impl !== doc)) return null;
    return doc?.moduleInitialized && doc.renderMode === 'svg' && doc.previewMode === 0 &&
      doc.hookedElem?.firstElementChild?.tagName.toLowerCase() === 'svg' &&
      typeof doc.retrieveDOMState === 'function' && typeof doc.r?.rescale === 'function'
      ? doc : null;
  };
  // The pinned SVG frontend already owns point-to-source mapping. Keep only
  // the context-menu request, then invoke that hook after the native choice;
  // dispatching a click here would also follow document links.
  let sourceContextToken = 0;
  const sourceContextElement = (target, svg) => {
    const element = target?.nodeType === 3 ? target.parentElement : target;
    return element?.isConnected === true && svg?.contains?.(element) ? element : null;
  };
  const sourcePageGeometry = page => {
    const pageNumber = page.getAttribute('data-page-number');
    const number = Number(pageNumber);
    const width = Number(page.getAttribute('data-page-width'));
    const height = Number(page.getAttribute('data-page-height'));
    const matrix = page.getScreenCTM?.();
    if (pageNumber === null || pageNumber.trim() === '' || !Number.isSafeInteger(number) || number < 0 ||
        !Number.isFinite(width) || width <= 0 || !Number.isFinite(height) || height <= 0 || !matrix) return null;
    const geometry = [number, width, height, matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
    return geometry.every(Number.isFinite) && matrix.a * matrix.d - matrix.b * matrix.c !== 0 ? geometry : null;
  };
  const sourcePageAtPoint = (svg, x, y) => {
    for (const page of svg.children) {
      if (page.tagName?.toLowerCase() !== 'g' || !page.classList?.contains('typst-page')) continue;
      const geometry = sourcePageGeometry(page);
      if (!geometry) continue;
      const [, width, height, a, b, c, d, e, f] = geometry;
      const determinant = a * d - b * c;
      const localX = (d * (x - e) - c * (y - f)) / determinant;
      const localY = (a * (y - f) - b * (x - e)) / determinant;
      if (localX < 0 || localY < 0 || localX > width || localY > height) continue;
      return page.getAttribute('data-dummy') === '1' ? null : { page, geometry };
    }
    return null;
  };
  const sourceContextGeometry = doc => {
    const scroll = doc.hookedElem.parentElement;
    return [doc.currentScaleRatio, scroll?.clientWidth, scroll?.clientHeight, scroll?.scrollLeft, scroll?.scrollTop];
  };
  window.tiptoptypRevealSource = token => {
    if (!Number.isSafeInteger(token) || token !== sourceContextRequest?.token) return false;
    const request = sourceContextRequest;
    sourceContextRequest = null;
    const doc = documentRenderer();
    if (doc !== request.doc || doc.hookedElem !== request.hook ||
        doc.hookedElem.firstElementChild !== request.svg || request.svg.isConnected !== true ||
        doc.hookedElem.sourceMappingHandler !== request.handler ||
        !sourceContextGeometry(doc).every((value, index) => value === request.geometry[index])) return false;
    const resolved = sourcePageAtPoint(request.svg, request.x, request.y);
    if (resolved?.page !== request.page ||
        !resolved.geometry.every((value, index) => value === request.pageGeometry[index])) return false;
    try {
      const pending = request.handler.call(request.hook, {
        target: request.page, clientX: request.x, clientY: request.y,
      });
      // Its socket may close after admission; never leave a rejected async
      // source-map request as an unhandled page error.
      pending?.catch?.(() => {});
      return true;
    } catch (_) { return false; }
  };
  document.addEventListener('contextmenu', event => {
    sourceContextRequest = null;
    const { clientX: x, clientY: y } = event;
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0 ||
        typeof window.ipc?.postMessage !== 'function') return;
    const doc = documentRenderer();
    const hook = doc?.hookedElem, svg = hook?.firstElementChild;
    if (!sourceContextElement(event.target, svg) || svg.isConnected !== true ||
        typeof hook?.sourceMappingHandler !== 'function' || sourceContextToken >= Number.MAX_SAFE_INTEGER) return;
    // WebKit can retain an old page as the hit-test target after a programmatic
    // scroll. Resolve the current paper under the point instead of that target.
    const resolved = sourcePageAtPoint(svg, x, y);
    if (!resolved) return;
    const geometry = sourceContextGeometry(doc);
    if (!geometry.every(Number.isFinite)) return;
    const token = ++sourceContextToken;
    sourceContextRequest = { token, doc, hook, svg, page: resolved.page, pageGeometry: resolved.geometry,
      handler: hook.sourceMappingHandler, geometry, x, y };
    event.preventDefault();
    event.stopImmediatePropagation();
    window.ipc.postMessage(JSON.stringify({ type: 'preview-context-menu', token, x, y }));
  }, true);
  const prepare = doc => {
    installRevealWindow(doc.windowElem);
    if (prepared.has(doc)) return;
    prepared.add(doc);
    // Tinymist rescales before, during, and after a viewport render. Only the
    // first changed geometry needs SVG layout/scroll anchoring. Source patches
    // which replace the SVG or change its geometry still invalidate this cache.
    const original = doc.r.rescale;
    let previous = null;
    const geometry = () => {
      const svg = doc.hookedElem.firstElementChild;
      return [svg, svg?.getAttribute('data-width'), svg?.getAttribute('data-height'),
        svg?.getAttribute('width'), svg?.getAttribute('height'),
        doc.cachedDOMState.width, doc.cachedDOMState.height, doc.currentScaleRatio,
        doc.hookedElem.style.height, doc.hookedElem.style.transform];
    };
    doc.r.rescale = () => {
      const next = geometry();
      const state = partialDocs.get(doc);
      if (previous && next.every((value, index) => value === previous[index]) && !(fullMode(doc) && state?.fullDirty)) return;
      const full = state && fullMode(doc) ? normalizeFullGeometry(doc, state) : null;
      // Our current layout/point supersedes any delayed frontend resize anchor,
      // including on cursor-only repaints where navigation owns the scroll.
      if (full) doc.clearSvgResizeAnchor?.();
      original();
      if (full) {
        state.offsetScale = svgCoordinates(doc.hookedElem.firstElementChild, full.scale).sy;
        state.coverage.scale = state.offsetScale;
        restorePartialZoomPoint(doc, state, full.point, state.offsetScale);
      }
      previous = geometry();
    };
  };
  const enqueue = (doc, change) => {
    prepare(doc);
    if (pending?.doc !== doc) pending = { doc, scale: doc.currentScaleRatio, resize: false, demand: false, zoom: false };
    change(pending);
    if (pending.zoom || pending.resize) window.tiptoptypZoomSnapshot?.begin(doc, pending.scale, { x: pending.x, y: pending.y });
    if (frame) return;
    const flush = () => {
      frame = 0;
      const input = pending;
      pending = null;
      const doc = documentRenderer();
      if (!doc || doc !== input.doc) {
        // Acceptance is not completion: a reset/disposal can remove this owner
        // between the native script and its queued frame. Terminate the serial
        // request so the next initialized renderer can accept input again.
        if (Number.isSafeInteger(input.nativeRevision)) window.tiptoptypZoomSnapshot?.abandon(input.nativeRevision);
        return;
      }
      if (Number.isSafeInteger(input.nativeRevision) && window.tiptoptypZoomSnapshot?.owns?.(input.nativeRevision, doc) === false) return;
      try {
        doc.cachedDOMState = doc.retrieveDOMState();
        const scroll = doc.hookedElem.parentElement;
        let zoomPoint = input.source ?? null, deferredView = null;
        if (input.zoom) {
          const rect = doc.hookedElem.firstElementChild.getBoundingClientRect();
          const viewport = scroll.getBoundingClientRect();
          const oldScale = doc.currentScaleRatio;
          const x = input.x ?? (viewport.left + scroll.clientWidth / 2);
          const y = input.y ?? (viewport.top + scroll.clientHeight / 2);
          const contentX = x - rect.left;
          const contentY = y - rect.top;
          const partial = partialDocs.get(doc);
          if (partial?.coverage?.svg === doc.hookedElem.firstElementChild && partial.offsets?.length) {
            if (partialMode(doc)) zoomPoint = { x: contentX / partial.coverage.scale,
              y: sourcePoint(pageGeometry(doc, partial), partial.offsets, contentY / partial.coverage.scale), clientX: x + (input.panX || 0), clientY: y + (input.panY || 0) };
            else if (fullMode(doc)) {
              const coordinates = svgCoordinates(doc.hookedElem.firstElementChild, partial.offsetScale);
              zoomPoint = { x: (x - coordinates.x) / coordinates.sx,
                y: sourcePoint(pageGeometry(doc, partial), partial.offsets, (y - coordinates.y) / coordinates.sy),
                clientX: x + (input.panX || 0), clientY: y + (input.panY || 0) };
            }
          }
          doc.currentScaleRatio = input.scale;
          const fit = Math.abs(input.scale - 1) < 1e-6;
          doc.hookedElem.classList.toggle('hide-scrollbar-x', fit);
          scroll.classList.toggle('hide-scrollbar-x', fit);
          if (input.source) zoomPoint = input.source;
          if (partialMode(doc) && zoomPoint && input.scroll) {
            const geometry = pageGeometry(doc, partial), scale = doc.cachedDOMState.width / geometry.width * input.scale;
            const offsets = paperLayout(geometry, scale).positions.map(p => p.y);
            zoomPoint = { x: input.scroll.left / scale, y: sourcePoint(geometry, offsets, input.scroll.top / scale), clientX: viewport.left, clientY: viewport.top };
          }
          if (partialMode(doc) && zoomPoint && !doc.patchQueue.length && !partial.force && !partial.running) {
            deferredView = partialViewport(doc, partial, { domState: doc.cachedDOMState, viewport, paper: rect,
              height: scroll.clientHeight, scrollTop: scroll.scrollTop });
          } else {
            doc.r.rescale();
            const after = doc.hookedElem.firstElementChild.getBoundingClientRect();
            const ratio = input.scale / oldScale;
            if (input.scroll) {
              scroll.scrollTo({ ...input.scroll, behavior: 'instant' });
              doc.clearSvgResizeAnchor?.();
            }
            else if (!(zoomPoint && fullMode(doc) && restorePartialZoomPoint(doc, partial, zoomPoint))) scroll.scrollBy(after.left + contentX * ratio - x - (input.panX || 0), after.top + contentY * ratio - y - (input.panY || 0));
          }
        } else if (input.resize) {
          doc.r.rescale();
          if (input.scroll) {
            scroll.scrollTo({ ...input.scroll, behavior: 'instant' });
            doc.clearSvgResizeAnchor?.();
          }
          else if (input.source) restorePartialZoomPoint(doc, partialDocs.get(doc), input.source);
        }
        if (doc.partialRendering && (input.demand || input.resize || input.zoom)) {
          if (!requestPartialViewport(doc, true, input.resize || input.zoom, deferredView, zoomPoint)) doc.addViewportChange();
        }
        updateControls();
        window.tiptoptypZoomSnapshot?.rendered();
      } catch (error) {
        window.tiptoptypZoomSnapshot?.failed?.(input.nativeRevision, doc);
        throw error;
      }
    };
    frame = requestAnimationFrame(flush);
  };
  const zoom = (doc, factor, reset = false, point = {}) => {
    if (window.tiptoptypZoomSnapshot?.request?.(factor, reset, point)) return;
    enqueue(doc, input => {
      input.scale = reset ? 1 : Math.min(10, Math.max(0.1, input.scale * factor));
      input.zoom = true;
      input.x = point.x;
      input.y = point.y;
    });
  };
  window.tiptoptypNativeZoom = request => {
    const doc = documentRenderer();
    if (!doc || !window.tiptoptypZoomSnapshot?.native(request, doc)) return false;
    lastNativeRequest = request;
    enqueue(doc, input => {
      input.scale = Math.min(10, Math.max(0.1, request.scale));
      input.zoom = !request.resize; input.resize = !!request.resize; input.x = request.x; input.y = request.y;
      input.scroll = request.scroll; input.source = request.source;
      input.panX = request.panX; input.panY = request.panY;
      input.nativeRevision = request.revision;
    });
    return true;
  };
  window.addEventListener('resize', event => {
    const doc = documentRenderer();
    if (!doc) return;
    event.stopImmediatePropagation();
    if (window.tiptoptypZoomSnapshot?.resize?.()) {
      const request = lastNativeRequest, scroll = doc.hookedElem.parentElement;
      // Native bounds can already contain a newer queued width while this
      // flight's first RAF fitted the previous DOM width. Refresh this same
      // owner once so readiness can release its serial slot for the successor.
      if (request && window.tiptoptypZoomSnapshot.owns?.(request.revision, doc) &&
          (doc.cachedDOMState.width !== scroll.clientWidth || doc.cachedDOMState.height !== scroll.clientHeight) &&
          !(pending?.doc === doc && pending.nativeRevision === request.revision)) {
        enqueue(doc, input => {
          input.resize = true;
          input.scroll = request.scroll; input.source = request.source;
          input.nativeRevision = request.revision;
        });
      }
      return;
    }
    enqueue(doc, input => { input.resize = true; });
    updateControls();
  }, true);
  window.addEventListener('scroll', event => {
    const doc = documentRenderer(), state = partialDocs.get(doc);
    const view = state && partialMode(doc) && event.target === doc.hookedElem.parentElement
      ? partialViewport(doc, state) : null;
    if (event.target === scrollElement()) {
      window.tiptoptypZoomSnapshot?.scrolled();
      recordNavigationScroll(view);
    }
    if (!doc?.partialRendering || event.target !== doc.hookedElem.parentElement) return;
    event.stopImmediatePropagation();
    doc.clearSvgResizeAnchor();
    if (!requestPartialViewport(doc, false, false, view)) enqueue(doc, input => { input.demand = true; });
  }, true);
  window.addEventListener('wheel', event => {
    const doc = documentRenderer();
    if (!event.ctrlKey || !doc || !doc.windowElem.contains(event.target)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const unit = event.deltaMode === 1 ? 20 : event.deltaMode === 2
      ? doc.hookedElem.parentElement.clientHeight : 1;
    zoom(doc, Math.exp(-event.deltaY * unit * 0.002), false, { x: event.clientX, y: event.clientY });
  }, { capture: true, passive: false });
  const command = action => {
    const doc = documentRenderer();
    if (!doc) return false;
    if (action === 'in') zoom(doc, 1.1);
    else if (action === 'out') zoom(doc, 1 / 1.1);
    else if (action === 'reset') zoom(doc, 1, true);
    else return false;
    updateControls();
    return true;
  };

  // UI lives in the shared native controls window. This adapter only reports
  // geometry and carries out viewer actions; no parallel HTML toolbar.
  let lastState = '';
  const back = [];
  const forward = [];
  const pushHistory = (history, value) => {
    if (history.length === 256) history.shift();
    history.push(value);
  };
  let pendingNavigation = null;
  const scrollElement = () => documentRenderer()?.hookedElem.parentElement;
  const position = () => {
    const scroll = scrollElement();
    return scroll ? { top: scroll.scrollTop, left: scroll.scrollLeft } : null;
  };
  const restore = value => {
    window.tiptoptypZoomSnapshot?.invalidate();
    const doc = documentRenderer();
    doc?.clearSvgResizeAnchor();
    scrollElement()?.scrollTo({left:value.left,top:value.top,behavior:'instant'});
    if (doc) requestPartialViewport(doc, true);
  };
  const prepareHistory = () => {
    const current = position();
    pendingNavigation = current ? { ...current, at: performance.now() } : null;
  };
  const pages = () => {
    const doc = documentRenderer(), svg = doc?.hookedElem.firstElementChild;
    const state = partialDocs.get(doc);
    if (state && partialMode(doc) && state.nodeSVG === svg && state.nodes) return state.nodes;
    return svg ? [...svg.children].filter(node => node.tagName.toLowerCase() === 'g') : [];
  };
  const pageBounds = page => {
    // A <g>'s client rect bounds its ink, not the paper (and may be empty).
    // The renderer supplies paper dimensions and a transform for every page.
    const matrix = page.getScreenCTM?.();
    const width = Number(page.getAttribute?.('data-page-width'));
    const height = Number(page.getAttribute?.('data-page-height'));
    if (matrix && width > 0 && height > 0) {
      return {left:matrix.e, top:matrix.f, right:matrix.e + width * matrix.a, bottom:matrix.f + height * matrix.d};
    }
    return page.getBoundingClientRect();
  };
  const currentPage = (partialView = null) => {
    const scroll = scrollElement();
    if (!scroll) return 0;
    const doc = documentRenderer(), state = partialDocs.get(doc);
    if (state && partialMode(doc)) {
      const view = partialView ?? partialViewport(doc, state), range = pageRange(view, 0);
      let current = 0, visible = 0;
      for (let index = range.first; index <= range.last; index++) {
        const page = view.geometry.pages[index], top = view.offsets?.[index] ?? page.y + index * view.gap;
        const overlap = Math.max(0, Math.min(top + page.height, view.top + view.height) - Math.max(top, view.top));
        if (overlap > visible) { current = index; visible = overlap; }
      }
      return current;
    }
    const all = pages();
    const viewport = scroll.getBoundingClientRect();
    // Linked headings are positioned below the viewport's top edge. Prefer
    // the page occupying most of the viewport over the previous page's tail.
    // Papers are ordered vertically. Reading every screen transform during
    // zoom forces needless geometry work in the full viewer on long documents.
    let lo = 0, hi = all.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (pageBounds(all[mid]).bottom <= viewport.top) lo = mid + 1;
      else hi = mid;
    }
    let current = 0, visible = 0;
    for (let index = lo; index < all.length; index++) {
      const rect = pageBounds(all[index]);
      if (rect.top >= viewport.bottom) break;
      const overlap = Math.max(0, Math.min(rect.bottom, viewport.bottom) - Math.max(rect.top, viewport.top));
      if (overlap > visible) { current = index; visible = overlap; }
    }
    return current;
  };
  const gotoPage = page => {
    const scroll = scrollElement();
    const target = pages()[page];
    if (!scroll || !target) return;
    const before = position();
    const top = pageBounds(target).top - scroll.getBoundingClientRect().top + scroll.scrollTop;
    if (Math.abs(top - scroll.scrollTop) < 1) return;
    pendingNavigation = null;
    documentRenderer()?.clearSvgResizeAnchor();
    if (before) pushHistory(back, before);
    forward.length = 0;
    scroll.scrollTo({ top, behavior: 'instant' });
    requestPartialViewport(documentRenderer(), true);
    updateControls();
  };
  window.tiptoptypZoomSnapshot?.attach(() => {
    const doc = documentRenderer(), scroll = doc?.hookedElem?.parentElement, svg = doc?.hookedElem?.firstElementChild;
    if (!scroll || !svg) return null;
    const state = partialDocs.get(doc);
    // Native capture covers the web view, including scrollbar/margin pixels.
    // Keep its image coordinates separate from the SVG fit-to-width dimension.
    const paper = svg.getBoundingClientRect(), viewport = scroll.getBoundingClientRect();
    const piecewise = state?.offsets && state.nodeSVG === svg && (partialMode(doc) || fullMode(doc));
    if (piecewise && state.snapshotOffsets !== state.offsets) {
      state.snapshotOffsets=state.offsets;
      state.snapshotPages=state.offsets.map((y,i)=>[y,state.pages.pages[i].height]);
    }
    return { doc, svg, scale: doc.currentScaleRatio, background: palette[0],
      geometry: { paper:[paper.left,paper.top,paper.width,paper.height], viewport:[viewport.left,viewport.top,scroll.clientWidth,scroll.clientHeight],
        view:[window.innerWidth ?? scroll.clientWidth,window.innerHeight ?? scroll.clientHeight], scroll:[scroll.scrollLeft,scroll.scrollTop],
        source_scale:piecewise ? state.offsetScale : 0,
        content_size:piecewise ? [state.pages.width,state.pages.height] : [0,0],
        pixel_rounded:piecewise && fullMode(doc),
        pages:piecewise ? state.snapshotPages : [] },
      width: window.innerWidth ?? scroll.clientWidth, height: window.innerHeight ?? scroll.clientHeight,
      fitWidth: doc.cachedDOMState.width, x: 0, y: 0, left: scroll.scrollLeft, top: scroll.scrollTop,
      ready: doc.moduleInitialized && !doc.isRendering && !doc.patchQueue.length && !pending &&
        doc.cachedDOMState.width === scroll.clientWidth &&
        (!partialMode(doc) || (state?.coverage?.svg === svg &&
          Math.abs(state.coverage.scale - doc.cachedDOMState.width / (state.pages?.width || 1) * doc.currentScaleRatio) < 1e-6)) };
  });
  const updateControls = (partialView = null) => {
    window.tiptoptypZoomSnapshot?.rendered();
    document.documentElement?.classList.toggle('tiptoptyp-preview-partial', partialMode(documentRenderer() ?? {}));
    document.documentElement?.classList.toggle('tiptoptyp-preview-focused', document.hasFocus());
    const state = JSON.stringify({ type: 'preview-state', page: currentPage(partialView?.layout ? partialView : null), count: pages().length,
      focused: document.hasFocus(), zoom: documentRenderer()?.currentScaleRatio ?? 1, back: back.length > 0, forward: forward.length > 0 });
    if (state !== lastState) { lastState = state; window.ipc?.postMessage(state); }
  };
  window.tiptoptypPreviewAction = ({ action, value }) => {
    if (action === 'zoom-in') command('in');
    else if (action === 'zoom-out') command('out');
    else if (action === 'fit') command('reset');
    else if (action === 'page') gotoPage(value);
    else if (action === 'location') {
      const doc = documentRenderer();
      if (doc && Number.isFinite(value?.page) && Number.isFinite(value?.x) && Number.isFinite(value?.y)) {
        prepareHistory();
        window.tiptoptypPreviewRevealIntent(true);
        doc.windowElem.handleTypstLocation?.(doc.hookedElem.firstElementChild,value.page+1,value.x,value.y);
        requestPartialViewport(doc, true);
      }
    }
    else if (action === 'find' || action === 'outline') prepareHistory();
    else if (action === 'back' || action === 'forward') {
      const from = action === 'back' ? back : forward;
      const to = action === 'back' ? forward : back;
      const target = from.pop(), here = position();
      if (target && here) { pendingNavigation = null; pushHistory(to,here); restore(target); }
    }
    updateControls();
  };
  let palette = [[255,255,255],[0,0,0]];
  let borders = ['1px solid transparent', '1px solid transparent'];
  let accent = [0,0,0];
  const applyAccent = () => {
    document.documentElement.style.setProperty('--tiptoptyp-reveal-accent', `rgb(${accent.join(',')})`);
    document.documentElement.style.setProperty('--tiptoptyp-reveal-wash', `rgba(${accent.join(',')},0.2)`);
  };
  const applyBorders = () => {
    document.documentElement.style.setProperty('--tiptoptyp-hover-border', borders[0]);
    document.documentElement.style.setProperty('--tiptoptyp-focus-border', borders[1]);
  };
  let hovering=false;
  const reportOutline=()=>{
    const stroke=document.hasFocus()?borders[1]:hovering?borders[0]:'';
    const match=(stroke??'').match(/^([\d.]+)px solid rgba\(([^)]+)\)$/);
    window.tiptoptypZoomSnapshot?.outline?.(match?Number(match[1]):0,match?match[2].split(',').map(Number):[0,0,0,0]);
  };
  let filter;
  const applyPalette = () => {
    if (!document.body) return;
    if (!filter) {
      const namespace = 'http://www.w3.org/2000/svg';
      const svg = document.createElementNS(namespace,'svg');
      svg.style.cssText = 'position:absolute;width:0;height:0;pointer-events:none';
      filter = document.createElementNS(namespace,'filter');
      filter.id = 'tiptoptyp-palette';
      filter.setAttribute('color-interpolation-filters','sRGB');
      const transfer = document.createElementNS(namespace,'feComponentTransfer');
      for (const channel of ['R','G','B']) {
        const component = document.createElementNS(namespace,`feFunc${channel}`);
        component.setAttribute('type','linear'); transfer.append(component);
      }
      filter.append(transfer); svg.append(filter); document.body.append(svg);
    }
    const [bg,fg] = palette;
    // The identity transfer still allocates filtered page surfaces in WebKit.
    // Both SVG viewers can bypass that exact identity operation; tinted and
    // inverted pages retain the same channel transfer.
    const identity = bg.every(value => value === 255) && fg.every(value => value === 0);
    document.documentElement.style.setProperty('--tiptoptyp-preview-filter', identity ? 'none' : 'url(#tiptoptyp-palette)');
    [...filter.firstElementChild.children].forEach((channel,index) => {
      channel.setAttribute('slope',String((bg[index]-fg[index])/255));
      channel.setAttribute('intercept',String(fg[index]/255));
    });
    document.documentElement.style.setProperty('--tiptoptyp-paper', `rgb(${bg.join(',')})`);
    document.documentElement.style.setProperty('--tiptoptyp-ink', `rgb(${fg.join(',')})`);
    applyBorders(); applyAccent();
  };
  window.tiptoptypSetPalette = (bg,fg,hover,focus,nextAccent) => {
    const equal = (left,right) => left.every((channel,index) => channel === right[index]);
    const paperChanged = !equal(palette[0],bg) || !equal(palette[1],fg);
    const bordersChanged = borders[0] !== hover || borders[1] !== focus;
    const accentChanged = !equal(accent,nextAccent);
    if (!paperChanged && !bordersChanged && !accentChanged) return;
    palette = [bg,fg]; borders = [hover,focus]; accent = nextAccent;
    if (paperChanged) {
      window.tiptoptypZoomSnapshot?.invalidate();
      applyPalette();
    } else {
      if (bordersChanged) applyBorders();
      if (accentChanged) applyAccent();
    }
    if (bordersChanged) reportOutline();
    if (paperChanged || bordersChanged) window.tiptoptypZoomSnapshot?.settle();
  };
  const mount = () => {
    const style = document.createElement('style');
    // WKWebView can apply an HTML-ancestor SVG filter in takeSnapshot while
    // omitting it from the on-screen compositor. Keep filters inside the SVG,
    // one per page, and recolor the renderer's separate paper rectangles directly.
    style.textContent = `
      html:hover::before, html.tiptoptyp-preview-focused::before {
        content: ''; position: fixed; inset: 0; z-index: 2147483646; pointer-events: none;
        border: var(--tiptoptyp-hover-border);
      }
      html.tiptoptyp-preview-focused::before { border: var(--tiptoptyp-focus-border); }
      /* Preserve the pinned ripple's animationend cleanup, while replacing its
         subtle teal ring with our explicit-navigation-only destination cue. */
      .typst-jump-ripple { visibility: hidden !important; pointer-events: none !important; }
      .tiptoptyp-reveal-beacon { position: fixed; width: 0; height: 0; z-index: 2147483645; pointer-events: none; color: var(--tiptoptyp-reveal-accent); transform-origin: 0 0; }
      .tiptoptyp-reveal-beacon span { position: absolute; box-sizing: border-box; pointer-events: none; }
      .tiptoptyp-reveal-halo { width: 34px; height: 34px; left: -17px; top: -17px; border-radius: 50%;
        border: 2px solid #fff; box-shadow: 0 0 0 2px #171717, inset 0 0 0 2px #171717;
        background: var(--tiptoptyp-reveal-wash); animation: tiptoptyp-reveal-center var(--tiptoptyp-reveal-duration,900ms) ease-out both; }
      .tiptoptyp-reveal-ring { width: 18px; height: 18px; left: 0; top: 0; transform: translate(-50%, -50%); border: 3px solid currentColor; border-radius: 50%;
        box-shadow: 0 0 0 1px #171717, inset 0 0 0 1px #fff; animation: tiptoptyp-reveal-ring var(--tiptoptyp-reveal-duration,900ms) ease-out both; }
      .tiptoptyp-reveal-second { animation-duration: var(--tiptoptyp-reveal-second-duration,740ms); animation-delay: var(--tiptoptyp-reveal-second-delay,160ms); }
      .tiptoptyp-reveal-center { width: 20px; height: 20px; left: -10px; top: -10px;
        animation: tiptoptyp-reveal-center var(--tiptoptyp-reveal-duration,900ms) ease-out both; }
      .tiptoptyp-reveal-center::before, .tiptoptyp-reveal-center::after { content: ''; position: absolute; background: currentColor;
        box-shadow: 0 0 0 1px #171717, 0 0 0 2px #fff; border-radius: 1px; }
      .tiptoptyp-reveal-center::before { width: 20px; height: 2px; left: 0; top: 9px; }
      .tiptoptyp-reveal-center::after { width: 2px; height: 20px; left: 9px; top: 0; }
      .tiptoptyp-reveal-beacon[data-style="Pulse"] .tiptoptyp-reveal-ring,
      .tiptoptyp-reveal-beacon[data-style="Spotlight"] .tiptoptyp-reveal-ring { display: none; }
      .tiptoptyp-reveal-beacon[data-style="Pulse"] .tiptoptyp-reveal-halo {
        width: 54px; height: 54px; left: -27px; top: -27px; border-color: currentColor;
        animation-name: tiptoptyp-reveal-pulse;
      }
      .tiptoptyp-reveal-beacon[data-style="Spotlight"] .tiptoptyp-reveal-halo {
        width: 172px; height: 172px; left: -86px; top: -86px; border: none; box-shadow: none;
        background: radial-gradient(circle,var(--tiptoptyp-reveal-wash),transparent 70%);
      }
      @keyframes tiptoptyp-reveal-pulse { 0% { opacity: 1; transform: scale(0.8); } 55% { opacity: 0.8; transform: scale(1.1); } 100% { opacity: 0; transform: scale(1); } }
      @keyframes tiptoptyp-reveal-ring { from { width: 18px; height: 18px; opacity: 1; } to { width: 126px; height: 126px; opacity: 0; } }
      @keyframes tiptoptyp-reveal-center { 0%, 55% { opacity: 1; } 100% { opacity: 0; } }
      #typst-container-top { display: none !important; }
      body { background: var(--tiptoptyp-paper) !important; }
      #typst-container .typst-page-inner { fill: var(--tiptoptyp-paper); }
      #typst-container g[data-page-width] { filter: var(--tiptoptyp-preview-filter, url(#tiptoptyp-palette)); }
    `;
    document.head.append(style);
    applyPalette();
    document.documentElement?.addEventListener?.('mouseenter',()=>{hovering=true;reportOutline();});
    document.documentElement?.addEventListener?.('mouseleave',()=>{hovering=false;reportOutline();});
    reportOutline(); updateControls();
    const container = document.getElementById('typst-container');
    if (container) new MutationObserver(() => {
      // Mutations during our zoom are the live patch being covered. Source
      // changes invalidate through addChangement before they are processed.
      updateControls();
    }).observe(container,{ childList:true, subtree:true });
  };
  document.addEventListener('click', event => {
    const anchor = event.target.closest?.('a');
    if (!anchor) return;
    const href = anchor.getAttribute('href') ?? anchor.getAttribute('xlink:href') ?? '';
    let internal = href.startsWith('#');
    if (!internal) {
      try { internal = new URL(href, location.href).origin === location.origin; }
      catch (_) { return; }
    }
    if (!internal) return;
    prepareHistory();
  }, true);
  document.addEventListener('DOMContentLoaded', mount);
  if (document.readyState !== 'loading') mount();
  const recordNavigationScroll = (partialView = null) => {
    const now = position();
    if (pendingNavigation && performance.now() - pendingNavigation.at < 2000 && now &&
        (Math.abs(now.top - pendingNavigation.top) > 1 || Math.abs(now.left - pendingNavigation.left) > 1)) {
      pushHistory(back, { top: pendingNavigation.top, left: pendingNavigation.left });
      forward.length = 0;
      pendingNavigation = null;
    }
    updateControls(partialView);
  };
  window.addEventListener('tiptoptyp-preview-zoom', event => command(event.detail));
  window.addEventListener('focus',()=>{reportOutline();updateControls();});
  window.addEventListener('blur',()=>{reportOutline();updateControls();});
  window.addEventListener('keydown', event => {
    if (!document.hasFocus()) return;
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopImmediatePropagation();
      window.ipc?.postMessage(JSON.stringify({type:'focus-editor'}));
      return;
    }
    if (event.target?.closest?.('input, textarea, [contenteditable]')) { event.stopImmediatePropagation(); return; }
    if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey &&
        (event.key === 'Home' || event.key === 'End')) {
      event.preventDefault(); event.stopImmediatePropagation();
      gotoPage(event.key === 'Home' ? 0 : pages().length - 1);
      return;
    }
    if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key === 't') {
      event.preventDefault(); event.stopImmediatePropagation();
      window.tiptoptypZoomSnapshot?.invalidate();
      palette = [palette[1],palette[0]]; applyPalette();
      window.tiptoptypZoomSnapshot?.settle();
      window.ipc?.postMessage(JSON.stringify({type:'invert-preview'}));
      return;
    }
    if (!(event.metaKey || event.ctrlKey)) return;
    const action = event.key === '=' || event.key === '+' ? 'in'
      : event.key === '-' ? 'out' : event.key === '0' ? 'reset' : null;
    if (!action || !command(action)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
})();
