// Opt-in real pinned Tinymist browser integration and bounded performance probe.
// Invoked by test-preview-e2e.cjs --large; servers/browsers are owned by this run.
// --mode full|partial|both (default both), --adapter FILE, --output FILE,
// --measure-only records contract failures rather than asserting success.
// --palette-checks adds untimed live-palette pixel/filter/clip acceptance.
// --geometry-checks adds untimed partial far-page source/pointer anchoring.
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const PAGE_COUNT = 48;
const STRESS = process.argv.includes('--stress');
const MAX_NODES = STRESS ? 320000 : 30000;
const VIEWPORT = { width: 900, height: 700 };
const sha256 = value => crypto.createHash('sha256').update(value).digest('hex');
const marker = (index, revision = 0) => index === 31 && revision === 1 ? [240, 80, 20]
  : [36 + (index * 37) % 180, 30 + (index * 73) % 180, 20 + (index * 47) % 180];
function argument(name, fallback) {
  const at = process.argv.indexOf(name);
  if (at < 0) return fallback;
  assert.ok(process.argv[at + 1] && !process.argv[at + 1].startsWith('--'), `${name} requires a value`);
  return process.argv[at + 1];
}
function coverageFailures(state, partial) {
  const failures = [];
  if (!state) return ['Renderer is unavailable'];
  if (state.pages !== PAGE_COUNT) failures.push(`Expected ${PAGE_COUNT} page skeletons, got ${state.pages}`);
  if (!state.visible.length) failures.push('No page intersects the viewport');
  for (const page of state.visible) {
    if (page.dummy || page.inkNodes === 0) failures.push(`Visible page ${page.index} has no rendered ink`);
  }
  if (partial && (state.canvases || state.canvasMixins)) failures.push('Partial SVG contains offscreen canvas fallbacks');
  if (partial && state.populated > state.visible.length + 6) {
    failures.push(`Partial rendering retained ${state.populated} populated pages for ${state.visible.length} visible pages`);
  }
  // This bound is fixture-specific and deliberately well above one window of ink.
  // A regression to all 48 densely populated pages must still fail.
  if (partial && state.nodes > MAX_NODES) failures.push(`Partial SVG DOM exceeds ${MAX_NODES} nodes: ${state.nodes}`);
  return failures;
}
function idleChanges(before, after) {
  return ['rescale', 'rerender', 'viewport'].filter(key => before[key] !== after[key]);
}
function stableSamplePair(before, after) {
  const rendered = state => !!state && state.scale === 1 &&
    Number.isFinite(state.viewportWidth) && state.viewportWidth > 0 &&
    Number.isFinite(state.viewportHeight) && state.viewportHeight > 0 &&
    state.visible.length === 1 && state.visible[0].index === 18 &&
    !state.visible[0].dummy && state.visible[0].inkNodes > 0;
  return rendered(before) && rendered(after) &&
    before.viewportWidth === after.viewportWidth && before.viewportHeight === after.viewportHeight &&
    Math.abs(after.scrollTop - before.scrollTop) < 1;
}
function assertSampleStart(startState) {
  assert.deepEqual(startState.viewport, VIEWPORT, 'Every sample starts at the same viewport');
  assert.equal(startState.scale, 1, 'Every sample starts fitted to width');
  assert.equal(startState.page, 18, 'Every sample starts at page 18');
  assert.deepEqual(startState.visible, [18], 'Every sample starts on actual rendered page 18');
  assert.ok(Math.abs(startState.anchorFraction - 0.15) < 0.01, `Sample reset offset drifted: ${JSON.stringify(startState)}`);
}
// One active RAF sampler, even when a canceled callback was already in flight.
function createFrameSampler(stats, request, cancel) {
  let token = 0, generation = 0;
  const stop = () => { ++generation; cancel(token); token = 0; };
  const start = () => {
    stop();
    const current = generation;
    stats.frameGaps = [];
    let previous = null;
    const frame = now => {
      if (current !== generation) return;
      if (previous !== null && stats.frameGaps.length < 1024) stats.frameGaps.push(now - previous);
      previous = now; token = request(frame);
    };
    token = request(frame);
  };
  return { start, stop };
}
// Keep pure geometry/coverage admission independently testable without a browser.
function selfTest() {
  const visible = [{ index: 0, dummy: false, inkNodes: 5 }];
  assert.deepEqual(coverageFailures({ pages: 48, visible, populated: 5, nodes: 9000 }, true), []);
  assert.equal(coverageFailures({ pages: 48, visible: [{ index: 0, dummy: true, inkNodes: 0 }], populated: 1, nodes: 10 }, true).length, 1);
  assert.equal(coverageFailures({ pages: 48, visible, populated: 48, nodes: MAX_NODES+1 }, true).length, 2);
  assert.deepEqual(coverageFailures({ pages: 48, visible, populated: 48, nodes: MAX_NODES+1 }, false), []);
  assert.equal(new Set(Array.from({ length: PAGE_COUNT }, (_, index) => marker(index).join(','))).size, PAGE_COUNT);
  assert.notDeepEqual(marker(31), marker(31, 1));
  assert.equal(coverageFailures({ pages: 48, visible, populated: 5, nodes: 9000, canvases: 1 }, true).length, 1);
  const idle = {rescale:7,rerender:3,viewport:2};
  assert.deepEqual(idleChanges(idle,{...idle}),[]);
  assert.deepEqual(idleChanges(idle,{...idle,rescale:8}),['rescale']);
  assert.deepEqual(idleChanges(idle,{...idle,rerender:4,viewport:3}),['rerender','viewport']);
  const settled={scale:1,scrollTop:100,viewportWidth:900,viewportHeight:700,visible:[{index:18,dummy:false,inkNodes:5}]};
  assert.ok(stableSamplePair(settled,{...settled}));
  for(const change of [{scale:1.1},{scrollTop:102},{viewportWidth:650},{viewportHeight:600},
    {viewportWidth:NaN},{viewportHeight:0},{visible:[]},
    {visible:[{index:19,dummy:false,inkNodes:5}]},
    {visible:[{index:18,dummy:true,inkNodes:0}]},
    {visible:[{index:18,dummy:false,inkNodes:5},{index:19,dummy:false,inkNodes:5}]}]) {
    assert.equal(stableSamplePair(settled,{...settled,...change}),false);
    assert.equal(stableSamplePair({...settled,...change},settled),false);
  }
  assert.equal(stableSamplePair(null,settled),false);
  assert.equal(stableSamplePair(settled,null),false);
  const callbacks = new Map(); let nextToken = 0;
  const stats = { frameGaps: [] };
  const sampler = createFrameSampler(stats, callback => { callbacks.set(++nextToken, callback); return nextToken; }, token => callbacks.delete(token));
  const tick = (token, now) => { const callback = callbacks.get(token); callbacks.delete(token); callback(now); };
  sampler.start(); tick(1, 0);
  const stale = callbacks.get(2); sampler.stop(); sampler.start();
  stale(100); // Simulates a canceled callback already queued by the browser.
  assert.equal(callbacks.size, 1);
  tick(3, 200); tick(4, 216);
  assert.deepEqual(stats.frameGaps, [16]);
  sampler.start(); assert.equal(callbacks.size, 1);
  sampler.stop(); assert.equal(callbacks.size, 0);
  const start = { viewport: VIEWPORT, scale: 1, page: 18, visible: [18], anchorFraction: 0.15 };
  assertSampleStart(start);
  for (const change of [{ scale: 1.1 }, { page: 19, visible: [19] }, { anchorFraction: 0.25 }, { viewport: { width: 650, height: 700 } }]) {
    assert.throws(() => assertSampleStart({ ...start, ...change }));
  }
}
// Passed directly to page.evaluate/waitForFunction; no application instrumentation.
function observe() {
  const doc = document.getElementById('typst-container')?.documents?.[0]?.impl;
  const svg = doc?.hookedElem?.firstElementChild;
  const scroll = doc?.hookedElem?.parentElement;
  if (!svg || !scroll) return null;
  const viewport = scroll.getBoundingClientRect();
  const groups = [...svg.children].filter(node => node.matches('g.typst-page'));
  const all = groups.map((group, index) => {
    const matrix = group.getScreenCTM();
    const width = Number(group.getAttribute('data-page-width'));
    const height = Number(group.getAttribute('data-page-height'));
    const top = matrix.f, left = matrix.e;
    const right = left + width * matrix.a, bottom = top + height * matrix.d;
    return {
      index, dummy: group.getAttribute('data-dummy') === '1',
      inkNodes: group.querySelectorAll('use,path,text,rect:not(.typst-page-inner)').length,
      visible: Math.min(bottom, viewport.bottom) - Math.max(top, viewport.top) > 8 &&
        Math.min(right, viewport.right) - Math.max(left, viewport.left) > 8,
      top, bottom, height: bottom - top,
      // Marker ink cannot be expected when zoom/pan shows only blank paper or
      // moves the stripe offscreen. This uses known fixture geometry, never
      // rendered ink bounds, so missing SVG cannot satisfy pixel acceptance.
      markerVisible: Math.min(top+(24+(window.tttProbeMarkerHeight ?? 632))*matrix.d,viewport.bottom)-Math.max(top+24*matrix.d,viewport.top)>8 &&
        Math.min(left+32*matrix.a,viewport.right)-Math.max(left+24*matrix.a,viewport.left)>2
    };
  });
  return {
    pages: groups.length, populated: all.filter(item => !item.dummy).length,
    nodes: svg.querySelectorAll('*').length, canvases: svg.querySelectorAll('canvas').length,
    canvasMixins: svg.querySelectorAll('[data-mixin-canvas]').length, visible: all.filter(item => item.visible),
    partial: doc.partialRendering, scale: doc.currentScaleRatio,
    scrollTop: scroll.scrollTop, maxScroll: scroll.scrollHeight - scroll.clientHeight,
    viewportWidth: scroll.clientWidth, viewportHeight: scroll.clientHeight, viewportTop: viewport.top, all
  };
}
// Inspect physical SVG scale, not only the frontend's zoom percentage.
async function verifyZoomGeometry(page, mode, contract) {
  return page.evaluate(async ({partial}) => {
    const doc=document.getElementById('typst-container').documents[0].impl;
    const settle=async()=>{await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);};
    window.tiptoptypPreviewAction({action:'fit'});await settle();
    window.tiptoptypPreviewAction({action:'page',value:0});await settle();
    const read=()=>{const group=[...doc.hookedElem.firstElementChild.children].find(n=>n.matches('g.typst-page'));
      return {zoom:doc.currentScaleRatio,paperScale:group.getScreenCTM().a};};
    const baseline=read(),checks=[],errors=[];
    let patches=0;const rerender=doc.r.rerender;
    doc.r.rerender=function(...args){patches++;return rerender.apply(this,args);};
    try {
      // A tiny change stays within the same buffered pages. Fractional paper
      // sizes must not force a WASM patch and detach the current gesture ink.
      doc.hookedElem.dispatchEvent(new WheelEvent('wheel',{bubbles:true,cancelable:true,ctrlKey:true,
        deltaY:-1,clientX:200,clientY:200}));await settle();
      if(partial && patches!==0)errors.push(`covered fractional zoom rebuilt SVG ${patches} times`);
      window.tiptoptypPreviewAction({action:'fit'});await settle();
      let expected=1;
      for(const action of [...Array(8).fill('zoom-in'),...Array(8).fill('zoom-out')]) {
        window.tiptoptypPreviewAction({action});expected*=action==='zoom-in'?1.1:1/1.1;
        await settle();
        const state=read();checks.push({action,expected,...state});
        if(Math.abs(state.zoom-expected)>1e-8)errors.push('zoom inputs did not accumulate');
        if(Math.abs(state.paperScale/baseline.paperScale/expected-1)>.005)errors.push('paper scale does not match zoom');
      }
      window.tiptoptypPreviewAction({action:'fit'});await settle();
      return {baseline,checks,patches,errors};
    } finally {doc.r.rerender=rerender;}
  }, {partial:mode==='partial'}).then(result=>{
    for(const error of result.errors)contract(false,`zoom geometry: ${error}`);
    return result;
  });
}
// Optional visual acceptance only: this branch never runs in the default
// matched-performance recipe and does not modify source or renderer resources.
async function verifyPalettes(page, mode, contract) {
  const states = [
    { name: 'identity', bg: [255,255,255], fg: [0,0,0], save: true },
    { name: 'dark', bg: [38,38,38], fg: [235,219,178], save: true },
    { name: 'tinted', bg: [238,245,252], fg: [20,40,70] },
    { name: 'inverted', bg: [0,0,0], fg: [255,255,255] },
    { name: 'identity-restored', bg: [255,255,255], fg: [0,0,0], save: true },
  ];
  const results = [];
  const runId = `${Date.now()}-${mode}`;
  const directory = path.join(__dirname, '../.tiptoptyp/screenshots/agent-review/partial-optimize');
  for (const palette of states) {
    await page.evaluate(async ({bg,fg}) => {
      window.tiptoptypSetPalette(bg,fg,'1px solid transparent','1px solid transparent',[79,140,255]);
      await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame);
    }, palette);
    const state = await page.evaluate(() => {
      const doc = document.getElementById('typst-container').documents[0].impl;
      const svg = doc.hookedElem.firstElementChild;
      const groups = [...svg.children].filter(node => node.matches('g.typst-page'));
      const group = groups[0], matrix = group.getScreenCTM();
      const point = (x,y) => ({x:matrix.e+x*matrix.a+y*matrix.c,y:matrix.f+x*matrix.b+y*matrix.d});
      return {
        partial:doc.partialRendering,
        filters:groups.map(page => getComputedStyle(page).filter),
        clips:groups.map(page => {
          const clip=page.getAttribute('clip-path')??'', id=clip.match(/^url\(#(typst-page-clip-\d+)\)$/)?.[1];
          const rect=id?svg.querySelector(`[id="${id}"] rect`):null;
          return {clip,width:Number(page.getAttribute('data-page-width')),height:Number(page.getAttribute('data-page-height')),
            clipWidth:rect?Number(rect.getAttribute('width')):null,clipHeight:rect?Number(rect.getAttribute('height')):null};
        }),
        canvases:svg.querySelectorAll('canvas').length,canvasMixins:svg.querySelectorAll('[data-mixin-canvas]').length,
        // The fixture leaves 24pt margins and places an 8pt colored stripe at
        // the upper-left content origin. These points lie safely inside solid
        // paper/stripe pixels, away from text, page edges and antialiasing.
        paper:point(Number(group.getAttribute('data-page-width'))-12,140),
        stripe:point(28,140),
      };
    });
    contract(state.partial === (mode==='partial'),`${palette.name}: effective mode changed during palette checks`);
    const identity=palette.name.startsWith('identity');
    state.filters.forEach((filter,index) => contract(
      identity?filter==='none':filter.includes('#tiptoptyp-palette'),
      `${palette.name}: page ${index} has unexpected filter ${filter}`));
    contract(state.canvases===0&&state.canvasMixins===0,`${palette.name}: palette introduced canvas fallbacks`);
    state.clips.forEach((clip,index) => contract(clip.clipWidth===clip.width&&clip.clipHeight===clip.height,
      `${palette.name}: page ${index} lost its existing page-sized clipPath`));
    const png=await page.screenshot();
    const expectedStripe=marker(0).map((value,index)=>Math.round(palette.fg[index]+(palette.bg[index]-palette.fg[index])*value/255));
    const pixels=await page.evaluate(async ({png,paper,stripe,expectedPaper,expectedStripe}) => {
      const bitmap=await createImageBitmap(await (await fetch(`data:image/png;base64,${png}`)).blob());
      const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
      const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
      const sample=(point,expected)=>{
        const x=Math.round(point.x),y=Math.round(point.y);
        if(x<1||y<1||x>=canvas.width-1||y>=canvas.height-1)return {inBounds:false,matching:0};
        const values=context.getImageData(x-1,y-1,3,3).data;let matching=0;
        for(let offset=0;offset<values.length;offset+=4) {
          if(expected.every((value,index)=>Math.abs(value-values[offset+index])<=3)&&values[offset+3]===255)matching++;
        }
        return {inBounds:true,matching,center:[...values.slice(16,20)]};
      };
      return {paper:sample(paper,expectedPaper),stripe:sample(stripe,expectedStripe)};
    },{png:png.toString('base64'),paper:state.paper,stripe:state.stripe,expectedPaper:palette.bg,expectedStripe});
    contract(pixels.paper.inBounds&&pixels.paper.matching===9,`${palette.name}: actual paper pixels differ from requested palette (${JSON.stringify(pixels.paper)})`);
    contract(pixels.stripe.inBounds&&pixels.stripe.matching===9,`${palette.name}: actual colored stripe pixels differ from palette transform (${JSON.stringify(pixels.stripe)})`);
    let capture;
    if(palette.save) {
      fs.mkdirSync(directory,{recursive:true});capture=path.join(directory,`${runId}-${palette.name}.png`);fs.writeFileSync(capture,png);
    }
    results.push({name:palette.name,bg:palette.bg,fg:palette.fg,filter:state.filters[0],pages:state.filters.length,
      canvases:state.canvases,canvasMixins:state.canvasMixins,clipsIntact:state.clips.every(clip=>clip.clipWidth===clip.width&&clip.clipHeight===clip.height),
      expectedStripe,pixels,...(capture?{capture,kind:'standalone Chromium viewport; not composed native editor'}:{})});
  }
  return results;
}
// Real page matrices prove semantic source coordinates stay under the wheel
// pointer, including the normal patch fallback after lightweight geometry reuse
// rejects a deliberately invalidated background association. Opt-in and untimed.
async function verifyGeometry(page, contract) {
  const target={page:42,x:250,y:250};
  // fit is queued for the next frame; settle it before a smooth location
  // action, otherwise its scrollBy can cancel that navigation at page zero.
  await page.evaluate(async () => {
    window.tiptoptypPreviewAction({action:'fit'});
    await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
  });
  await page.evaluate(value => window.tiptoptypPreviewAction({action:'location',value}),target);
  // Poll actual source geometry in one bounded async evaluation. Do not assume
  // the automation poller's Promise truthiness means a smooth scroll settled.
  const source=await page.evaluate(async target => {
    let previous=null,last=null,stable=0;
    for(let attempt=0;attempt<120;attempt++) {
      await new Promise(requestAnimationFrame);
      const doc=document.getElementById('typst-container').documents[0].impl, scroll=doc.hookedElem.parentElement;
      const group=doc.hookedElem.firstElementChild.querySelector(`g.typst-page[data-page-number="${target.page}"]`),matrix=group?.getScreenCTM();
      const viewport=scroll.getBoundingClientRect();
      const pointer=matrix?{x:matrix.e+target.x*matrix.a+target.y*matrix.c,y:matrix.f+target.x*matrix.b+target.y*matrix.d}:null;
      const expectedY=viewport.top+doc.windowElem.clientHeight*.382;
      const scrollTop=scroll.scrollTop;
      stable=previous!==null&&Math.abs(scrollTop-previous)<1?stable+1:0;previous=scrollTop;
      last={partial:doc.partialRendering,scale:doc.currentScaleRatio,pointer,expectedY,scrollTop,attempts:attempt+1,
        visible:!!group&&group.getAttribute('data-dummy')!=='1',
        inside:!!pointer&&pointer.x>viewport.left&&pointer.x<viewport.right&&pointer.y>viewport.top&&pointer.y<viewport.bottom,
        sourceError:pointer?Math.abs(pointer.y-expectedY):null};
      if(last.visible&&last.inside&&last.sourceError<=2&&stable>=2)return {...last,settled:true};
    }
    return {...last,settled:false};
  },target);
  contract(source.settled&&source.partial&&source.visible&&source.inside,
    `geometry: far-page source location did not land in rendered paper (${JSON.stringify({target,source})})`);
  contract(source.sourceError!==null&&source.sourceError<=2,`geometry: source location anchor drifted (${JSON.stringify({target,source})})`);
  const checks=[{name:'reuse',delta:-12,fallback:false},{name:'normal-patch-fallback',delta:-12,fallback:true},
    {name:'reverse-reuse',delta:12,fallback:false},{name:'reverse-normal-patch-fallback',delta:12,fallback:true}];
  const results=[];
  for(const check of checks) {
    const frames=await page.evaluate(async ({target,pointer,check}) => {
      const doc=document.getElementById('typst-container').documents[0].impl;
      if(check.fallback) {
        const background=doc.hookedElem.firstElementChild.querySelector(`.typst-page-inner[data-page-number="${target.page}"]`);
        if(!background)throw Error('geometry: expected paper background unavailable');
        background.setAttribute('data-page-number','-1');
      }
      doc.hookedElem.dispatchEvent(new WheelEvent('wheel',{bubbles:true,cancelable:true,ctrlKey:true,deltaY:check.delta,
        clientX:pointer.x,clientY:pointer.y}));
      const frames=[];
      for(let frame=0;frame<2;frame++) {
        await new Promise(requestAnimationFrame);
        const svg=doc.hookedElem.firstElementChild, group=svg.querySelector(`g.typst-page[data-page-number="${target.page}"]`),matrix=group.getScreenCTM();
        const point={x:matrix.e+target.x*matrix.a+target.y*matrix.c,y:matrix.f+target.x*matrix.b+target.y*matrix.d};
        frames.push({frame:frame+1,scale:doc.currentScaleRatio,point,error:Math.hypot(point.x-pointer.x,point.y-pointer.y),
          backgroundRestored:!!svg.querySelector(`.typst-page-inner[data-page-number="${target.page}"]`)&&!svg.querySelector('.typst-page-inner[data-page-number="-1"]'),
          coverage:window.tttProbeObserve()});
      }
      return frames;
    },{target,pointer:source.pointer,check});
    for(const frame of frames) {
      contract(frame.error<=2,`geometry ${check.name} RAF ${frame.frame}: source point moved ${frame.error}px away from wheel pointer`);
      contract(frame.backgroundRestored,`geometry ${check.name} RAF ${frame.frame}: normal patch did not restore paper association`);
      for(const failure of coverageFailures(frame.coverage,true))contract(false,`geometry ${check.name} RAF ${frame.frame}: ${failure}`);
    }
    results.push({name:check.name,deltaY:check.delta,forcedNormalPatch:check.fallback,frames:frames.map(({coverage,...frame})=>
      ({...frame,visible:coverage.visible.map(item=>item.index),canvases:coverage.canvases,canvasMixins:coverage.canvasMixins}))});
  }
  await page.evaluate(async () => {
    window.tiptoptypPreviewAction({action:'fit'});
    await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
    window.tiptoptypPreviewAction({action:'page',value:0});
    await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
  });
  return {target,source,checks:results};
}
async function runLane({ mode, adapter, executable, source, measureOnly, paletteChecks = false, geometryChecks = false, zoomChecks = false }) {
  const { chromium } = require(process.env.TIPTOPTYP_PLAYWRIGHT || 'playwright');
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), `ttt-preview-${mode}-`));
  const file = path.join(directory, 'main.typ');
  fs.writeFileSync(file, source);
  const flags = ['preview', '--no-open', '--root', directory, '--host', '127.0.0.1:0',
    '--data-plane-host', '127.0.0.1:0', '--control-plane-host', '127.0.0.1:0', '--invert-colors', 'never'];
  if (mode === 'partial') flags.push('--partial-rendering=true');
  const server = spawn(executable, [...flags, file]);
  let log = '', browser, page;
  const failures = [], observations = [], samples = [], captures = [], sampleStarts = [];
  const watchdog = setTimeout(() => { server.kill(); browser?.close(); }, 150000);
  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(Error(`Preview startup timed out: ${log.slice(-2000)}`)), 15000);
      const read = chunk => {
        log = (log + chunk).slice(-16000);
        if (/Static file server listening on: ([\d.:]+)/.test(log) && /Control panel server listening on: ([\d.:]+)/.test(log)) {
          clearTimeout(timeout); resolve();
        }
      };
      server.stdout.on('data', read); server.stderr.on('data', read);
      server.on('error', error => { clearTimeout(timeout); reject(error); });
      server.on('exit', code => { clearTimeout(timeout); reject(Error(`Tinymist exited ${code}: ${log.slice(-2000)}`)); });
    });
    const url = `http://${log.match(/Static file server listening on: ([\d.:]+)/)[1]}`;
    const control = `ws://${log.match(/Control panel server listening on: ([\d.:]+)/)[1]}`;
    browser = await chromium.launch({ headless: true,
      ...(process.env.TIPTOPTYP_TEST_CHROME ? { executablePath: process.env.TIPTOPTYP_TEST_CHROME } : {}) });
    page = await browser.newPage({ viewport: VIEWPORT, deviceScaleFactor: 1 });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.messages = [];
      window.ipc = { postMessage: message => {
        const value = JSON.parse(message);
        window.messages.push(value);
        if (window.messages.length > 256) window.messages.shift();
      } };
    });
    await page.addInitScript({ content: `window.tttProbeObserve = ${observe.toString()}; window.tttCreateFrameSampler = ${createFrameSampler.toString()}; window.tttStableSamplePair = ${stableSamplePair.toString()};` });
    await page.addInitScript({content:`window.tttProbeMarkerHeight = ${source.includes('height: 793pt') ? 793 : 632};`});
    await page.addInitScript(adapter);
    await page.goto(url);
    await page.waitForFunction(() => document.getElementById('typst-container')?.documents?.[0]?.impl?.moduleInitialized);
    await page.evaluate(async endpoint => {
      window.outlineSerial = 0;
      window.control = new WebSocket(endpoint);
      window.control.onmessage = event => {
        const message = JSON.parse(event.data);
        if (message.event === 'outline') { window.outline = message.items; window.outlineSerial++; }
      };
      await new Promise((resolve, reject) => { window.control.onopen = resolve; window.control.onerror = reject; });
    }, control);
    fs.appendFileSync(file, '\n');
    await page.waitForFunction(() => window.outline?.length === 48, null, { timeout: 15000 });
    await page.waitForTimeout(3000);
    assert.equal((await page.evaluate(observe)).partial, mode === 'partial', 'Verify the effective renderer mode');
    const contract = (condition, message) => {
      if (condition) return;
      failures.push(message);
      if (!measureOnly) assert.fail(message);
    };
    const observeCoverage = async name => {
      await page.waitForFunction(() => {
        const doc = document.getElementById('typst-container')?.documents?.[0]?.impl;
        if (!doc) return false;
        const viewport = doc.hookedElem.parentElement.getBoundingClientRect();
        const groups = [...doc.hookedElem.firstElementChild.children].filter(node => node.matches('g.typst-page'));
        const visible = groups.filter(group => {
          const matrix = group.getScreenCTM();
          const bottom = matrix.f + Number(group.getAttribute('data-page-height')) * matrix.d;
          return Math.min(bottom, viewport.bottom) - Math.max(matrix.f, viewport.top) > 8;
        });
        return visible.length && visible.every(group => group.getAttribute('data-dummy') !== '1' && group.querySelector('use,path,text'));
      }, null, { timeout: 5000 }).catch(error => {
        contract(false, `${name}: settled viewport did not refill (${error.message.split('\n')[0]})`);
      });
      await page.waitForTimeout(100);
      const state = await page.evaluate(observe);
      const problems = coverageFailures(state, mode === 'partial');
      for (const problem of problems) contract(false, `${name}: ${problem}`);
      // Decode real browser output and require the page-specific ink. A dummy
      // group's paper geometry alone cannot satisfy this pixel assertion.
      const png = await page.screenshot();
      if (name === 'initial' || name === 'source-replacement') {
        const captureDirectory = path.join(__dirname, '../.tiptoptyp/screenshots/agent-review/partial-render');
        fs.mkdirSync(captureDirectory, { recursive: true });
        const capture = path.join(captureDirectory, `${Date.now()}-${mode}-${name}.png`);
        fs.writeFileSync(capture, png);
        captures.push({ path: capture, kind: 'standalone Chromium viewport; not composed native editor',
          sourceSha256: sha256(name === 'source-replacement' ? source.replace('#let revision = 0', '#let revision = 1') : source) });
      }
      const counts = await page.evaluate(async ({ png, colors }) => {
        const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${png}`)).blob());
        const canvas = document.createElement('canvas'); canvas.width = bitmap.width; canvas.height = bitmap.height;
        const context = canvas.getContext('2d'); context.drawImage(bitmap, 0, 0); bitmap.close();
        const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
        return colors.map(color => {
          let count = 0;
          for (let offset = 0; offset < pixels.length; offset += 4) {
            if (color.every((value, channel) => Math.abs(value - pixels[offset + channel]) <= 3)) count++;
          }
          return count;
        });
      }, { png: png.toString('base64'), colors: state.visible.map(item => marker(item.index, name === 'source-replacement' ? 1 : 0)) });
      state.visible.forEach((item, index) => contract(!item.markerVisible || counts[index] > 10,
        `${name}: expected visible marker ink for page ${item.index} missing (${counts[index]} pixels)`));
      observations.push({ name, pages: state.pages, populated: state.populated, nodes: state.nodes, canvases: state.canvases,
        visible: state.visible.map(item => item.index), pixels: counts, scrollTop: state.scrollTop, scale: state.scale });
      return state;
    };
    const transientFrames = [];
    const checkFrames = (name, states) => {
      states.forEach((state, frame) => {
        for (const problem of coverageFailures(state, mode === 'partial')) contract(false, `${name} RAF ${frame + 1}: ${problem}`);
        transientFrames.push({ name, frame: frame + 1, populated: state?.populated, nodes: state?.nodes,
          canvases: state?.canvases, visible: (state?.visible ?? []).map(item => ({ index: item.index, dummy: item.dummy, inkNodes: item.inkNodes })) });
      });
    };
    const jump = async (index, offset = 0.15) => {
      const states = await page.evaluate(async ({ index, offset }) => {
        const doc = document.getElementById('typst-container').documents[0].impl;
        const scroll = doc.hookedElem.parentElement;
        const groups = [...doc.hookedElem.firstElementChild.children].filter(node => node.matches('g.typst-page'));
        const group = groups[index], matrix = group.getScreenCTM();
        doc.clearSvgResizeAnchor();
        const top = matrix.f - scroll.getBoundingClientRect().top + scroll.scrollTop;
        scroll.scrollTo({ top: top + Number(group.getAttribute('data-page-height')) * matrix.d * offset, behavior: 'instant' });
        const states = [];
        for (let frame = 0; frame < 2; frame++) { await new Promise(requestAnimationFrame); states.push(window.tttProbeObserve()); }
        return states;
      }, { index, offset });
      checkFrames(`scroll-${index}`, states);
    };
    await observeCoverage('initial');
    const zoomResults = zoomChecks ? await verifyZoomGeometry(page, mode, contract) : null;
    const paletteResults = paletteChecks ? await verifyPalettes(page, mode, contract) : null;
    const geometryResults = geometryChecks && mode === 'partial' ? await verifyGeometry(page, contract) : null;
    for (const index of [17, 42, 3, 47, 0]) { await jump(index); await observeCoverage(`scroll-${index}`); }
    const rapid = await page.evaluate(async () => {
      const doc = document.getElementById('typst-container').documents[0].impl;
      const scroll = doc.hookedElem.parentElement;
      for (const fraction of [1, 0, 0.9, 0.1, 0.7, 0.25]) scroll.scrollTo({ top: (scroll.scrollHeight - scroll.clientHeight) * fraction, behavior: 'instant' });
      const states = [];
      for (let frame = 0; frame < 2; frame++) { await new Promise(requestAnimationFrame); states.push(window.tttProbeObserve()); }
      return states;
    });
    checkFrames('rapid-jumps', rapid);
    await observeCoverage('rapid-jumps');
    // Continuous events deliberately outlast upstream's 500 ms debounce.
    const moving = await page.evaluate(async ({ file, source }) => {
      const doc = document.getElementById('typst-container').documents[0].impl;
      const scroll = doc.hookedElem.parentElement;
      doc.clearSvgResizeAnchor();
      const states = [];
      for (let step = 0; step < 60; step++) {
        scroll.scrollTo({ top: (scroll.scrollHeight - scroll.clientHeight) * step / 59, behavior: 'instant' });
        if (step === 30) window.control.send(JSON.stringify({ event: 'updateMemoryFiles', files: { [file]: source.replace('#let revision = 0', '#let revision = 2') } }));
        await new Promise(requestAnimationFrame); states.push(window.tttProbeObserve());
        await new Promise(requestAnimationFrame); states.push(window.tttProbeObserve());
        await new Promise(resolve => setTimeout(resolve, 20));
      }
      return states;
    }, { file, source });
    checkFrames('continuous-scroll-with-source-change', moving);
    await observeCoverage('continuous-scroll');
    await jump(0, 0);
    await observeCoverage('before-link');
    await page.locator('g.typst-page[data-page-number="0"] a').first().click();
    await page.waitForFunction(() => window.messages.at(-1)?.page === 40);
    await observeCoverage('link-40');
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'back' }));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 0);
    await observeCoverage('link-back');
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'forward' }));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 40);
    await observeCoverage('link-forward');
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'page', value: 31 }));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 31);
    await observeCoverage('page-navigation');
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'zoom-in' }));
    await page.waitForFunction(() => document.getElementById('typst-container').documents[0].impl.currentScaleRatio > 1);
    await observeCoverage('zoom');
    for (const width of [650, 1050, 900]) { await page.setViewportSize({ width, height: 700 }); await observeCoverage(`resize-${width}`); }
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'fit' }));
    await page.waitForFunction(() => document.getElementById('typst-container').documents[0].impl.currentScaleRatio === 1);
    await observeCoverage('fit');
    const outlineSerial = await page.evaluate(() => window.outlineSerial);
    await page.evaluate(({ file, source }) => window.control.send(JSON.stringify({ event: 'updateMemoryFiles', files: { [file]: source } })),
      { file, source: source.replace('#let revision = 0', '#let revision = 1') });
    await page.waitForFunction(previous => window.outlineSerial > previous, outlineSerial);
    await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'page', value: 31 }));
    await observeCoverage('source-replacement');
    // Restore identical source/geometry before comparing timed workloads.
    await page.evaluate(({ file, source }) => window.control.send(JSON.stringify({ event: 'updateMemoryFiles', files: { [file]: source } })), { file, source });
    await page.waitForTimeout(1000);
    await jump(18, 0.15);
    await observeCoverage('measurement-start');
    await page.waitForTimeout(3000);
    await page.evaluate(() => {
      const doc = document.getElementById('typst-container').documents[0].impl;
      window.probe = { rescale: 0, rerender: 0, viewport: 0, rescaleSyncMs: 0, rerenderSyncMs: 0, frameGaps: [] };
      for (const name of ['rescale', 'rerender']) {
        const original = doc.r[name];
        doc.r[name] = function(...args) {
          window.probe[name]++;
          const before = performance.now();
          const value = original.apply(this, args);
          window.probe[`${name}SyncMs`] += performance.now() - before;
          return value;
        };
      }
      const original = doc.addViewportChange;
      doc.addViewportChange = function(...args) { window.probe.viewport++; return original.apply(this, args); };
      window.probeFrames = window.tttCreateFrameSampler(window.probe, requestAnimationFrame.bind(window), cancelAnimationFrame.bind(window));
    });
    // One untimed workload warmup followed by five same-sequence samples.
    for (let sample = -1; sample < 5; sample++) {
      // Reset outside all measured counters so each run starts at the same
      // physical page, scale, viewport and offset, even after anchor rounding.
      await page.setViewportSize(VIEWPORT);
      await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'fit' }));
      await page.waitForFunction(() => document.getElementById('typst-container').documents[0].impl.currentScaleRatio === 1);
      await jump(18, 0.15);
      await page.waitForTimeout(800);
      const stability = await page.evaluate(async () => {
        let after=null;
        // This Playwright version does not poll Promise predicates. Await the
        // two real RAF observations explicitly, bounded outside sample timing.
        for(let attempt=0;attempt<30;attempt++) {
          const before=window.tttProbeObserve();
          await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
          after=window.tttProbeObserve();
          if(window.tttStableSamplePair(before,after))return {settled:true,attempts:attempt+1};
        }
        return {settled:false,scale:after?.scale,scrollTop:after?.scrollTop,
          visible:after?.visible.map(item=>({index:item.index,dummy:item.dummy,inkNodes:item.inkNodes}))};
      });
      assert.ok(stability.settled,`Sample reset did not reach two stable rendered RAFs: ${JSON.stringify(stability)}`);
      const reset = await page.evaluate(observe);
      const anchor = reset.all[18];
      const startState = { viewport: page.viewportSize(), scale: reset.scale, page: reset.visible[0]?.index,
        visible: reset.visible.map(item => item.index), scrollTop: reset.scrollTop,
        anchorFraction: (reset.viewportTop - anchor.top) / anchor.height, populated: reset.populated, nodes: reset.nodes };
      assertSampleStart(startState);
      sampleStarts.push({ sample, ...startState });
      await page.evaluate(() => {
        const stats = window.probe;
        for (const key of ['rescale', 'rerender', 'viewport', 'rescaleSyncMs', 'rerenderSyncMs']) stats[key] = 0;
        window.probeFrames.start();
      });
      const started = Date.now();
      for (let step = 0; step < 36; step++) {
        await page.setViewportSize({ width: 900 - Math.round(250 * Math.sin(step / 35 * Math.PI)), height: 700 });
        await page.waitForTimeout(20);
      }
      await page.evaluate(async () => {
        const doc = document.getElementById('typst-container').documents[0].impl;
        const scroll = doc.hookedElem.parentElement;
        const start = scroll.scrollTop;
        for (let step = 0; step < 24; step++) {
          scroll.scrollTo({ top: start + 3000 * Math.sin(step / 23 * Math.PI), behavior: 'instant' });
          await new Promise(resolve => setTimeout(resolve, 20));
        }
        for (let step = 0; step < 10; step++) {
          doc.hookedElem.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, ctrlKey: true,
            deltaY: step < 5 ? -10 : 10, clientX: 350, clientY: 200 }));
          await new Promise(resolve => setTimeout(resolve, 20));
        }
      });
      await page.waitForTimeout(800);
      const stats = await page.evaluate(() => {
        window.probeFrames.stop();
        const frames = [...window.probe.frameGaps].sort((left, right) => left - right);
        return { rescale: window.probe.rescale, rerender: window.probe.rerender, viewport: window.probe.viewport,
          rescaleSyncMs: window.probe.rescaleSyncMs, rerenderSyncMs: window.probe.rerenderSyncMs,
          frameCount: frames.length, frameGapP95Ms: frames[Math.floor(frames.length * 0.95)] ?? 0,
          frameGapMaxMs: frames.at(-1) ?? 0 };
      });
      if (sample >= 0) samples.push({ sample, startState, workloadWallMs: Date.now() - started, ...stats });
    }
    await observeCoverage('measurement-end');
    await page.waitForTimeout(1000);
    const idleBefore = await page.evaluate(() => ({ rescale: window.probe.rescale, rerender: window.probe.rerender, viewport: window.probe.viewport }));
    await page.waitForTimeout(1500);
    const idleAfter = await page.evaluate(() => ({ rescale: window.probe.rescale, rerender: window.probe.rerender, viewport: window.probe.viewport }));
    for (const change of idleChanges(idleBefore, idleAfter)) {
      contract(false, `Idle document performed ${change} work: ${JSON.stringify({ idleBefore, idleAfter })}`);
    }
    contract(errors.length === 0, `Browser errors: ${JSON.stringify(errors)}`);
    return { mode, browser: await browser.version(), effectivePartial: mode === 'partial', validated: !measureOnly,
      failures, observations, transientFrames, captures, sampleStarts, samples, idleBefore, idleAfter, errors,
      ...(paletteResults ? {paletteChecks:paletteResults} : {}), ...(geometryResults ? {geometryChecks:geometryResults} : {}), ...(zoomResults ? {zoomChecks:zoomResults} : {}) };
  } catch (error) {
    const state = page ? await page.evaluate(observe).catch(() => null) : null;
    throw Error(`${error.message}\n${JSON.stringify({ mode, state, log: log.slice(-2000) })}`);
  } finally {
    clearTimeout(watchdog);
    await browser?.close();
    server.kill();
    await new Promise(resolve => { if (server.exitCode !== null || server.signalCode !== null) resolve(); else server.once('exit', resolve); });
    fs.rmSync(directory, { recursive: true, force: true });
  }
}
async function run() {
  if (process.argv.includes('--help')) {
    console.log('Usage: node scripts/test-preview-e2e.cjs --large [--mode full|partial|both] [--adapter FILE] [--output FILE] [--measure-only] [--palette-checks] [--geometry-checks] [--fractional-pages] [--zoom-checks] [--stress]\n' +
      'Set TIPTOPTYP_TEST_TINYMIST to the pinned optimized binary, TIPTOPTYP_PLAYWRIGHT to Playwright and optionally TIPTOPTYP_TEST_CHROME.\n' +
      '--self-test runs only browser-free coverage checks. --measure-only records failures and marks the receipt unvalidated.\n' +
      '--palette-checks adds untimed same-document identity/dark/tinted/inverted pixel, filter and clipping acceptance.\n' +
      '--geometry-checks adds untimed partial-only far-page source and Ctrl-wheel pointer anchoring, including normal patch fallback.\n' +
      '--fractional-pages uses ISO A4 dimensions; --zoom-checks asserts repeated physical zoom and covered SVG reuse.\n' +
      '--stress uses the 48-page dense matrix/nested-math fixture with unequal fractional widths and a separately recorded 320,000-node partial bound.');
    return;
  }
  selfTest();
  if (process.argv.includes('--self-test')) { console.log('Partial-preview coverage helper checks passed'); return; }
  const probeSha256 = sha256(fs.readFileSync(__filename));
  const mode = argument('--mode', 'both');
  assert.ok(['full', 'partial', 'both'].includes(mode), '--mode must be full, partial or both');
  const executable = process.env.TIPTOPTYP_TEST_TINYMIST;
  assert.ok(executable, 'Set TIPTOPTYP_TEST_TINYMIST to the pinned optimized binary');
  const fixture = path.join(__dirname, STRESS ? 'fixtures/tinymist-zoom-stress.typ' : 'fixtures/tinymist-partial-large.typ');
  const fractionalPages = process.argv.includes('--fractional-pages');
  assert.ok(!STRESS || !fractionalPages, '--stress already has fractional paper sizes; omit --fractional-pages');
  const originalSource = fs.readFileSync(fixture, 'utf8');
  const source = fractionalPages ? originalSource.replace('width: 500pt, height: 680pt', 'width: 210mm, height: 297mm').replace('height: 632pt', 'height: 793pt') : originalSource;
  const adapterPath = path.resolve(argument('--adapter', path.join(__dirname, '../src/preview_navigation.js')));
  const adapter = fs.readFileSync(adapterPath, 'utf8');
  const measureOnly = process.argv.includes('--measure-only');
  const paletteChecks = process.argv.includes('--palette-checks');
  const geometryChecks = process.argv.includes('--geometry-checks');
  const zoomChecks = process.argv.includes('--zoom-checks');
  const results = [];
  for (const lane of mode === 'both' ? ['full', 'partial'] : [mode]) {
    results.push(await runLane({ mode: lane, executable, source, adapter, measureOnly, paletteChecks, geometryChecks, zoomChecks }));
  }
  const receipt = { platform: process.platform, arch: process.arch, fixture: path.relative(path.join(__dirname, '..'), fixture),
    fixtureSha256: sha256(source), ...(fractionalPages ? {fixtureTransform:'ISO A4: 210mm × 297mm'} : {}), probeSha256, tinymistSha256: sha256(fs.readFileSync(executable)), adapterSha256: sha256(adapter),
    stressFixture: STRESS, maxSvgNodes: MAX_NODES, viewport: VIEWPORT, deviceScaleFactor: 1, warmupMs: 3000, workloadWarmups: 1, measuredSamples: 5,
    sampleReset: 'Outside timing/counters: 900×700 viewport, fit scale1, rendered page18 +15% page offset; 800ms settle and two stable RAFs; start state asserted and recorded for warmup and all five samples',
    recipe: '36 resize steps (900→650→900), 24 scroll steps (3000px out/back), 10 Ctrl-wheel steps (−10×5,+10×5); 20ms input spacing; 800ms drain',
    measurement: 'Synchronous renderer spans and browser RAF gaps; includes Playwright scheduling, excludes native WKWebView/GPU and compiler startup. No wall-time thresholds.',
    validated: !measureOnly, ...(paletteChecks ? {paletteChecks:true} : {}), ...(geometryChecks ? {geometryChecks:true} : {}), ...(zoomChecks ? {zoomChecks:true} : {}), results };
  const output = argument('--output', null);
  if (output) { fs.mkdirSync(path.dirname(output), { recursive: true }); fs.writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`); }
  console.log(JSON.stringify(receipt, null, 2));
  return receipt;
}
module.exports = { run, selfTest, coverageFailures, marker, createFrameSampler, assertSampleStart, idleChanges, stableSamplePair };
if (require.main === module) run().catch(error => { console.error(error); process.exitCode = 1; });
