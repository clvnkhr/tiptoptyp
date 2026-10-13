import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(process.env.TIPTOPTYP_BUFFER_SCRIPT || new URL('../src/preview_snapshot_buffer.js', import.meta.url), 'utf8');
function fixture() {
  const reads = new Map(), clones = [];
  const element = (tag, html, { classes = [], page = null, rect = {}, children = [] } = {}) => ({
    tagName: tag, outerHTML: html, children,
    classList: { contains: value => classes.includes(value) },
    getAttribute: name => name === 'data-page-number' ? page : null,
    getBoundingClientRect: () => rect,
    contains: () => false,
    cloneNode(deep) { clones.push(deep); assert.equal(deep, false, 'never clone a dense SVG subtree');
      return { style: {}, get outerHTML() { return `<svg style="${this.style.cssText}"></svg>`; } }; },
    querySelectorAll() { throw Error('never walk a dense SVG subtree'); },
  });
  const paper = (page, top) => element('rect', `<rect class="typst-page-inner" data-page-number="${page}"/>`,
    { classes: ['typst-page-inner'], page: String(page), rect: { left: 0, right: 600, top, bottom: top + 900 } });
  const ink = (page, html) => element('g', `<g class="typst-page" data-page-number="${page}">${html}</g>`,
    { classes: ['typst-page'], page: String(page) });
  const children = [element('defs', '<defs><path id="unused"/></defs>'), paper(0, 0),
    ink(0, '<use href="#glyph"/><use xlink:href="#glyph"/>'), paper(1, 910), ink(1, '<path/>'),
    paper(2, 1820), ink(2, '<use href="#far"/>')];
  const svg = element('svg', '<svg/>', { children, rect: { left: 10, top: -20, width: 600, height: 2730 } });
  const ids = new Map([
    ['glyph', element('path', '<path id="glyph" fill="url(#paint)"/>')],
    ['paint', element('linearGradient', '<linearGradient id="paint" href="#glyph"/>')],
    ['tiptoptyp-palette', element('filter', '<filter id="tiptoptyp-palette"/>')],
  ]);
  const document = {
    documentElement: {}, styleSheets: [{ cssRules: [{ cssText: 'svg { display:block; }' }] }],
    getElementById(id) { reads.set(id, (reads.get(id) || 0) + 1); return ids.get(id); },
    createElement(tag) { assert.equal(tag, 'textarea');
      return { set innerHTML(value) { this.value = value.replace(/&(?:quot|amp|lt|gt|apos);/g,
        entity => ({ '&quot;': '"', '&amp;': '&', '&lt;': '<', '&gt;': '>', '&apos;': "'" })[entity]); } }; },
  };
  const window = {};
  runInNewContext(source, { document, window, getComputedStyle: () => ({ getPropertyValue: () => 'none' }) });
  const frame = { svg, width: 800, height: 600, background: [24, 31, 42] };
  return { frame, svg, ids, reads, clones, children, document, element, ink, run: () => window.tiptoptypSnapshotBuffer(frame) };
}

test('bounded buffer serializes each dependency once without copying or walking dense trees', () => {
  const f = fixture(), buffer = f.run();
  assert.deepEqual(f.clones, [false]);
  assert.deepEqual([buffer.x, buffer.y, buffer.width, buffer.height], [-400, -600, 1600, 1800]);
  assert.equal(f.reads.get('glyph'), 1); assert.equal(f.reads.get('paint'), 1);
  assert.equal(f.reads.get('tiptoptyp-palette'), 1);
  assert.equal(f.reads.has('far'), false);
  assert.match(buffer.html, /left:410px;top:580px;/);
  assert.match(buffer.html, /background:rgb\(24,31,42\)!important/);
  assert.match(buffer.html, /script-src 'none'/);
  assert.match(buffer.html, /<defs>.*id="glyph".*<\/defs>/);
  assert.equal(buffer.html.includes('id="unused"'), false);
  assert.equal(buffer.html.includes('data-page-number="2"'), false);
});
test('href namespaces, entity-encoded ids and quoted CSS url dependencies survive serialization', () => {
  const f = fixture();
  f.children[2] = f.ink(0, '<use xlink:href="#a&amp;b"/><path style="clip-path:url(&quot;#clip&quot;);fill:url(\'#paint\')"/>');
  f.ids.set('a&b', f.element('path', '<path id="a&amp;b"/>'));
  f.ids.set('clip', f.element('clipPath', '<clipPath id="clip"/>'));
  const html = f.run().html;
  assert.equal(f.reads.get('a&b'), 1); assert.equal(f.reads.get('clip'), 1);
  assert.match(html, /id="a&amp;b"/); assert.match(html, /id="clip"/);
});
test('quoted attribute contents cannot invent a second href attribute', () => {
  const f = fixture();
  f.children[2] = f.ink(0, '<text data-note=" href=&quot;#phantom&quot;"> href="#text"</text><!-- > <use href="#comment"/> --><![CDATA[<use href="#cdata"/>]]><path data-note="a > b" fill="url(#paint)"/>');
  f.run(); for (const id of ['phantom','text','comment','cdata']) assert.equal(f.reads.has(id), false);
  assert.equal(f.reads.get('paint'), 1);
});
test('missing references and cyclic definitions are bounded; live SVG ancestors cannot be copied', () => {
  const f = fixture();
  f.children[2] = f.ink(0, '<use href="#missing"/><use href="#missing"/><use href="#root"/><use href="#parent"/>');
  f.ids.set('root', f.svg); f.ids.set('parent', { contains: node => node === f.svg });
  f.run(); assert.equal(f.reads.get('missing'), 1); assert.equal(f.reads.get('root'), 1); assert.equal(f.reads.get('parent'), 1);
});
test('each idle refresh reads current definitions and palette without a stale persistent cache', () => {
  const f = fixture(); assert.match(f.run().html, /id="glyph" fill=/);
  f.ids.set('glyph', f.element('path', '<path id="glyph" d="M1 2"/>'));
  f.frame.background = [255, 255, 255];
  const html = f.run().html;
  assert.match(html, /id="glyph" d="M1 2"/); assert.equal(html.includes('id="glyph" fill='), false);
  assert.match(html, /background:rgb\(255,255,255\)!important/);
});
test('invalid geometry and pathological serialized documents fail before native IPC', () => {
  const f = fixture(); f.svg.getBoundingClientRect = () => ({ left: 0, top: 0, width: 0, height: 1 });
  assert.equal(f.run(), null); assert.equal(f.clones.length, 0);
  f.svg.getBoundingClientRect = () => ({ left: 0, top: 0, width: 600, height: 900 });
  f.children[2] = f.ink(0, 'x'.repeat(20_000_001)); assert.equal(f.run(), null);
});
