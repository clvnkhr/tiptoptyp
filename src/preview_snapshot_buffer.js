// Idle-only, bounded paper window. This inert SVG copy is rasterized in a
// separate native WebKit view, never by moving/resizing the live document.
(() => {
  const plan = (width, height) => ({ x: -width / 2, y: -height, width: width * 2, height: height * 3 });
  const overlaps = (a, b) => a.right > b.x && a.left < b.x + b.width && a.bottom > b.y && a.top < b.y + b.height;
  // Resolve the actual opaque viewport backdrop, including transparent parent
  // layers. This bounded ancestor walk runs only during idle capture admission.
  window.tiptoptypSnapshotBackdrop = frame => {
    let alpha = 0, color = [0, 0, 0], node = frame.svg?.parentElement ?? document.body;
    for (let depth = 0; node && depth < 32 && alpha < 1; depth++, node = node.parentElement) {
      const css = getComputedStyle(node).backgroundColor;
      const values = css?.match(/^rgba?\(([^)]+)\)$/)?.[1].trim().split(/[\s,/]+/).map(Number);
      if (!values || values.length < 3 || !values.every(Number.isFinite)) continue;
      const opacity = Math.max(0, Math.min(1, values[3] ?? 1)), weight = (1 - alpha) * opacity;
      color = color.map((value, index) => value + values[index] * weight); alpha += weight;
    }
    return [...color.map((value, index) => value + (frame.background?.[index] ?? 255) * (1 - alpha)), 1];
  };
  window.tiptoptypSnapshotBuffer = frame => {
    const svg = frame.svg, rect = svg.getBoundingClientRect();
    if (![rect.left, rect.top, rect.width, rect.height].every(Number.isFinite) || rect.width <= 0 || rect.height <= 0) return null;
    const area = plan(frame.width, frame.height);
    const copy = svg.cloneNode(false);
    // The SVG is transparent between papers; its live parent supplies the
    // gutter. Keep that backdrop in the inert copy, including at page edges.
    const paperBackground = window.tiptoptypSnapshotBackdrop(frame);
    const gutter = `rgb(${paperBackground.slice(0,3).join(',')})`;
    copy.style.cssText = `${copy.style.cssText || ''};position:absolute;left:${rect.left-area.x}px;top:${rect.top-area.y}px;width:${rect.width}px;height:${rect.height}px;transform:none;background:${gutter};overflow:hidden;`;
    const queue = [], included = new Set(), ink = [], definitions = [];
    const enqueue = id => { if (!included.has(id)) { included.add(id); queue.push(id); } };
    let decoder;
    const decode = value => {
      if (!value.includes('&')) return value;
      decoder ??= document.createElement('textarea');
      // outerHTML quotes attribute values. Decode entities without parsing or
      // attaching SVG ink; protect literal '<' from textarea end-tag parsing.
      decoder.innerHTML = value.replaceAll('<', '&lt;');
      return decoder.value;
    };
    const refs = markup => {
      // Native serialization is far cheaper than cloning dense math pages and
      // walking every element/attribute through JS. Tinymist's serialized SVG
      // attributes are double quoted; consume whole values before inspecting
      // href/url so quoted CSS and embedded markup cannot invent attributes.
      // Skip comments/CDATA and the text following a tag. Attribute tokens
      // consume complete quoted values first, including any '>' inside them.
      for (const attr of markup.matchAll(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|>[^<]*|\s([^\s"'<>/=]+)="([^"]*)"/g)) {
        if (!attr[1]) continue;
        const href = attr[1] === 'href' || attr[1].endsWith(':href'), urls = attr[2].includes('url(');
        if (!href && !urls) continue;
        const value = decode(attr[2]);
        if (href && value.startsWith('#')) enqueue(value.slice(1));
        if (urls) {
          for (const match of value.matchAll(/url\(\s*['"]?#([^)'"\s]+)['"]?\s*\)/g)) enqueue(match[1]);
        }
      }
    };
    const papers=new Set([...svg.children].filter(n=>n.classList.contains('typst-page-inner') && overlaps(n.getBoundingClientRect(),area)).map(n=>n.getAttribute('data-page-number')));
    for (const child of svg.children) {
      if (child.tagName.toLowerCase() === 'defs') continue;
      if ((child.classList.contains('typst-page') || child.classList.contains('typst-page-inner')) && !papers.has(child.getAttribute('data-page-number'))) continue;
      const markup = child.outerHTML; ink.push(markup); refs(markup);
    }
    // Palette definitions live outside Tinymist's SVG. Include the same transfer
    // once; resolve glyph/gradient dependencies from this document by ID.
    enqueue('tiptoptyp-palette');
    while (queue.length) {
      const source = document.getElementById(queue.pop());
      if (!source || source === svg || source.contains(svg)) continue;
      const markup = source.outerHTML; definitions.push(markup); refs(markup);
    }
    // Only the SVG shell is cloned. No second dense SVG tree or parse is needed
    // in the live renderer; the native capture view parses this inert document.
    const shell = copy.outerHTML, close = shell.lastIndexOf('</');
    if (close < 0) return null;
    const markup = shell.slice(0, close) + '<defs>' + definitions.join('') + '</defs>' + ink.join('') + shell.slice(close);
    const styles = [...document.styleSheets].flatMap(sheet => {
      try { return [...sheet.cssRules].map(rule => rule.cssText); } catch (_) { return []; }
    }).join('\n');
    const variables = ['--tiptoptyp-paper','--tiptoptyp-ink','--tiptoptyp-preview-filter'].map(name => `${name}:${getComputedStyle(document.documentElement).getPropertyValue(name)};`).join('');
    const background = gutter;
    const html = `<html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'"><style>${styles} html,body{margin:0!important;overflow:hidden!important;background:${background}!important;} :root{${variables}}</style></head><body><div id="typst-container">${markup}</div></body></html>`;
    // Pathological SVGs fall back to a viewport capture, not an unbounded IPC.
    return html.length <= 20_000_000 ? { ...area, html, paperBackground } : null;
  };
})();
