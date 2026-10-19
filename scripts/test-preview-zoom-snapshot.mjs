import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('../src/preview_zoom_snapshot.js', import.meta.url), 'utf8');
function fixture({ provider = true, ready = true, density = 2 } = {}) {
  let serial = 0, reads = 0;
  const timers = new Map(), frames = [], messages = [];
  const doc = {}, svg = { isConnected: true };
  const current = { doc, svg, scale: 1, width: 900, height: 700, fitWidth:880, x: 0, y: 0, left: 0, top: 100, ready };
  const window = { devicePixelRatio: density, addEventListener() {} };
  if (provider) window.tiptoptypCaptureZoomSnapshot = value => messages.push(value);
  vm.runInNewContext(source, { window, Date, Math,
    Image: class { constructor() { throw Error('Image decoding must never happen on the renderer thread'); } },
    document: { createElement() { throw Error('No DOM overlay on the renderer thread'); } },
    setTimeout: fn => { const id = ++serial; timers.set(id, fn); return id; },
    clearTimeout: id => timers.delete(id), requestAnimationFrame: fn => { frames.push(fn); return frames.length; },
  });
  const cache = window.tiptoptypZoomSnapshot;
  cache.attach(() => { reads++; return { ...current }; }); cache.configure(true);
  const tick = () => { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(fn => fn()); };
  const nextFrame = () => { const callbacks = frames.splice(0); callbacks.forEach(fn => fn()); };
  const paint = () => { nextFrame(); nextFrame(); };
  const captures = () => messages.filter(value => value.op === 'capture');
  const admit = (request = captures().at(-1), result = { width: 2239, height: 1742 }) => {
    cache.resolve(request.token, result);
    if (cache.inspect().admitting) cache.admitted(request.token, true);
  };
  return { cache, current, svg, doc, messages, timers, frames, tick, paint, nextFrame, admit, captures, window, get reads() { return reads; } };
}

test('idle capture is bounded and admits only metadata; no decoding, DOM overlay or idle RAF', () => {
  const f = fixture(); assert.equal(f.captures().length,0); f.tick();
  const request=f.captures()[0]; assert.ok(request.density>2); assert.ok(request.width*request.height*request.density**2<=4_000_001);
  f.cache.settle(); f.tick(); assert.equal(f.captures().length,1);
  f.admit(); assert.equal(f.cache.inspect().cached,true); assert.equal(f.messages.at(-1).op,'admit');
  for(let i=0;i<50;i++){f.cache.settle();f.tick();} assert.equal(f.captures().length,1);assert.equal(f.frames.length,0);assert.equal(f.timers.size,0);
});
test('unsupported platforms and disabled settings queue no captures or zoom work',()=>{
  for(const provider of [false,true]){const f=fixture({provider});if(provider)f.cache.configure(false);f.tick();assert.equal(f.captures().length,0);assert.equal(f.cache.request(1.1),false);}
});
test('input asks native compositor and never starts a screenshot or delayed renderer frame',()=>{
  const f=fixture();f.tick();f.admit();assert.equal(f.cache.request(1.1,false,{x:300,y:200}),true);
  assert.deepEqual(JSON.parse(JSON.stringify(f.messages.at(-1))),{op:'zoom',factor:1.1,reset:false,x:300,y:200});
  assert.equal(f.cache.begin(f.doc,1.1),false);assert.equal(f.frames.length,0);f.tick();assert.equal(f.captures().length,1);
});
test('latest sharp handoff waits for paint; older acknowledgements cannot dismiss newer zoom',()=>{
  const f=fixture();f.tick();f.admit(); const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();
  f.cache.native({token,revision:2,scale:1.4});f.paint();assert.equal(f.messages.filter(m=>m.op==='ready').length,0);
  f.current.ready=false;f.current.scale=1.4;f.cache.rendered();f.paint();assert.equal(f.cache.inspect().active,true);
  f.current.ready=true;f.cache.rendered();f.paint();assert.equal(f.cache.inspect().active,false);assert.equal(f.messages.filter(m=>m.op==='ready').at(-1).revision,2);
});
test('source/theme invalidation retires native image and late snapshots without duplicate capture',()=>{
  const f=fixture();f.tick();const old=f.captures()[0];f.cache.invalidate();f.cache.settle();f.tick();assert.equal(f.captures().length,1);
  f.admit(old);assert.equal(f.cache.inspect().cached,false);assert.equal(f.messages.filter(m=>m.op==='reject').length,1);f.tick();assert.equal(f.captures().length,2);
});
test('captures across scroll, resize, SVG replacement or document switch are rejected',()=>{
  for(const change of [{top:200},{width:800},{fitWidth:800},{svg:{isConnected:true}},{doc:{}}]){
    const f=fixture();f.tick();Object.assign(f.current,change);f.admit();assert.equal(f.cache.inspect().cached,false);assert.equal(f.messages.at(-1).op,'reject');
  }
});
test('automatic scroll anchoring during native zoom keeps image; later manual scrolling releases it',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;f.cache.native({token,revision:1,scale:1.2});
  f.current.top=120;f.cache.scrolled();assert.equal(f.cache.inspect().active,true);f.current.scale=1.2;f.cache.rendered();f.paint();
  const count=f.messages.length;f.cache.scrolled();assert.equal(f.messages.length,count);f.current.top=300;f.cache.scrolled();assert.equal(f.messages.filter(m=>m.op==='invalidate').at(-1).op,'invalidate');
});
test('turning off during capture or sharp handoff releases image and rejects old callbacks',()=>{
  const f=fixture();f.tick();const old=f.captures()[0];f.cache.configure(false);f.admit(old);assert.equal(f.cache.inspect().cached,false);
  f.cache.configure(true);f.tick();f.admit();f.cache.native({token:f.captures().at(-1).token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();
  f.cache.configure(false);f.paint();assert.equal(f.messages.filter(m=>m.op==='ready').length,0);assert.equal(f.cache.inspect().active,false);
});
test('failure and oversized results fall back without retry loops',()=>{
  for(const result of [null,{width:4000,height:4000}]){const f=fixture();f.tick();f.admit(f.captures()[0],result);assert.equal(f.cache.inspect().failures,1);assert.equal(f.cache.inspect().cached,false);f.tick();assert.equal(f.captures().length,1);}
});
test('cold zoom cancels idle capture and round trips reject pre-gesture snapshots',()=>{
  const f=fixture();f.cache.begin(f.doc,1.1);f.tick();assert.equal(f.captures().length,0);f.current.ready=false;f.cache.rendered();f.tick();assert.equal(f.captures().length,0);
  f.current.ready=true;f.cache.rendered();f.tick();const request=f.captures()[0];f.cache.begin(f.doc,1.1);f.cache.begin(f.doc,1);f.admit(request);assert.equal(f.cache.inspect().cached,false);
});
test('native request validation rejects malformed scales and revisions',()=>{
  const f=fixture();for(const request of [{scale:NaN,revision:1},{scale:1,revision:1.5},{scale:1}])assert.equal(f.cache.native(request),false);
});

test('repeated settled notifications schedule one handoff frame, not a repaint queue',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:1,scale:1.2});f.current.scale=1.2;
  for(let i=0;i<1000;i++)f.cache.rendered();assert.equal(f.frames.length,1);f.paint();assert.equal(f.cache.inspect().handoffs,1);
});

test('source or palette retires pixels without abandoning the latest renderer acknowledgement',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;f.cache.native({token,revision:1,scale:1.2});
  f.cache.invalidate();assert.equal(f.cache.inspect().cached,false);assert.equal(f.cache.inspect().active,true);
  f.current.scale=1.2;f.cache.rendered();f.paint();assert.equal(f.messages.filter(m=>m.op==='ready').at(-1).token,token);
});

test('buffered capture includes its opaque palette and keeps the physical pixel cap',()=>{
  const f=fixture();
  // The production reader provides the palette and inert buffered SVG payload.
  f.current.background=[24,31,42];f.current.geometry={paper:[0,0,900,2100]};
  f.tick();const request=f.captures()[0];assert.deepEqual(Array.from(request.background),[24,31,42]);
  assert.equal(request.geometry.paper[3],2100);
});
test('native divider resize cancels idle capture and retains pixels until its acknowledged sharp frame',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.current.width=800;assert.equal(f.cache.resize(),true);assert.equal(f.messages.at(-1).op,'resize');
  f.current.ready=false;f.cache.rendered();f.tick();assert.equal(f.captures().length,1);assert.equal(f.cache.inspect().active,false);
  f.current.ready=true;f.current.width=800;f.cache.native({token,revision:1,scale:1,resize:true});f.cache.rendered();f.paint();
  assert.equal(f.messages.filter(m=>m.op==='ready').at(-1).revision,1);
  assert.equal(f.cache.inspect().active,false);
});


test('cold startup resize leaves the renderer in charge instead of waiting for a missing native acknowledgement',()=>{
  for(const ready of [false,true]) {
    const f=fixture({ready});
    assert.equal(f.cache.resize(),false);
    assert.equal(f.cache.inspect().active,false);
    assert.equal(f.messages.filter(m=>m.op==='resize').length,0);
    f.current.ready=true;f.cache.rendered();f.tick();f.admit();
    f.current.width=800;assert.equal(f.cache.resize(),true);
    f.current.ready=false;
    assert.equal(f.cache.resize(),true,'an already active native gesture retains resize ownership');
  }
});


test('sharp handoff keeps valid pixels available while refreshing in the background',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.paint();
  assert.equal(f.cache.inspect().active,false);
  assert.equal(f.cache.inspect().cached,true,'sharp handoff created a cold-cache gap');
  f.tick();assert.equal(f.captures().length,2);
  assert.equal(f.cache.inspect().cached,true,'refresh discarded the previous valid image');
  assert.equal(f.cache.resize(),true,'divider must retain its native owner during refresh');
  f.admit(f.captures()[1]);assert.equal(f.cache.inspect().cached,true,'rejected refresh discarded usable pixels');
});
test('clamped top-edge scrolling retains pixels while an idle refresh replaces their token',()=>{
  const f=fixture();f.current.top=0;f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.paint();f.tick();
  const replacement=f.captures()[1];assert.notEqual(replacement.token,token);
  assert.equal(f.cache.inspect().capturing,true);assert.equal(f.cache.inspect().cached,true);
  const invalidations=f.messages.filter(message=>message.op==='invalidate').length;
  f.cache.scrolled();
  assert.equal(f.cache.inspect().cached,true,'clamped scroll discarded the retained image during capture');
  assert.equal(f.cache.inspect().capturing,true,'clamped scroll cancelled the idle replacement');
  f.admit(replacement);f.cache.scrolled();
  assert.equal(f.cache.inspect().cached,true,'admitting a healthy replacement created a cold-cache gap');
  assert.equal(f.messages.filter(message=>message.op==='invalidate').length,invalidations);
  f.cache.native({token:replacement.token,revision:2,scale:1.3});
  assert.equal(f.cache.inspect().hits,2,'the next zoom must use the admitted replacement token');
});
test('refresh failure preserves valid pixels and source changes still flush them',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.paint();f.tick();
  f.admit(f.captures()[1],null);
  assert.equal(f.cache.inspect().cached,true,'failed replacement discarded valid pixels');
  f.cache.invalidate();assert.equal(f.cache.inspect().cached,false);
});


test('native cold fallback discards metadata for an image that cannot cover the viewport',()=>{
  const f=fixture();f.tick();f.admit();
  f.cache.native({token:null,revision:1,scale:.1});
  assert.equal(f.cache.inspect().cached,false);
  f.current.scale=.1;f.cache.rendered();f.paint();f.tick();
  assert.equal(f.captures().length,2);
});


test('native admission acknowledgement rejects a raced refresh without discarding old pixels',()=>{
  const f=fixture();f.tick();f.admit();const old=f.captures()[0].token;
  f.cache.native({token:old,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.paint();f.tick();
  const refresh=f.captures()[1];f.cache.resolve(refresh.token,{width:1000,height:1000});
  assert.equal(f.cache.inspect().admitting,true);assert.equal(f.cache.inspect().cached,true);
  f.cache.native({token:old,revision:2,scale:1.3});f.cache.admitted(refresh.token,false);
  assert.equal(f.cache.inspect().cached,true);assert.equal(f.cache.inspect().admitting,false);
  assert.equal(f.cache.inspect().active,true);
});
test('native input after admission can precede its JS acknowledgement without losing pixels',()=>{
  const f=fixture();f.tick();const request=f.captures()[0];
  f.cache.resolve(request.token,{width:1000,height:1000});
  assert.equal(f.cache.inspect().cached,false);
  f.cache.native({token:request.token,revision:1,scale:1.2});
  assert.equal(f.cache.inspect().cached,true);assert.equal(f.cache.inspect().admitting,false);
  f.cache.admitted(request.token,true);
  assert.equal(f.cache.inspect().cached,true);
  assert.equal(f.messages.filter(m=>m.op==='reject').length,0);
});
test('source changes reject delayed native acceptance and waiting admission cannot duplicate captures',()=>{
  const f=fixture();f.tick();const request=f.captures()[0];
  f.cache.resolve(request.token,{width:1000,height:1000});
  for(let i=0;i<100;i++){f.cache.settle();f.tick();}
  assert.equal(f.captures().length,1);
  f.cache.invalidate();f.cache.admitted(request.token,true);
  assert.equal(f.cache.inspect().cached,false);
  assert.equal(f.messages.at(-1).op,'reject');
});


test('sharp handoff records final anchor scroll after its display boundary',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.current.top=120;
  f.cache.rendered();
  f.current.top=121.5;f.current.left=2;
  f.paint();
  assert.equal(f.cache.inspect().cached,true);
  f.cache.scrolled();
  assert.equal(f.cache.inspect().cached,true,'deferred automatic anchor scroll retired the warmed snapshot');
  assert.equal(f.messages.filter(m=>m.op==='invalidate').length,1,'the only invalidation should be configure(true)');
  f.cache.native({token,revision:2,scale:1.3});
  assert.equal(f.cache.inspect().hits,2,'the next gesture should reuse the retained image');
});


test('successive pinches reuse stale pixels until one validated idle replacement arrives',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  let pending;
  for(let revision=1;revision<=20;revision++) {
    const scale=1+revision*.02;
    f.cache.native({token,revision,scale});
    assert.equal(f.cache.inspect().cached,true);
    if(pending) f.admit(pending);
    f.current.scale=scale;f.current.top=100+revision*2;
    f.cache.rendered();f.current.top+=.25;f.paint();f.cache.scrolled();
    assert.equal(f.cache.inspect().cached,true,'a refresh race discarded usable pixels');
    assert.equal(f.cache.inspect().active,false);
    f.tick();pending=f.captures().at(-1);
    assert.equal(f.cache.inspect().cached,true,'capture construction discarded usable pixels');
  }
  assert.equal(f.cache.inspect().hits,20);
  f.admit(pending);
  assert.equal(f.cache.inspect().cached,true);
  const replacement=pending.token;
  assert.notEqual(replacement,token);
  f.cache.native({token:replacement,revision:21,scale:1.5});
  assert.equal(f.cache.inspect().hits,21);
  f.cache.invalidate();assert.equal(f.cache.inspect().cached,false,'source invalidation must still flush the retained image');
});


test('geometry-only changes reject pending captures without retiring warmed pixels',()=>{
  const geometry=()=>({paper:[0,-100,880,2100],source_scale:1,pages:[[0,700],[710,700],[1420,700]]});
  for(const change of [value=>({...value,paper:[0,-100,880,2200]}),
      value=>({...value,source_scale:1.01}),
      value=>({...value,pages:value.pages.map(page=>[...page])})]) {
    const f=fixture();f.current.geometry=geometry();f.tick();f.admit();
    const token=f.captures()[0].token;
    f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.paint();f.tick();
    const pending=f.captures()[1];f.current.geometry=change(f.current.geometry);
    f.admit(pending);
    assert.equal(f.messages.at(-1).op,'reject','capture completed against a different page layout');
    assert.equal(f.cache.inspect().cached,true,'layout refresh retired still usable pixels');
    f.tick();assert.equal(f.captures().length,3);
  }
});

test('geometry-only changes schedule a replacement and stable layouts do no idle work',()=>{
  const f=fixture();
  const pages=[[0,700],[710,700],[1420,700]];
  f.current.geometry={paper:[0,-100,880,2100],source_scale:1,pages};
  f.tick();f.admit();
  f.current.geometry={paper:[0,-100,880,2100],source_scale:1,pages};
  for(let i=0;i<50;i++){f.cache.settle();f.tick();}
  assert.equal(f.captures().length,1,'equivalent metadata must not create idle capture work');
  f.current.geometry={...f.current.geometry,pages:pages.map(page=>[...page])};
  f.cache.settle();f.tick();
  assert.equal(f.captures().length,2,'page-layout replacement failed to refresh pixels');
  assert.equal(f.cache.inspect().cached,true,'preparing the replacement discarded warmed pixels');
});

test('a duplicate viewport resize cannot cancel the native flight sharp handoff', () => {
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.current.width=800;f.cache.resize();
  f.cache.native({token,revision:1,scale:1,resize:true});f.cache.rendered();
  // AppKit has already updated the WebView bounds. Its queued DOM resize event
  // reports those same bounds, so the native size check sends no new request.
  f.cache.resize();f.paint();
  assert.equal(f.messages.filter(m=>m.op==='ready').at(-1)?.revision,1);
  assert.equal(f.cache.inspect().active,false);
  assert.equal(f.cache.inspect().cached,true);
});


test('a delayed resize for the acknowledged viewport does not create an ownerless gesture', () => {
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.current.width=800;f.cache.resize();
  f.cache.native({token,revision:1,scale:1,resize:true});f.cache.rendered();f.paint();
  const messages=f.messages.length;
  for(let i=0;i<20;i++) assert.equal(f.cache.resize(),true);
  assert.equal(f.messages.length,messages,'same viewport must not request ignored native work');
  assert.equal(f.cache.inspect().active,false);
  f.tick();assert.equal(f.captures().length,2,'idle refresh must still run');
  f.current.width=700;assert.equal(f.cache.resize(),true);
  assert.equal(f.cache.inspect().active,false,'only a dispatched native request owns resizing');
});


test('a scroll notification without movement keeps the idle raster',()=>{
  const f=fixture();f.tick();f.admit();f.cache.scrolled();assert.equal(f.cache.inspect().cached,true);
  assert.equal(f.cache.inspect().active,false);
});
test('resize-induced scroll arriving before resize retains the native anchor and raster',()=>{
  const f=fixture();f.tick();f.admit();f.current.width=500;f.current.top=80;f.cache.scrolled();
  assert.equal(f.cache.inspect().cached,true);assert.equal(f.cache.inspect().active,false);
  assert.equal(f.messages.at(-1).op,'resize');
  const token=f.captures()[0].token;f.cache.native({token,revision:1,scale:1});f.cache.rendered();f.paint();
  assert.equal(f.cache.inspect().active,false);assert.equal(f.cache.inspect().cached,true);
  f.current.top=100;f.cache.scrolled();assert.equal(f.cache.inspect().cached,false);
});

test('WebKit rubber-band scroll origins are never admitted as idle captures',()=>{
  const f=fixture();f.current.geometry={paper:[0,2,900,2000],scroll:[0,-2],pages:[]};f.tick();
  assert.equal(f.captures().length,0);
  f.current.geometry={paper:[0,0,900,2000],scroll:[0,0],pages:[]};f.cache.rendered();f.tick();
  assert.equal(f.captures().length,1);
});

test('same-position scroll from another document still discards obsolete pixels',()=>{
  const f=fixture();f.tick();f.admit();f.current.doc={};f.current.width=500;f.cache.scrolled();
  assert.equal(f.cache.inspect().cached,false);assert.equal(f.cache.inspect().active,false);
});

test('a former document handoff cannot suppress scroll invalidation in a new document',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1});f.cache.rendered();f.paint();
  f.current.doc={};f.cache.scrolled();assert.equal(f.cache.inspect().cached,false);
});

test('the outline is separate from opaque image coverage and posts only when it changes',()=>{
  const f=fixture();f.cache.outline(2,[255,128,0,1]);
  const first=f.messages.filter(m=>m.op==='outline').length;
  for(let i=0;i<50;i++)f.cache.outline(2,[255,128,0,1]);
  assert.equal(f.messages.filter(m=>m.op==='outline').length,first);
  f.cache.outline(0,[0,0,0,0]);assert.equal(f.messages.at(-1).width,0);
  f.cache.configure(false);f.cache.outline(1,[1,2,3,1]);f.cache.configure(true);
  assert.equal(f.messages.filter(m=>m.op==='outline').at(-1).width,1);
});

test('cached pixels survive the first frame before the latest vector is presented',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();
  const callbacks=f.frames.splice(0);callbacks.forEach(fn=>fn());
  assert.equal(f.cache.inspect().active,true,'ready was sent before a completed paint opportunity');
  assert.equal(f.messages.filter(m=>m.op==='ready').length,0);
});

test('resize completion waits for live renderer readiness even when zoom scale is unchanged',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1});f.current.width=800;f.current.ready=false;
  f.cache.rendered();f.paint();assert.equal(f.cache.inspect().active,true);assert.equal(f.frames.length,0);
  f.current.fitWidth=780;f.current.ready=true;f.cache.rendered();f.paint();assert.equal(f.cache.inspect().active,false);
});
test('geometry movement between paint boundaries retains the cache until stable',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1.2});f.current.scale=1.2;f.cache.rendered();f.nextFrame();
  f.current.top+=20;f.nextFrame();assert.equal(f.cache.inspect().active,true);
  assert.equal(f.messages.filter(m=>m.op==='ready').length,0);assert.equal(f.frames.length,1);
  f.paint();assert.equal(f.cache.inspect().handoffs,1);assert.equal(f.frames.length,0);
});
test('new divider direction during the paint boundary cannot reveal the previous zoom',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:1,scale:1});f.current.fitWidth=780;f.cache.rendered();f.nextFrame();
  f.cache.native({token,revision:2,scale:1});f.nextFrame();
  assert.equal(f.messages.filter(m=>m.op==='ready').length,0);assert.equal(f.cache.inspect().active,true);
  f.current.fitWidth=900;f.cache.rendered();f.paint();assert.equal(f.messages.filter(m=>m.op==='ready').at(-1).revision,2);
});
test('capture uses the live opaque backdrop even if an oversized SVG falls back to viewport capture',()=>{
  for(const buffered of [false,true]){
    const f=fixture();f.window.tiptoptypSnapshotBackdrop=()=>[82,86,89,1];
    if(buffered)f.window.tiptoptypSnapshotBuffer=()=>({width:1800,height:2100,paperBackground:[82,86,89,1]});
    f.current.background=[24,31,42];f.tick();assert.deepEqual(Array.from(f.captures()[0].background),[82,86,89]);
  }
});

test('a second divider size and a source update cannot trap a ready serial flight at its former width',()=>{
  for (const sourceEdit of [false,true]) {
    const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
    f.cache.native({token,revision:1,scale:1,resize:true});
    f.current.width=750;f.current.fitWidth=730;
    if(sourceEdit)f.cache.invalidate();
    for(let i=0;i<100;i++){f.cache.rendered();f.paint();}
    assert.equal(f.messages.filter(m=>m.op==='ready').length,1,'old fitted width deadlocked the serial flight');
    assert.equal(f.cache.inspect().active,false);
  }
});
test('a resize notification alone cannot become an ownerless active zoom',()=>{
  const f=fixture();f.tick();f.admit();f.current.width=800;f.current.fitWidth=780;
  f.cache.resize();f.cache.rendered();f.paint();
  assert.equal(f.cache.inspect().active,false,'native may no-op an already applied resize');
});

test('a rejected native request retires its owner and publishes fresh readiness without restart',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:1,scale:1.2});
  f.current.scale=1.2;f.cache.rendered();f.nextFrame();
  assert.equal(f.cache.rejected(1),true);f.paint();
  assert.equal(f.cache.inspect().active,false);assert.equal(f.cache.inspect().cached,false);
  assert.equal(f.messages.filter(m=>m.op==='ready').length,0);assert.equal(f.messages.at(-1).op,'frame');
  f.tick();f.admit();assert.equal(f.cache.inspect().cached,true);
  f.cache.native({token:f.captures().at(-1).token,revision:2,scale:1.3});f.current.scale=1.3;f.cache.rendered();f.paint();
  assert.equal(f.cache.inspect().handoffs,1);
});
test('a late rejection cannot cancel a newer native owner',()=>{
  const f=fixture();f.tick();f.admit();const token=f.captures()[0].token;
  f.cache.native({token,revision:2,scale:1.3});assert.equal(f.cache.rejected(1),false);
  assert.equal(f.cache.inspect().active,true);f.current.scale=1.3;f.cache.rendered();f.paint();assert.equal(f.cache.inspect().handoffs,1);
});

test('native rejection never echoes an unversioned abort into a potentially newer native flight',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:1,scale:1.2});
  const before=f.messages.filter(m=>m.op==='invalidate').length;f.cache.rejected(1);
  assert.equal(f.messages.filter(m=>m.op==='invalidate').length,before);
});
test('abandon terminates only the matching serial owner and locally releases its stale image',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:2,scale:1.2});
  assert.equal(f.cache.abandon(1),false);assert.equal(f.cache.inspect().active,true);
  assert.equal(f.cache.abandon(2),true);assert.equal(f.cache.inspect().active,false);
  assert.deepEqual(f.messages.filter(m=>m.op==='abandon').map(m=>m.revision),[2]);
  assert.equal(f.messages.filter(m=>m.op==='invalidate').length,1,'abandon must use its versioned native terminal message');
});

for (const atPaintBoundary of [false,true]) test(`renderer replacement ${atPaintBoundary?'during':'before'} sharp painting terminates the old serial owner`,()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:4,scale:1.2});
  f.current.scale=1.2;f.cache.rendered();if(atPaintBoundary)f.nextFrame();
  f.current.doc={};f.current.scale=1;f.cache.invalidate();
  for(let i=0;i<100;i++){f.cache.rendered();f.paint();}
  assert.equal(f.cache.inspect().active,false);assert.deepEqual(f.messages.filter(m=>m.op==='abandon').map(m=>m.revision),[4]);
});

test('reset and disposal retire only the document owning the unacknowledged native flight',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:5,scale:1.2});
  assert.equal(f.cache.retireDocument({}),false);assert.equal(f.cache.inspect().active,true);
  assert.equal(f.cache.retireDocument(f.doc),true);assert.equal(f.cache.inspect().active,false);
  assert.deepEqual(f.messages.filter(m=>m.op==='abandon').map(m=>m.revision),[5]);
});

test('same-scale renderer replacement between paint callbacks cannot acknowledge an old document',()=>{
  const f=fixture();f.tick();f.admit();f.cache.native({token:f.captures()[0].token,revision:6,scale:1.2});
  f.current.scale=1.2;f.cache.rendered();f.nextFrame();f.current.doc={};f.nextFrame();
  assert.equal(f.cache.inspect().active,false);assert.equal(f.messages.filter(m=>m.op==='ready').length,0);
  assert.deepEqual(f.messages.filter(m=>m.op==='abandon').map(m=>m.revision),[6]);
});


test('pending and visible reveal beacons never enter the persistent zoom cache', () => {
  const f = fixture();
  let revealing = true;
  f.window.tiptoptypPreviewRevealActive = () => revealing;
  f.tick();
  assert.equal(f.captures().length, 0);
  for (let i = 0; i < 100; i++) f.cache.settle();
  assert.equal(f.timers.size, 0);
  revealing = false; f.cache.settle(); f.tick();
  const capture = f.captures().at(-1);
  revealing = true; f.admit(capture);
  assert.equal(f.cache.inspect().cached, false);
  assert.equal(f.timers.size, 0);
  revealing = false; f.cache.settle(); f.tick();
  const next = f.captures().at(-1);
  f.cache.resolve(next.token, {width: 1800, height: 1400});
  revealing = true; f.cache.admitted(next.token, true);
  assert.equal(f.cache.inspect().cached, false);
  revealing = false; f.cache.settle(); f.tick(); f.admit();
  assert.equal(f.cache.inspect().cached, true);
  assert.equal(f.frames.length, 0);
});


test('finite feedback defers capture timers without discarding warmed zoom pixels',()=>{
  const f=fixture(); f.tick(); f.admit(); const token=f.captures()[0].token;
  f.cache.deferCaptures(true); f.cache.deferCaptures(true);
  assert.equal(f.cache.inspect().capture_deferred,true);
  assert.equal(f.cache.inspect().cached,true);
  f.cache.native({token,revision:1,scale:1.2});
  f.current.scale=1.2; f.cache.rendered(); f.paint();
  assert.equal(f.cache.inspect().active,false,'sharp acknowledgement is not deferred');
  assert.equal(f.cache.inspect().cached,true);
  for(let i=0;i<20;i++){f.cache.settle();f.tick();}
  assert.equal(f.captures().length,1);
  assert.equal(f.timers.size,0);
  f.cache.deferCaptures(false); f.cache.deferCaptures(false);
  assert.equal(f.timers.size,1,'resume schedules one refresh');
  f.tick(); assert.equal(f.captures().length,2);
  assert.equal(f.cache.inspect().cached,true);
  f.admit(); assert.equal(f.cache.inspect().cached,true);
  assert.equal(f.timers.size,0);
});

test('deferral stops cold capture before buffer serialization and retries queued native requests once',()=>{
  const f=fixture(); let buffers=0;
  f.window.tiptoptypSnapshotBuffer=()=>{buffers++;return null;};
  f.cache.deferCaptures(true); f.tick();
  assert.equal(buffers,0); assert.equal(f.captures().length,0);
  for(let i=0;i<20;i++){f.cache.settle();f.tick();}
  assert.equal(f.timers.size,0);
  f.cache.deferCaptures(false); f.tick();
  const old=f.captures()[0]; assert.equal(buffers,1);
  // A timer can post IPC immediately before native deferral takes effect.
  f.cache.deferCaptures(true); f.cache.deferred(old.token);
  assert.equal(f.cache.inspect().capturing,false);
  assert.equal(f.cache.inspect().failures,0);
  f.cache.deferred('stale-token'); f.tick();
  assert.equal(f.captures().length,1);
  f.current.top=220; f.cache.scrolled();
  f.cache.deferCaptures(false); f.tick();
  assert.equal(f.captures().length,2);
  assert.equal(f.cache.inspect().capturing,true);
  assert.equal(f.captures()[1].token===old.token,false);
  f.admit(); assert.equal(f.cache.inspect().cached,true);
  assert.equal(f.timers.size,0);
});

test('held native admission settles once after feedback and invalidated results remain stale',()=>{
  const f=fixture(); f.tick(); const request=f.captures()[0];
  f.cache.deferCaptures(true);
  f.cache.resolve(request.token,{width:1000,height:1000});
  assert.equal(f.cache.inspect().admitting,true);
  f.cache.deferCaptures(false);
  assert.equal(f.timers.size,0,'held admission owns the capture slot');
  f.cache.admitted(request.token,true);
  assert.equal(f.cache.inspect().cached,true);
  assert.equal(f.timers.size,0);
  f.cache.deferCaptures(true); f.cache.invalidate(); f.cache.settle(); f.tick();
  assert.equal(f.cache.inspect().cached,false);
  assert.equal(f.captures().length,1);
  f.cache.admitted(request.token,true);
  assert.equal(f.cache.inspect().cached,false,'late admission cannot restore invalid pixels');
  f.cache.deferCaptures(false); f.tick();
  assert.equal(f.captures().length,2);
});

for (const blocked of ['feedback', 'capture', 'reveal']) test(`renderer readiness is independent of optional ${blocked} pixel warming`, () => {
  const f = fixture({ready:false});
  if (blocked === 'feedback') f.cache.deferCaptures(true);
  if (blocked === 'capture') {
    f.current.ready = true; f.cache.settle(); f.tick();
    assert.equal(f.cache.inspect().capturing, true);
    f.cache.invalidate(true);
  }
  if (blocked === 'reveal') f.window.tiptoptypPreviewRevealActive = () => true;
  const before = f.messages.filter(m => m.op === 'frame').length;
  f.current.ready = true;
  f.current.scale = 1.3;
  for (let i = 0; i < 100; i++) f.cache.settle();
  const frames = f.messages.filter(m => m.op === 'frame');
  assert.equal(frames.length, before + 1, 'native gestures must initialize without waiting for pixels');
  assert.equal(frames.at(-1).scale, 1.3);
  f.tick();
  assert.equal(f.captures().length, blocked === 'capture' ? 1 : 0);
  assert.equal(f.timers.size, 0);
});

test('failed render reports its applied scale and cannot cancel a newer revision or renderer',()=>{
  const f=fixture();f.tick();f.admit();
  const token=f.captures()[0].token;
  f.cache.native({revision:7,owner:19,scale:1.3,token});f.doc.currentScaleRatio=1.3;
  assert.equal(f.cache.failed(6,f.doc),false);
  assert.equal(f.cache.failDocument({}),false);
  assert.equal(f.cache.inspect().active,true);
  assert.equal(f.cache.failed(7,f.doc),true);
  const failures=f.messages.filter(m=>m.op==='render-failed');
  assert.equal(failures.length,1);assert.equal(failures[0].scale,1.3);assert.equal(failures[0].owner,19);
  f.cache.native({revision:8,scale:1.4,token:null});
  assert.equal(f.cache.failed(7,f.doc),false);
  assert.equal(f.cache.inspect().active,true);
  f.current.scale=1.4;f.cache.rendered();f.paint();
  assert.equal(f.cache.inspect().active,false);
  assert.equal(f.frames.length,0);
});

for (const blocked of ['feedback','capture','admission','timer']) test(`reported readiness avoids repeated DOM reads during ${blocked}`,()=>{
  const f=fixture();
  if(blocked==='feedback') f.cache.deferCaptures(true);
  if(blocked==='capture'||blocked==='admission') f.tick();
  if(blocked==='admission') f.cache.resolve(f.captures()[0].token,{width:1000,height:1000});
  const before=f.reads;
  for(let i=0;i<1000;i++)f.cache.settle();
  assert.equal(f.reads,before);
  assert.equal(f.messages.filter(m=>m.op==='frame').length,1);
});

// Handoff geometry is live SVG geometry; the old bitmap stays retained.
test('sharp handoff reports its actual position for the next retained-image gesture', () => {
  const f = fixture(); f.tick(); f.admit();
  const token = f.captures()[0].token;
  f.cache.native({token, revision: 1, scale: 1.35});
  f.current.scale = 1.35;
  f.current.geometry = {paper: [-185.25, -13810.5, 1200, 45000], source_scale: 1.123};
  f.cache.rendered(); f.paint();
  const ready = f.messages.find(m => m.op === 'ready');
  assert.deepEqual(JSON.parse(JSON.stringify(ready.position)),
    {x: -185.25, y: -13810.5, scale: 1.123, width: 900, height: 700});
  assert.equal(f.cache.inspect().cached, true);
});
