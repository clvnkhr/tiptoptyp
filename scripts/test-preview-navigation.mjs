import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/preview_navigation.js', import.meta.url), 'utf8');
function fixture() {
  const handlers = new Map();
  const frames = [];
  let rescales = 0;
  let demands = 0;
  const classes = { toggle() {} };
  const scroll = {
    clientWidth: 800, clientHeight: 600, classList: classes,
    x: 0, y: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0 }),
    scrollBy(x, y) { this.x += x; this.y += y; },
  };
  const svg = {
    tagName: 'svg', children: [],
    getAttribute: () => '100',
    getBoundingClientRect: () => ({ left: -scroll.x, top: -scroll.y }),
  };
  const doc = {
    moduleInitialized: true, renderMode: 'svg', previewMode: 0,
    partialRendering: false, currentScaleRatio: 1,
    windowElem: { contains: () => true },
    hookedElem: { firstElementChild: svg, parentElement: scroll, classList: classes, style: {} },
    cachedDOMState: { width: 800, height: 600 },
    addViewportChange() { demands++; },
    clearSvgResizeAnchor() {},
    retrieveDOMState: () => ({ width: 800, height: 600 }),
    r: { rescale() { rescales++; }, rerender() { throw Error('viewport rerender'); } },
  };
  const container = { documents: [{ impl: doc }] };
  runInNewContext(source, {
    document: { getElementById: () => container, readyState: 'loading', addEventListener() {} },
    window: { addEventListener: (name, handler) => handlers.set(name, handler) },
    requestAnimationFrame: callback => { frames.push(callback); return frames.length; },
  });
  return {
    doc, scroll, frames, container, get rescales() { return rescales; },
    get demands() { return demands; },
    flush() { frames.splice(0).forEach(f => f()); },
    event(name, data = {}) {
      const event = { deltaMode: 0, prevented: false, stopped: false,
        preventDefault() { this.prevented = true; },
        stopImmediatePropagation() { this.stopped = true; }, ...data };
      handlers.get(name)(event);
      return event;
    },
  };
}

test('resize bursts rescale once per frame, never rerender or keep idle frames alive', () => {
  const f = fixture();
  for (let i = 0; i < 1000; i++) assert.ok(f.event('resize').stopped);
  assert.equal(f.frames.length, 1);
  f.flush();
  assert.equal(f.rescales, 1);
  assert.equal(f.frames.length, 0);
});
test('small wheel deltas accumulate continuously and preserve the pointer anchor', () => {
  const f = fixture();
  for (let i = 0; i < 5; i++) {
    assert.ok(f.event('wheel', { ctrlKey: true, deltaY: -1, clientX: 200, clientY: 150 }).prevented);
  }
  f.flush();
  assert.ok(Math.abs(f.doc.currentScaleRatio - Math.exp(0.01)) < 1e-12);
  assert.ok(Math.abs((f.scroll.y + 150) / f.doc.currentScaleRatio - 150) < 1e-10);
  assert.equal(f.rescales, 1);
  assert.equal(f.event('wheel', { deltaY: 40 }).prevented, false);
});
test('keyboard zoom clamps, resets, and leaves text input alone', () => {
  const f = fixture();
  for (let i = 0; i < 100; i++) f.event('keydown', { metaKey: true, key: '+' });
  f.flush();
  assert.equal(f.doc.currentScaleRatio, 10);
  f.event('keydown', { metaKey: true, key: '0' });
  f.flush();
  assert.equal(f.doc.currentScaleRatio, 1);
  assert.equal(f.event('keydown', { metaKey: true, key: '-', target: { closest: () => ({}) } }).prevented, false);
});
test('unsupported renderers and disconnected documents retain normal event handling', () => {
  const f = fixture();
  f.doc.renderMode = 'canvas';
  assert.equal(f.event('resize').stopped, false);
  assert.equal(f.frames.length, 0);
  f.doc.renderMode = 'svg';
  f.event('resize');
  f.container.documents = [];
  f.flush();
  assert.equal(f.rescales, 0);
});

test('partial rendering refreshes scroll demand once per frame without idle work', () => {
  const f = fixture();
  f.doc.partialRendering = true;
  for (let i = 0; i < 100; i++) f.event('scroll', { target: f.scroll });
  f.flush();
  assert.equal(f.demands, 1);
  assert.equal(f.rescales, 0);
  assert.equal(f.frames.length, 0);
});

test('native commands, option-key shortcuts and wheel inputs compose in order', () => {
  const f = fixture();
  f.event('tiptoptyp-preview-zoom', { detail: 'in' });
  f.event('wheel', { ctrlKey: true, deltaY: -1, clientX: 200, clientY: 150 });
  f.event('keydown', { metaKey: true, altKey: true, key: '+' });
  f.flush();
  assert.ok(Math.abs(f.doc.currentScaleRatio - 1.21 * Math.exp(0.002)) < 1e-12);
  f.event('tiptoptyp-preview-zoom', { detail: 'reset' });
  f.flush();
  assert.equal(f.doc.currentScaleRatio, 1);
});

test('redundant layout is skipped but replaced SVG invalidates geometry', () => {
  const f = fixture();
  f.event('resize');
  f.flush();
  f.doc.r.rescale();
  assert.equal(f.rescales, 1);
  f.doc.hookedElem.firstElementChild = { ...f.doc.hookedElem.firstElementChild };
  f.doc.r.rescale();
  assert.equal(f.rescales, 2);
});
