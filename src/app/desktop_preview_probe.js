// Read-only inspection of the actual Tinymist DOM; never changes the page.
(() => {
  const page = document.querySelector('#typst-container g[data-page-width]');
  if (!page) return {ready: false};
  const ancestors = [];
  for (let node = page; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (style.filter !== 'none') ancestors.push(style.filter);
  }
  const channels = [...document.querySelectorAll('#tiptoptyp-palette feComponentTransfer > *')];
  return {
    ready: page.getBoundingClientRect().width > 0,
    filters: ancestors,
    background: getComputedStyle(document.body).backgroundColor,
    slopes: channels.map(c => Number(c.getAttribute('slope'))),
    intercepts: channels.map(c => Number(c.getAttribute('intercept'))),
  };
})();
