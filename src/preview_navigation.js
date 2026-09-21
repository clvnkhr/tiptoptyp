// Adapter for the pinned Tinymist SVG frontend. Retain one pending input/frame;
// viewport demand stays current without the frontend's 500 ms scroll debounce.
(() => {
  let frame = 0;
  let pending = null;
  const prepared = new WeakSet();
  const documentRenderer = () => {
    const docs = document.getElementById('typst-container')?.documents;
    if (docs?.length !== 1) return null;
    const doc = docs[0].impl;
    return doc?.moduleInitialized && doc.renderMode === 'svg' && doc.previewMode === 0 &&
      doc.hookedElem?.firstElementChild?.tagName.toLowerCase() === 'svg' &&
      typeof doc.retrieveDOMState === 'function' && typeof doc.r?.rescale === 'function'
      ? doc : null;
  };
  const prepare = doc => {
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
      if (previous && next.every((value, index) => value === previous[index])) return;
      original();
      previous = geometry();
    };
  };
  const enqueue = (doc, change) => {
    prepare(doc);
    if (pending?.doc !== doc) pending = { doc, scale: doc.currentScaleRatio, resize: false, demand: false, zoom: false };
    change(pending);
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const input = pending;
      pending = null;
      const doc = documentRenderer();
      if (!doc || doc !== input.doc) return;
      doc.cachedDOMState = doc.retrieveDOMState();
      const scroll = doc.hookedElem.parentElement;
      if (input.zoom) {
        const rect = doc.hookedElem.firstElementChild.getBoundingClientRect();
        const viewport = scroll.getBoundingClientRect();
        const oldScale = doc.currentScaleRatio;
        const x = input.x ?? (viewport.left + scroll.clientWidth / 2);
        const y = input.y ?? (viewport.top + scroll.clientHeight / 2);
        const contentX = x - rect.left;
        const contentY = y - rect.top;
        doc.currentScaleRatio = input.scale;
        const fit = Math.abs(input.scale - 1) < 1e-6;
        doc.hookedElem.classList.toggle('hide-scrollbar-x', fit);
        scroll.classList.toggle('hide-scrollbar-x', fit);
        doc.r.rescale();
        const after = doc.hookedElem.firstElementChild.getBoundingClientRect();
        const ratio = input.scale / oldScale;
        scroll.scrollBy(after.left + contentX * ratio - x, after.top + contentY * ratio - y);
      } else if (input.resize) {
        doc.r.rescale();
      }
      if (doc.partialRendering && (input.demand || input.resize || input.zoom)) {
        doc.addViewportChange();
      }
    });
  };
  const zoom = (doc, factor, reset = false, point = {}) => enqueue(doc, input => {
    input.scale = reset ? 1 : Math.min(10, Math.max(0.1, input.scale * factor));
    input.zoom = true;
    input.x = point.x;
    input.y = point.y;
  });
  window.addEventListener('resize', event => {
    const doc = documentRenderer();
    if (!doc) return;
    event.stopImmediatePropagation();
    enqueue(doc, input => { input.resize = true; });
  }, true);
  window.addEventListener('scroll', event => {
    const doc = documentRenderer();
    if (!doc?.partialRendering || event.target !== doc.hookedElem.parentElement) return;
    event.stopImmediatePropagation();
    doc.clearSvgResizeAnchor();
    enqueue(doc, input => { input.demand = true; });
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
    return true;
  };
  window.addEventListener('tiptoptyp-preview-zoom', event => command(event.detail));
  window.addEventListener('keydown', event => {
    if (!(event.metaKey || event.ctrlKey) ||
        event.target?.closest?.('input, textarea, [contenteditable]')) return;
    const action = event.key === '=' || event.key === '+' ? 'in'
      : event.key === '-' ? 'out' : event.key === '0' ? 'reset' : null;
    if (!action || !command(action)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
})();
