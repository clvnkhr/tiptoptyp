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

  // The native WebView is above egui's framebuffer, so its controls must be
  // drawn inside the WebView rather than in a root-window egui Area.
  let controls;
  let expanded = false;
  let outline = [];
  let indexedOutline = [];
  let searchText = '';
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
  const restore = value => scrollElement()?.scrollTo(value.left, value.top);
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
  const button = (label, title, action) => {
    const element = document.createElement('button');
    element.type = 'button';
    element.textContent = label;
    element.title = title;
    element.addEventListener('click', action);
    return element;
  };
  const updateControls = () => {
    if (!controls) return;
    const all = pages();
    controls.querySelector('[data-page]').value = String(currentPage() + 1);
    controls.querySelector('[data-count]').textContent = `/ ${all.length}`;
    controls.querySelector('[data-zoom]').textContent = `${Math.round((documentRenderer()?.currentScaleRatio ?? 1) * 100)}%`;
    controls.querySelector('[data-back]').disabled = back.length === 0;
    controls.querySelector('[data-forward]').disabled = forward.length === 0;
    controls.classList.toggle('expanded', expanded);
    controls.querySelector('[data-open]').hidden = expanded;
    controls.querySelector('[data-full]').hidden = !expanded;
    controls.style.left = `${Math.max(0, Math.min(innerWidth - controls.offsetWidth, controls.offsetLeft))}px`;
    controls.style.top = `${Math.max(0, Math.min(innerHeight - controls.offsetHeight, controls.offsetTop))}px`;
  };
  const mountControls = () => {
    if (controls || !document.body) return;
    const style = document.createElement('style');
    style.textContent = `
      #typst-container-top { display: none !important; }
      #tiptoptyp-preview-controls { position: fixed; left: 12px; top: 12px; z-index: 2147483647;
        font: 13px -apple-system, BlinkMacSystemFont, sans-serif; color: var(--vscode-menu-foreground, #26303b);
        background: var(--vscode-menu-background, #fff); border: 1px solid var(--vscode-menu-border, #b7c3cc);
        border-radius: 8px; box-shadow: 0 3px 14px #0003; padding: 4px; max-width: calc(100vw - 24px); }
      #tiptoptyp-preview-controls button { color: inherit; background: transparent; border: 0;
        border-radius: 4px; padding: 5px 7px; cursor: pointer; font: inherit; }
      #tiptoptyp-preview-controls button:hover { background: #8883; }
      #tiptoptyp-preview-controls button:disabled { opacity: .4; cursor: default; }
      #tiptoptyp-preview-controls input { font: inherit; color: inherit; background: transparent;
        border: 1px solid #8886; border-radius: 4px; padding: 3px 5px; }
      #tiptoptyp-preview-controls [data-full] { width: max-content; max-width: 100%; }
      #tiptoptyp-preview-controls .row { display: flex; flex-wrap: wrap; align-items: center; gap: 2px; }
      #tiptoptyp-preview-controls .handle { cursor: move; user-select: none; touch-action: none; }
      #tiptoptyp-preview-controls .outline { max-height: 220px; overflow: auto; display: none; }
      #tiptoptyp-preview-controls .outline.open { display: block; }
      #tiptoptyp-preview-controls .outline button { display: block; text-align: left; width: 100%; }
    `;
    document.head.append(style);
    controls = document.createElement('div');
    controls.id = 'tiptoptyp-preview-controls';
    const opener = button('☰', 'Preview controls', () => { if (!controls.dataset.dragged) { expanded = true; updateControls(); } });
    opener.dataset.open = '';
    opener.className = 'handle';
    controls.append(opener);
    const full = document.createElement('div');
    full.dataset.full = '';
    const header = document.createElement('div');
    header.className = 'row handle';
    header.append(button('−', 'Minimize preview controls', () => { expanded = false; updateControls(); }));
    const title = document.createElement('span');
    title.textContent = 'Preview';
    header.append(title);
    const outlineBox = document.createElement('div'); outlineBox.className = 'outline';
    header.append(button('Outline', 'Document outline', () => {
      outlineBox.classList.toggle('open');
      renderOutline();
    }));
    full.append(header);
    const row = document.createElement('div');
    row.className = 'row';
    const backButton = button('←', 'Back', () => {
      const target = back.pop();
      const here = position();
      if (target && here) { pendingNavigation = null; pushHistory(forward, here); restore(target); updateControls(); }
    });
    backButton.dataset.back = '';
    row.append(backButton);
    const forwardButton = button('→', 'Forward', () => {
      const target = forward.pop();
      const here = position();
      if (target && here) { pendingNavigation = null; pushHistory(back, here); restore(target); updateControls(); }
    });
    forwardButton.dataset.forward = '';
    row.append(forwardButton);
    row.append(button('‹', 'Previous page', () => gotoPage(Math.max(0, currentPage() - 1))));
    const pageInput = document.createElement('input');
    pageInput.type = 'number'; pageInput.min = '1'; pageInput.style.width = '3.5em'; pageInput.dataset.page = '';
    pageInput.addEventListener('change', () => gotoPage(Math.max(0, Number(pageInput.value) - 1)));
    row.append(pageInput);
    const count = document.createElement('span'); count.dataset.count = ''; row.append(count);
    row.append(button('›', 'Next page', () => gotoPage(Math.min(pages().length - 1, currentPage() + 1))));
    row.append(button('−', 'Zoom out', () => command('out')));
    row.append(button('+', 'Zoom in', () => command('in')));
    row.append(button('↔', 'Fit page width', () => command('reset')));
    const zoomLabel = document.createElement('span'); zoomLabel.dataset.zoom = ''; row.append(zoomLabel);
    full.append(row);
    const findRow = document.createElement('div'); findRow.className = 'row';
    const findInput = document.createElement('input'); findInput.placeholder = 'Find source text';
    findInput.title = 'Find Typst source text and reveal its position in the preview';
    findInput.setAttribute('aria-label', 'Find in preview');
    findInput.addEventListener('input', () => { searchText = findInput.value; });
    findInput.addEventListener('keydown', event => { if (event.key === 'Enter') findNext(); });
    findRow.append(findInput, button('Find next', 'Find next matching page', findNext));
    full.append(findRow, outlineBox);
    controls.append(full);
    document.body.append(controls);
    const startDrag = event => {
      if (event.target.closest('input') || (event.target.closest('button') && event.target !== opener)) return;
      const startX = event.clientX, startY = event.clientY;
      const left = controls.offsetLeft, top = controls.offsetTop;
      controls.dataset.dragged = '';
      const move = point => {
        if (Math.abs(point.clientX - startX) + Math.abs(point.clientY - startY) > 3) controls.dataset.dragged = 'yes';
        controls.style.left = `${Math.max(0, Math.min(innerWidth - controls.offsetWidth, left + point.clientX - startX))}px`;
        controls.style.top = `${Math.max(0, Math.min(innerHeight - controls.offsetHeight, top + point.clientY - startY))}px`;
      };
      const end = () => {
        window.removeEventListener('pointermove', move);
        window.removeEventListener('pointerup', end);
        setTimeout(() => { delete controls.dataset.dragged; }, 0);
      };
      window.addEventListener('pointermove', move);
      window.addEventListener('pointerup', end, { once: true });
    };
    header.addEventListener('pointerdown', startDrag);
    opener.addEventListener('pointerdown', startDrag);
    updateControls();
  };
  const findNext = () => {
    if (!searchText) return;
    if (window.ipc?.postMessage) {
      prepareHistory();
      window.ipc.postMessage(JSON.stringify({ type: 'find', query: searchText }));
      return;
    }
    if (typeof window.find === 'function' && window.find(searchText, false, false, true)) return;
    const all = pages();
    const from = currentPage();
    for (let offset = 1; offset <= all.length; offset++) {
      const page = (from + offset) % all.length;
      if (all[page].textContent.toLocaleLowerCase().includes(searchText.toLocaleLowerCase())) {
        gotoPage(page);
        return;
      }
    }
  };
  const renderOutline = () => {
    const box = controls?.querySelector('.outline');
    if (!box || !box.classList.contains('open')) return;
    box.replaceChildren();
    const visit = (items, depth = 0) => {
      for (const item of items) {
        const label = item.title ?? item.label ?? item.body ?? '';
        const page = item.page ?? item.pageNo ?? item.position?.page;
        if (label && (Number.isFinite(Number(page)) || item.path)) {
          const entry = button(String(label), 'Go to heading', () => {
            if (item.path && window.ipc?.postMessage) {
              prepareHistory();
              window.ipc.postMessage(JSON.stringify({ type: 'outline', path: item.path, line: item.line }));
            } else if (Number.isFinite(Number(page))) gotoPage(Number(page));
          });
          entry.style.paddingLeft = `${8 + (item.level ? item.level - 1 : depth) * 12}px`;
          box.append(entry);
        }
        if (Array.isArray(item.children)) visit(item.children, depth + 1);
      }
    };
    visit(indexedOutline.length ? indexedOutline : outline);
    if (!box.childElementCount) box.textContent = 'No outline available';
  };
  window.tiptoptypSetOutline = entries => {
    indexedOutline = Array.isArray(entries) ? entries : [];
    renderOutline();
  };
  window.addEventListener('message', event => {
    if (event.data?.type === 'outline') {
      outline = Array.isArray(event.data.outline) ? event.data.outline : [];
      renderOutline();
    }
  }, true);
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
  document.addEventListener('DOMContentLoaded', mountControls);
  if (document.readyState !== 'loading') mountControls();
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
    if (!(event.metaKey || event.ctrlKey) ||
        event.target?.closest?.('input, textarea, [contenteditable]')) return;
    const action = event.key === '=' || event.key === '+' ? 'in'
      : event.key === '-' ? 'out' : event.key === '0' ? 'reset' : null;
    if (!action || !command(action)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
})();
