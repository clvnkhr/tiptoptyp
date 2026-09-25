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
      updateControls();
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
    updateControls();
  }, true);
  window.addEventListener('scroll', event => {
    if (event.target === scrollElement()) recordNavigationScroll();
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
    const doc = documentRenderer();
    doc?.clearSvgResizeAnchor();
    scrollElement()?.scrollTo({left:value.left,top:value.top,behavior:'instant'});
  };
  const prepareHistory = () => {
    const current = position();
    pendingNavigation = current ? { ...current, at: performance.now() } : null;
  };
  const pages = () => {
    const svg = documentRenderer()?.hookedElem.firstElementChild;
    return svg ? [...svg.children].filter(node => node.tagName.toLowerCase() === 'g') : [];
  };
  const currentPage = () => {
    const scroll = scrollElement();
    if (!scroll) return 0;
    const all = pages();
    const top = scroll.getBoundingClientRect().top;
    return Math.max(0, all.findLastIndex(page => page.getBoundingClientRect().top <= top + 16));
  };
  const gotoPage = page => {
    const scroll = scrollElement();
    const target = pages()[page];
    if (!scroll || !target) return;
    const before = position();
    const top = target.getBoundingClientRect().top - scroll.getBoundingClientRect().top + scroll.scrollTop;
    if (Math.abs(top - scroll.scrollTop) < 1) return;
    if (before) pushHistory(back, before);
    forward.length = 0;
    scroll.scrollTo({ top, behavior: 'instant' });
    updateControls();
  };
  const updateControls = () => {
    const state = JSON.stringify({ type: 'preview-state', page: currentPage(), count: pages().length,
      zoom: documentRenderer()?.currentScaleRatio ?? 1, back: back.length > 0, forward: forward.length > 0 });
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
        doc.windowElem.handleTypstLocation?.(doc.hookedElem.firstElementChild,value.page+1,value.x,value.y);
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
    [...filter.firstElementChild.children].forEach((channel,index) => {
      channel.setAttribute('slope',String((bg[index]-fg[index])/255));
      channel.setAttribute('intercept',String(fg[index]/255));
    });
    const container = document.getElementById('typst-container');
    if (container) container.style.filter = 'url(#tiptoptyp-palette)';
    document.body.style.background = `rgb(${bg.join(',')})`;
  };
  window.tiptoptypSetPalette = (bg,fg) => { palette = [bg,fg]; applyPalette(); };
  const mount = () => {
    const style = document.createElement('style');
    style.textContent = '#typst-container-top { display: none !important; }';
    document.head.append(style);
    applyPalette();
    updateControls();
    const container = document.getElementById('typst-container');
    if (container) new MutationObserver(updateControls).observe(container,{ childList:true, subtree:true });
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
  const recordNavigationScroll = () => {
    const now = position();
    if (pendingNavigation && performance.now() - pendingNavigation.at < 2000 && now &&
        (Math.abs(now.top - pendingNavigation.top) > 1 || Math.abs(now.left - pendingNavigation.left) > 1)) {
      pushHistory(back, { top: pendingNavigation.top, left: pendingNavigation.left });
      forward.length = 0;
      pendingNavigation = null;
    }
    updateControls();
  };
  window.addEventListener('tiptoptyp-preview-zoom', event => command(event.detail));
  window.addEventListener('keydown', event => {
    if (event.target?.closest?.('input, textarea, [contenteditable]')) { event.stopImmediatePropagation(); return; }
    if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key === 't') {
      event.preventDefault(); event.stopImmediatePropagation();
      palette = [palette[1],palette[0]]; applyPalette();
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
