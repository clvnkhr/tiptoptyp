// Regression checks for read-only observations of buffered Tinymist pages.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
const source = readFileSync(new URL('../src/app/desktop_preview_probe.js', import.meta.url), 'utf8');
function inspect({ firstDummy = true, visibleDummy = false, initialized = true, loaded = true, reveal = null } = {}) {
  const groups = [-700, 0, 800].map((top, index) => {
    const dummy = index === 0 ? firstDummy : index === 1 ? visibleDummy : true;
    return {
      matches: () => true,
      parentElement: null,
      getScreenCTM: () => ({ e: 0, f: top, a: 1, d: 1 }),
      getBoundingClientRect: () => ({ width: dummy ? 0 : 500 }),
      getAttribute: key => ({ 'data-page-width': '500', 'data-page-height': '680', 'data-dummy': dummy ? '1' : '0' })[key],
      querySelector: () => dummy ? null : {},
      querySelectorAll: () => dummy ? [] : [{ getBoundingClientRect: () => ({
        left: 20, top: top + 25, right: 28, bottom: top + 35, width: 8, height: 10,
      }) }],
    };
  });
  const svg = { children: groups, querySelectorAll: () => [],
    getBoundingClientRect: () => ({left:0,top:-700,width:500,height:2180}) };
  const doc = { moduleInitialized: initialized, partialRendering: true, currentScaleRatio: 1.21, hookedElem: {
    firstElementChild: svg,
    parentElement: { scrollLeft:17, scrollTop:700, getBoundingClientRect: () => ({ left: 0, top: 0, right: 900, bottom: 700 }) },
  } };
  const container = { documents: [{ impl: doc }] };
  return vm.runInNewContext(source, { window: {
    innerWidth: 900, innerHeight: 700,
    tiptoptypPreviewRevealState: () => reveal,
  },
    document: {
      readyState: loaded ? 'complete' : 'loading', body: {},
      getElementById: () => container,
      querySelector: selector => selector === '#typst-container' ? container : loaded ? groups[0] : null,
      querySelectorAll: () => [],
    },
    getComputedStyle: () => ({ filter: 'none', backgroundColor: 'rgb(255, 255, 255)' }),
  });
}
test('offscreen first-page placeholders do not mark visible ink unready', () => {
  const value = inspect();
  assert.equal(value.ready, true);
  assert.equal(value.partial, true);
  assert.equal(value.populated, 1);
  assert.equal(value.visible[0].index, 1);
  assert.equal(value.visible[0].ink, true);
});
test('visible placeholder pages are never reported ready', () => {
  const value = inspect({ firstDummy: false, visibleDummy: true });
  assert.equal(value.ready, false);
  assert.equal(value.visible[0].dummy, true);
});
test('missing SVG or uninitialized modules cannot report readiness', () => {
  assert.equal(inspect({ loaded: false }).ready, false);
  assert.equal(inspect({ initialized: false }).ready, false);
});

test('zoom observation reports actual visible-paper scale separately from requested zoom',()=>{
  const value=inspect();
  assert.equal(value.zoom,1.21);
  assert.equal(value.paper_scale,1);
});

test('sharp handoff observations expose actual scroll and SVG bounds',()=>{
  const value=inspect();
  assert.deepEqual(Array.from(value.scroll),[17,700]);
  assert.deepEqual(Array.from(value.paper_bounds),[0,-700,500,2180]);
  assert.deepEqual(Array.from(value.page_bounds[1]),[0,0,500,680]);
});

test('source ink reports only actual visible glyph bounds and native viewport size', () => {
  const value = inspect();
  assert.deepEqual(Array.from(value.window_size), [900, 700]);
  assert.equal(value.source_ink.length, 1);
  assert.equal(value.source_ink[0].page, 1);
  assert.deepEqual(Array.from(value.source_ink[0].bounds), [20, 25, 8, 10]);
  assert.equal(inspect({ visibleDummy: true }).source_ink.length, 0);
});

test('reveal observation reads the real frontend state without starting an animation', () => {
  assert.equal(inspect().reveal, null);
  const reveal = { active: true, pending: false, page: 1, center: [24, 30],
    viewport: [0, 0, 900, 700], elapsed_ms: 300 };
  assert.deepEqual(inspect({ reveal }).reveal, reveal);
  const retired = { active: false, pending: false, page: null, center: null,
    viewport: [0, 0, 900, 700], elapsed_ms: 0 };
  assert.deepEqual(inspect({ reveal: retired }).reveal, retired);
});
