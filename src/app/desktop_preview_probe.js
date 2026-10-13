// Read-only inspection of the actual Tinymist DOM; never changes the page.
(() => {
  const loaded = document.readyState === 'complete' && !!document.querySelector('#typst-container');
  const page = document.querySelector('#typst-container g[data-page-width]');
  if (!page) return {loaded, ready: false};
  const doc = document.getElementById('typst-container')?.documents?.[0]?.impl;
  const svg = doc?.hookedElem?.firstElementChild;
  const viewport = doc?.hookedElem?.parentElement?.getBoundingClientRect();
  const groups = svg ? [...svg.children].filter(node => node.matches('g.typst-page')) : [];
  const page_bounds = [];
  const visible = viewport ? groups.flatMap((group, index) => {
    const matrix = group.getScreenCTM();
    if (!matrix) { page_bounds[index] = null; return []; }
    const width = Number(group.getAttribute('data-page-width')) * matrix.a;
    const height = Number(group.getAttribute('data-page-height')) * matrix.d;
    page_bounds[index] = [matrix.e, matrix.f, width, height];
    if (Math.min(matrix.e + width, viewport.right) - Math.max(matrix.e, viewport.left) <= 8 ||
        Math.min(matrix.f + height, viewport.bottom) - Math.max(matrix.f, viewport.top) <= 8) return [];
    return [{ index, dummy: group.getAttribute('data-dummy') === '1',
      ink: group.querySelector('use,path,text,rect:not(.typst-page-inner)') !== null }];
  }) : [];
  const ancestors = [];
  for (let node = page; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (style.filter !== 'none') ancestors.push(style.filter);
  }
  const channels = [...document.querySelectorAll('#tiptoptyp-palette feComponentTransfer > *')];
  return {
    loaded,
    zoom_snapshot: window.tiptoptypZoomSnapshot?.inspect() ?? null,
    partial: doc?.partialRendering ?? false,
    zoom: doc?.currentScaleRatio ?? 1,
    scroll: doc?.hookedElem?.parentElement ? [doc.hookedElem.parentElement.scrollLeft, doc.hookedElem.parentElement.scrollTop] : null,
    paper_bounds: svg ? (() => { const r=svg.getBoundingClientRect(); return [r.left,r.top,r.width,r.height]; })() : null,
    paper_scale: groups[visible[0]?.index]?.getScreenCTM()?.a ?? null,
    pages: groups.length,
    populated: groups.filter(group => group.getAttribute('data-dummy') !== '1').length,
    canvases: svg?.querySelectorAll('canvas').length ?? 0,
    visible,
    page_bounds,
    ready: doc?.moduleInitialized === true && visible.length > 0 &&
      visible.every(page => !page.dummy && page.ink),
    filters: ancestors,
    background: getComputedStyle(document.body).backgroundColor,
    slopes: channels.map(c => Number(c.getAttribute('slope'))),
    intercepts: channels.map(c => Number(c.getAttribute('intercept'))),
  };
})();
