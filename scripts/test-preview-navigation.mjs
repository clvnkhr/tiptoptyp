import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/preview_navigation.js', import.meta.url), 'utf8');
const snapshotSource = readFileSync(new URL('../src/preview_zoom_snapshot.js', import.meta.url), 'utf8');
function fixture() {
  const handlers = new Map();
  let focused = true;
  const frames = [];
  let rescales = 0;
  let demands = 0;
  let clearedAnchors = 0;
  const messages = [];
  const activeClasses = new Set();
  const classes = { toggle(name, enabled) { enabled ? activeClasses.add(name) : activeClasses.delete(name); } };
  const styles = [];
  const properties = new Map();
  const element = () => ({ style: {}, children: [], attrs: {}, setAttribute(name,value) {this.attrs[name]=String(value);},
    getAttribute(name) {return this.attrs[name] ?? null;}, append(child) { this.children.push(child); }, get firstElementChild() { return this.children[0]; } });
  const scroll = {
    clientWidth: 800, clientHeight: 600, classList: classes,
    x: 0, y: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0, bottom: 600 }),
    get scrollTop() { return this.y; }, get scrollLeft() { return this.x; },
    scrollTo({top, left}) { if (top !== undefined) this.y = top; if (left !== undefined) this.x = left; },
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
    clearSvgResizeAnchor() { clearedAnchors++; },
    retrieveDOMState: () => ({ width: 800, height: 600 }),
    r: { rescale() { rescales++; }, rerender() { throw Error('viewport rerender'); } },
  };
  const container = { documents: [{ impl: doc }] };
  const body = element();
  const window = { addEventListener: (name, handler) => handlers.set(name, handler), ipc: { postMessage: value => messages.push(JSON.parse(value)) } };
  runInNewContext(source, {
    document: { hasFocus: () => focused, getElementById: () => container, readyState: 'loading',
      documentElement: {classList: classes, style: {setProperty: (key,value) => properties.set(key,value)}},
      body, head: {append: style => styles.push(style.textContent)},
      createElement: element, createElementNS: element,
      addEventListener: (name, handler) => handlers.set(name, handler) },
    MutationObserver: class { observe() {} },
    window,
    performance: {now: () => 0},
    requestAnimationFrame: callback => { frames.push(callback); return frames.length; },
  });
  return {
    setFocus(value) { focused = value; },
    styles, properties, activeClasses, body, palette: (...args) => window.tiptoptypSetPalette(...args),
    paletteScript: script => runInNewContext(script, {window}),
    doc, scroll, frames, container, messages, action: value => window.tiptoptypPreviewAction(value),
    revealSource: token => window.tiptoptypRevealSource(token),
    get rescales() { return rescales; }, get clearedAnchors() { return clearedAnchors; },
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

// Execute the JS embedded by native harnesses against the current adapter,
// rather than mirroring its parameter list in a separate test implementation.
function nativePaletteScripts(path) {
  return [...readFileSync(new URL(path, import.meta.url), 'utf8')
    .matchAll(/window\.tiptoptypSetPalette(?:\?\.)?\([^;\n]*?\);/g)]
    .map(match => match[0]);
}

function assertHarnessPalette(script, paper, ink) {
  const f = fixture();
  f.event('DOMContentLoaded');
  assert.doesNotThrow(() => f.paletteScript(script), script);
  assert.equal(f.properties.get('--tiptoptyp-paper'), `rgb(${paper.join(',')})`);
  assert.equal(f.properties.get('--tiptoptyp-ink'), `rgb(${ink.join(',')})`);
  assert.equal(f.properties.get('--tiptoptyp-hover-border'), '1px solid transparent');
  assert.equal(f.properties.get('--tiptoptyp-focus-border'), '1px solid transparent');
  const accent = f.properties.get('--tiptoptyp-reveal-accent');
  assert.match(accent, /^rgb\(\d+,\d+,\d+\)$/);
  assert.ok(accent.slice(4, -1).split(',').every(channel => Number(channel) <= 255));
}

test('native palette harness creation and reload scripts execute the production palette contract', () => {
  const calls = nativePaletteScripts('./test-preview-palette.swift');
  assert.equal(calls.length, 2, 'cover both creation-time and current-palette replay');
  assertHarnessPalette(calls[0], [255,255,255], [0,0,0]);
  assertHarnessPalette(calls[1]
    .replaceAll(String.raw`\(bg)`, '[24,31,42]')
    .replaceAll(String.raw`\(fg)`, '[200,210,220]'), [24,31,42], [200,210,220]);
});

test('native zoom harness dark-ready script executes the production palette contract', () => {
  const calls = nativePaletteScripts('../tests/native_preview_zoom.rs');
  assert.equal(calls.length, 1, 'cover the ready callback before it reports loaded');
  assertHarnessPalette(calls[0], [40,40,40], [235,219,178]);
});

test('resize bursts rescale once per frame, never rerender or keep idle frames alive', () => {
  const f = fixture();
  for (let i = 0; i < 1000; i++) assert.ok(f.event('resize').stopped);
  assert.equal(f.frames.length, 1);
  f.flush();
  assert.equal(f.rescales, 1);
  assert.equal(f.frames.length, 0);
});

test('page indicator follows the dominant visible page and explicit navigation clears link anchors', () => {
  const f = fixture();
  f.doc.hookedElem.firstElementChild.children = [0, 1, 2].map(index => ({
    tagName: 'g',
    getAttribute: name => name === 'data-page-width' ? '600' : '890',
    getScreenCTM: () => ({a:1,d:1,e:0,f:index * 900 - f.scroll.y}),
    getBoundingClientRect: () => { throw Error('Ink bounds are not page bounds'); },
  }));
  f.scroll.y = 800; // Tail of page one, most of page two.
  f.event('scroll', {target:f.scroll});
  assert.equal(f.messages.at(-1).page, 1);
  f.action({action:'page', value:2});
  assert.equal(f.clearedAnchors, 1);
  assert.equal(f.scroll.y, 1800);
  assert.equal(f.messages.at(-1).page, 2);
  f.action({action:'back'});
  assert.equal(f.scroll.y, 800);
  assert.equal(f.messages.at(-1).page, 1);
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

test('preview hotkeys require keyboard focus, and Escape returns to the editor', () => {
  const f = fixture();
  f.setFocus(false);
  assert.equal(f.event('keydown', {key:'t'}).prevented, false);
  assert.equal(f.event('keydown', {key:'Escape'}).prevented, false);
  assert.equal(f.messages.length, 0);
  f.setFocus(true);
  f.event('focus');
  assert.equal(f.messages.at(-1).focused, true);
  assert.equal(f.event('keydown', {key:'Escape'}).prevented, true);
  assert.equal(f.messages.at(-1).type, 'focus-editor');
  f.setFocus(false);
  f.event('blur');
  assert.equal(f.messages.at(-1).focused, false);
  assert.equal(f.frames.length, 0);
});

test('an initially unchanged native palette is applied when the preview DOM mounts', () => {
  const f=fixture();
  f.palette([255,255,255],[0,0,0],'1px solid transparent','1px solid transparent',[0,0,0]);
  assert.equal(f.properties.size,0);
  f.event('DOMContentLoaded');
  assert.equal(f.properties.get('--tiptoptyp-paper'),'rgb(255,255,255)');
  assert.equal(f.properties.get('--tiptoptyp-focus-border'),'1px solid transparent');
  assert.equal(f.properties.get('--tiptoptyp-reveal-accent'),'rgb(0,0,0)');
  assert.equal(f.properties.get('--tiptoptyp-reveal-wash'),'rgba(0,0,0,0.2)');
  assert.equal(f.frames.length,0);
});

test('preview focus uses a theme border, never a badge, with no layout or pointer interception', () => {
  const f = fixture();
  f.palette([255,255,255], [0,0,0], '1px solid rgba(1,2,3,1)', '2px solid rgba(4,5,6,1)', [79,140,255]);
  f.event('DOMContentLoaded');
  assert.equal(f.properties.get('--tiptoptyp-hover-border'), '1px solid rgba(1,2,3,1)');
  assert.equal(f.properties.get('--tiptoptyp-focus-border'), '2px solid rgba(4,5,6,1)', [79,140,255]);
  assert.match(f.styles[0], /html:hover::before/);
  assert.match(f.styles[0], /position: fixed/);
  assert.match(f.styles[0], /pointer-events: none/);
  assert.doesNotMatch(f.styles[0], /Preview focused|html[^{}]*::after/);
  assert.ok(f.activeClasses.has('tiptoptyp-preview-focused'));
  f.setFocus(false); f.event('blur');
  assert.ok(!f.activeClasses.has('tiptoptyp-preview-focused'));
  f.setFocus(true); f.event('focus');
  assert.ok(f.activeClasses.has('tiptoptyp-preview-focused'));
  assert.equal(f.frames.length, 0);
});

// Model the pinned SVG frontend's synchronous patcher and asynchronous queue.
// The first document is pushed before the socket enables partial rendering.
function partialFixture({ pages = Array.from({length:48}, () => ({width:600, height:300})), startup = true, partial = true, lateContainer = false, initializeLinks = null, decorated = false, configure = null, zoomSnapshot = null, layoutHook = null, nativeSnapshot = false } = {}) {
  const handlers = new Map(), frames = new Map(), windows = [], merges = [], errors = [];
  let nextFrame = 1, now = 0, canvasPages = 0, postRenders = 0, layouts = 0, reads = 0, timeouts = 0, rectReads = 0, domReads = 0, cursorSearches = 0;
  const classes = {toggle() {}, contains: name => name === 'typst-page'};
  const scroll = {clientWidth:600, clientHeight:300, scrollTop:0, scrollLeft:0, classList:classes,
    getBoundingClientRect() { rectReads++; return {left:23, top:37, bottom:37+this.clientHeight}; },
    scrollTo({top,left}) { if (top !== undefined) this.scrollTop=top; if(left !== undefined)this.scrollLeft=left; },
    scrollBy(x,y) { this.scrollLeft+=x; this.scrollTop+=y; }};
  let currentPages = startup ? [] : pages;
  const doc = {moduleInitialized:!startup, renderMode:'svg', previewMode:0, partialRendering:false,
    currentScaleRatio:1, isRendering:false, patchQueue:[], disposeList:[], sampledRenderTime:0,
    windowElem:scroll, hookedElem:{firstElementChild:null,parentElement:scroll,classList:classes,style:{}},
    retrieveDOMState() { domReads++; return {width:scroll.clientWidth,height:scroll.clientHeight,boundingRect:scroll.getBoundingClientRect()}; },
    clearSvgResizeAnchor() {}, shouldMixinCanvas() { return true; },
    reset() { currentPages=[]; this.moduleInitialized=false; },
    processQueue([kind,payload]) {
      if(kind==='new')this.reset();
      if(kind==='new'||kind==='diff-v1') { merges.push(kind); currentPages=payload; this.moduleInitialized=true; return true; }
      return kind==='viewport-change' && this.moduleInitialized;
    },
    postprocessChanges() { postRenders++; this.r.rescale(); },
    triggerUpdate() {
      if(this.isRendering)return;
      this.isRendering=true;
      requestAnimationFrame(() => {
        this.cachedDOMState=this.retrieveDOMState();
        let render=false;
        while(this.patchQueue.length)render=this.processQueue(this.patchQueue.shift())||render;
        if(render) { this.r.rescale(); this.r.rerender(); this.r.rescale(); }
        requestAnimationFrame(() => {this.isRendering=false;this.postprocessChanges();});
      });
    },
    addChangement(change) {
      if(change[0]==='new')this.patchQueue.length=0;
      if(change[0]==='viewport-change'&&this.isRendering) { timeouts++; return; }
      this.patchQueue.push(change);this.triggerUpdate();
    },
    addViewportChange() { this.addChangement(['viewport-change','']); },
    fetchSvgDataByDocMode() {
      return this.kModule.render_in_window(0,0,1e33,1e33);
    },
  };
  doc.cachedDOMState=doc.retrieveDOMState();
  doc.windowElem.contains=()=>true;
  doc.kModule={retrievePagesInfo() {reads++;return currentPages;},
    render_in_window(...bounds) { windows.push(bounds); return bounds; }};
  let scalePages=null,scaleWidth=1;
  const scale=()=>{if(scalePages!==currentPages){scalePages=currentPages;scaleWidth=Math.max(1,...currentPages.map(page=>Math.ceil(page.width)));}return scroll.clientWidth/scaleWidth*doc.currentScaleRatio;};
  const svgNode = (tagName, attrs = {}, className = '') => ({tagName,attrs:{...attrs},children:[],parentElement:null,
    classList:{contains:name=>className.split(' ').includes(name)},
    getAttribute(name){return this.attrs[name]===undefined?null:String(this.attrs[name]);},
    setAttribute(name,value){this.attrs[name]=String(value);},
    querySelectorAll(selector){if(tagName==='svg'&&selector==='.typst-svg-cursor')cursorSearches++;return this.children.flatMap(child=>[
      ...(selector==='.typst-svg-cursor'&&child.classList.contains('typst-svg-cursor')?[child]:[]),...child.querySelectorAll(selector)]);}});
  const patch = bounds => {
    let raw=0,moduleY=0;
    const svg=svgNode('svg',{'data-width':Math.max(1,...currentPages.map(page=>Math.ceil(page.width)))});
    svg.getBoundingClientRect=()=>{rectReads++;return {left:23-scroll.scrollLeft,top:37-scroll.scrollTop};};
    const append=node=>{node.parentElement=svg;svg.children.push(node);};
    currentPages.forEach((sourcePage,index)=>{
      const page={width:Math.ceil(sourcePage.width),height:Math.ceil(sourcePage.height)};
      const top=raw+10/scale()*index, x=(Number(svg.getAttribute('data-width'))-page.width)/2;
      const dummy=moduleY+sourcePage.height<=bounds[1]||moduleY>=bounds[3], transform=`translate(${x}, ${top})`;
      const node=svgNode('g',{'data-page-width':page.width,'data-page-height':page.height,'data-page-number':index,
        'data-x':x,'data-y':top,'data-dummy':dummy?'1':'0',transform},'typst-page');
      node.getScreenCTM=()=>({a:scale(),b:0,c:0,d:scale(),e:23-scroll.scrollLeft+x*scale(),f:37-scroll.scrollTop+Number(node.getAttribute('data-y'))*scale()});
      append(node);
      if(decorated) {
        append(svgNode('rect',{'data-page-number':index,'data-page-width':page.width,'data-page-height':page.height,
          width:page.width*100,height:page.height*100,transform:`${transform} scale(0.01)`},'typst-page-inner'));
        if(doc.cursorPosition?.[0]===index+1)append(svgNode('circle',{cx:doc.cursorPosition[1]*100,cy:doc.cursorPosition[2]*100,
          r:500/scale(),fill:'#86C166CC',transform:`${transform} scale(0.01)`}));
      }
      if(dummy&&doc.shouldMixinCanvas())canvasPages++;
      raw+=page.height;moduleY+=sourcePage.height;
    });
    const height=raw+Math.max(0,currentPages.length-1)*10/scale();
    svg.setAttribute('data-height',height);svg.setAttribute('viewBox',`0 0 ${svg.getAttribute('data-width')} ${height}`);
    if(decorated)append(svgNode('rect',{'data-page-width':svg.getAttribute('data-width'),'data-page-height':Math.ceil(height),
      width:svg.getAttribute('data-width'),height:Math.ceil(height)},'typst-page-outer'));
    doc.hookedElem.firstElementChild=svg;
  };
  doc.r={rescale() {layouts++;layoutHook?.(doc);}, rerender() { const bounds=doc.fetchSvgDataByDocMode();patch(bounds); },postRender() {postRenders++;}};
  const container={documents:[],initTypstSvg:initializeLinks,contains:()=>true};
  doc.windowElem=container;
  configure?.(doc);
  let availableContainer=lateContainer?null:container, observerDisconnects=0;
  const observers=[];
  const requestAnimationFrame=callback=>{const id=nextFrame++;frames.set(id,callback);return id;};
  const cancelAnimationFrame=id=>frames.delete(id);
  const properties=new Map();let propertyWrites=0;
  const element=()=>({style:{setProperty(name,value) {propertyWrites++;properties.set(name,value);}},children:[],attrs:{},setAttribute(name,value){this.attrs[name]=String(value);},getAttribute(name){return this.attrs[name]??null;},append(child){child.parentElement=this;this.children.push(child);},
    remove(){if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(child=>child!==this);},
    get firstElementChild(){return this.children[0];}});
  const body=element(),head=element();
  const messages=[];
  const window={innerWidth:646,innerHeight:374,addEventListener:(name,handler)=>handlers.set(name,handler),ipc:{postMessage:value=>messages.push(JSON.parse(value))},tiptoptypZoomSnapshot:zoomSnapshot};
  const sandbox={window,
    document:{getElementById:()=>availableContainer,readyState:'loading',hasFocus:()=>true,documentElement:{...element(),classList:classes},body,head,
      createElement:element,createElementNS:element,addEventListener:(name,handler)=>handlers.set(name,handler)},
    MutationObserver:class {constructor(callback){observers.push(callback);}observe(){}disconnect(){observerDisconnects++;}},
    requestAnimationFrame,cancelAnimationFrame,performance:{now:()=>now},
    setTimeout(){timeouts++;return timeouts;},clearTimeout(){},console:{error:e=>errors.push(e),log(){}}};
  if(nativeSnapshot){
    window.tiptoptypCaptureZoomSnapshot=message=>messages.push({type:'preview-zoom-snapshot',...message});
    runInNewContext(snapshotSource,sandbox);
  }
  runInNewContext(source,sandbox);
  if(lateContainer) {availableContainer=container;observers.forEach(callback=>callback());}
  container.documents=[]; // Assignment in the frontend's module initialization.
  container.documents.push({impl:doc,setPartialRendering(value){doc.partialRendering=value;}});
  doc.partialRendering=partial;
  if(!startup)patch([0,0,600,600]);
  return {doc,scroll,windows,merges,errors,frames,container,messages,body,head,properties,window,
    palette:(...args)=>window.tiptoptypSetPalette(...args),get propertyWrites(){return propertyWrites;},
    advance(ms){now+=ms;},
    revealOptions:options=>window.tiptoptypSetRevealOptions(options),
    revealIntent:explicit=>window.tiptoptypPreviewRevealIntent(explicit),
    revealActive:()=>window.tiptoptypPreviewRevealActive(),
    revealState:()=>JSON.parse(JSON.stringify(window.tiptoptypPreviewRevealState())),
    revealSource:token=>window.tiptoptypRevealSource(token),
    resetGeometryReads(){rectReads=0;domReads=0;},get rectReads(){return rectReads;},get domReads(){return domReads;},
    get cursorSearches(){return cursorSearches;},get observerDisconnects(){return observerDisconnects;},get canvasPages(){return canvasPages;},get layouts(){return layouts;},get reads(){return reads;},get timeouts(){return timeouts;},get postRenders(){return postRenders;},
    start(){doc.addChangement(['new',pages]);},
    flush(){for(const [id,callback]of[...frames]){if(frames.delete(id))callback();}},
    scrollTo(top){scroll.scrollTop=top;this.event('scroll',{target:scroll});},
    action(value){window.tiptoptypPreviewAction(value);},
    nativeZoom(value){return window.tiptoptypNativeZoom(value);},
    event(name,data={}){handlers.get(name)?.({preventDefault(){},stopImmediatePropagation(){},...data});},
    visibleComplete(){const svg=doc.hookedElem.firstElementChild;return !!svg&&svg.children.filter(page=>page.classList.contains('typst-page')).every(page=>{const m=page.getScreenCTM();const bottom=m.f+Number(page.getAttribute('data-page-height'))*m.d;return bottom<=37||m.f>=37+scroll.clientHeight||page.getAttribute('data-dummy')!=='1';});},
  };
}


// The real SVG frontend can retain a page/local viewport-top anchor for 600ms.
// Run that competing anchor inside the cached layout delegate, so repeated
// no-op rescale calls cannot fabricate a jump which the production cache skips.
function nativeResizeFixture(page, location) {
  let semantic = null;
  const snapshot = { attach() {}, native: () => true, begin() {}, rendered() {}, invalidate() {}, settle() {}, scrolled() {} };
  const f = partialFixture({ decorated: true, pages: Array.from({length: 512}, () => ({width: 600, height: 300})), zoomSnapshot: snapshot,
    layoutHook(doc) {
      if (!semantic) return;
      const papers = doc.hookedElem.firstElementChild.children.filter(node => node.classList.contains('typst-page'));
      f.scroll.scrollTop = (Number(papers[semantic.page].getAttribute('data-y')) + semantic.local) * f.scroll.clientWidth / 600;
    } });
  f.scroll.clientHeight = 200;
  f.start(); f.flush(); f.scrollTo(page * 310 + (location === 'page' ? 200 : 305));
  semantic = {page, local: 100};
  f.doc.clearSvgResizeAnchor = () => { semantic = null; };
  const sourcePoint = location === 'page' ? {page, local: 200} : {before: page, after: page + 1, fraction: .5};
  return { f, resize(width, revision = 1) {
    f.scroll.clientWidth = width;
    assert.equal(f.nativeZoom({revision, scale: 1, resize: true,
      source: {x: 0, y: sourcePoint, clientX: 23, clientY: 37}}), true);
    f.flush();
  }, check(width) {
    const papers = f.doc.hookedElem.firstElementChild.children.filter(node => node.classList.contains('typst-page'));
    const y = f.scroll.scrollTop / (width / 600), start = Number(papers[page].getAttribute('data-y'));
    const local = y - start;
    if (location === 'page') assert.ok(Math.abs(local - 200) < 1e-8, `page ${page} at width ${width}: native local 200 became ${local}`);
    else {
      const gap = Number(papers[page + 1].getAttribute('data-y')) - start - 300, fraction = (local - 300) / gap;
      assert.ok(Math.abs(fraction - .5) < 1e-8, `page ${page} at width ${width}: native gap midpoint became ${fraction}`);
    }
    assert.ok(f.visibleComplete(), 'sharp frame must fill the same visible destination');
    assert.equal(f.frames.size, 0, 'anchor correction must not create an idle frame loop');
    assert.deepEqual(f.errors, []);
  } };
}

test('native divider handoff keeps a scrolled source point when covered geometry is reused', () => {
  for (const location of ['page', 'gap']) for (const width of [594, 900]) {
    const scene = nativeResizeFixture(18, location), renders = scene.f.windows.length;
    scene.resize(width); scene.check(width);
    assert.ok(scene.f.windows.length <= renders + 1, 'nearby coverage needs no repair patch');
  }
});

for (const location of ['page', 'gap']) test(`native divider handoff retains its ${location} source after a far-page repair patch`, () => {
  for (const width of location === 'page' ? [300] : [300, 900]) {
    const scene = nativeResizeFixture(180, location), renders = scene.f.windows.length;
    scene.resize(width); scene.check(width);
    assert.ok(scene.f.windows.length <= renders + 2, 'at most one destination repair');
  }
});

test('repeated divider handoffs retain a stale raster source across changing widths', () => {
  for (const location of ['page', 'gap']) {
    const scene = nativeResizeFixture(180, location);
    for (const [index, width] of [594, 300, 900, 600, 300, 600].entries()) {
      scene.resize(width, index + 1); scene.check(width);
    }
  }
});

test('partial startup renders a bounded whole-page buffer before first SVG without canvas fallbacks',()=>{
  const f=partialFixture();f.start();f.flush();
  assert.deepEqual(f.windows,[[0,0,600,600]]);
  assert.equal(f.canvasPages,0);
  assert.ok(f.visibleComplete());
  assert.equal(f.frames.size,0);
});

test('partial geometry accounts for unequal pages, ten CSS pixel gaps, viewport origin and zoom',()=>{
  const f=partialFixture({pages:[{width:600,height:100},{width:400,height:200},{width:600,height:300},{width:500,height:400},{width:600,height:500}]});
  f.start();f.flush();
  f.doc.currentScaleRatio=2;
  f.scroll.clientHeight=100;
  f.scrollTo(500);f.flush();
  assert.deepEqual(f.windows.at(-1),[0,100,600,600]);
  assert.ok(f.visibleComplete());
});

test('far partial scroll fills destination before the first paint and drains queued source patches first',()=>{
  const f=partialFixture();f.start();f.flush();f.flush();
  f.doc.addChangement(['diff-v1',Array.from({length:60},()=>({width:800,height:300}))]);
  f.scrollTo(12000);
  assert.ok(f.visibleComplete());
  assert.deepEqual(f.merges,['new','diff-v1']);
  assert.equal(f.windows.at(-1)[2],800);
  f.flush();assert.equal(f.frames.size,0);
});

test('partial scroll within buffer avoids renders, distant bursts remain bounded and idle stays quiet',()=>{
  const f=partialFixture();f.start();f.flush();f.flush();
  const initial=f.windows.length;
  for(let i=0;i<100;i++)f.scrollTo(i/10);
  f.flush();assert.equal(f.windows.length,initial);
  for(let i=0;i<100;i++) {f.scrollTo(7000+i);assert.ok(f.visibleComplete());}
  f.flush();assert.ok(f.windows.length<=initial+2);
  assert.equal(f.timeouts,0);assert.equal(f.frames.size,0);
  const reads=f.reads;f.flush();assert.equal(f.reads,reads);
});

test('partial navigation and zoom refill in their own frame without a second blank frame',()=>{
  const f=partialFixture();f.start();f.flush();f.flush();
  f.action({action:'page',value:35});
  assert.ok(f.visibleComplete());
  f.action({action:'zoom-in'});f.flush();
  assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  f.action({action:'back'});assert.ok(f.visibleComplete());
});

test('partial source reset replaces queued patches and disconnected renderers discard work',()=>{
  const f=partialFixture();f.start();f.flush();f.flush();
  f.doc.addChangement(['diff-v1',[{width:100,height:100}]]);
  f.doc.addChangement(['new',[{width:600,height:200},{width:600,height:200}]]);
  f.flush();assert.deepEqual(f.merges,['new','new']);assert.equal(f.windows.at(-1)[3],400);
  const count=f.windows.length;
  f.doc.addChangement(['diff-v1',[{width:600,height:500}]]);
  f.container.documents=[];f.flush();assert.equal(f.windows.length,count);assert.equal(f.frames.size,0);
});

test('full renderer delegates its source queue, full render window and canvas policy unchanged',()=>{
  const f=partialFixture({partial:false});f.start();
  assert.equal(f.frames.size,1);f.flush();
  assert.deepEqual(f.windows,[[0,0,1e33,1e33]]);
  assert.equal(f.doc.shouldMixinCanvas(),true);
  assert.equal(f.frames.size,1); // The existing frontend's bookkeeping frame.
  f.flush();assert.equal(f.frames.size,0);
  const count=f.windows.length;f.scrollTo(9000);f.flush();assert.equal(f.windows.length,count);
  f.doc.addChangement(['diff-v1',[{width:600,height:100}]]);f.flush();
  assert.deepEqual(f.merges,['new','diff-v1']);assert.deepEqual(f.windows.at(-1),[0,0,1e33,1e33]);
});

test('document-start observation hooks the parser-created container and disconnects after installation',()=>{
  const f=partialFixture({lateContainer:true});f.start();f.flush();
  assert.deepEqual(f.windows,[[0,0,600,600]]);
  assert.equal(f.observerDisconnects,1);assert.equal(f.canvasPages,0);assert.equal(f.frames.size,0);
});

test('continuous partial scroll never exposes dummy ink and keeps metadata work bounded',()=>{
  const f=partialFixture({pages:Array.from({length:2000},()=>({width:600,height:300}))});f.start();f.flush();
  for(let top=0;top<70000;top+=35) {
    f.scrollTo(top);assert.ok(f.visibleComplete(),`unrendered viewport at ${top}`);
    assert.ok(f.frames.size<=1);f.flush();assert.equal(f.frames.size,0);
    assert.ok(f.windows.at(-1)[3]-f.windows.at(-1)[1]<=1200);
  }
  assert.equal(f.reads,1);assert.equal(f.timeouts,0);
});

test('partial resize and horizontal zoom demand refill with full paper width in the input frame',()=>{
  const f=partialFixture();f.start();f.flush();f.scrollTo(6000);
  f.scroll.clientWidth=200;f.scroll.scrollLeft=100;
  f.event('resize');assert.equal(f.frames.size,1);f.flush();
  assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  assert.equal(f.windows.at(-1)[0],0);assert.equal(f.windows.at(-1)[2],600);
  assert.ok(f.windows.at(-1)[3]-f.windows.at(-1)[1]<=3300);
});

test('partial upstream viewport messages coalesce without timeouts or queue growth',()=>{
  const f=partialFixture();f.start();f.flush();const count=f.windows.length;
  for(let i=0;i<1000;i++)f.doc.addViewportChange();
  assert.equal(f.frames.size,1);assert.equal(f.doc.patchQueue.length,0);assert.equal(f.timeouts,0);
  f.flush();assert.equal(f.windows.length,count+1);assert.equal(f.doc.isRendering,false);assert.equal(f.frames.size,0);
});

test('partial failed patches do not acknowledge coverage or spin while idle; the next demand can recover',()=>{
  const f=partialFixture();f.start();f.flush();
  const render=f.doc.r.rerender;
  f.doc.r.rerender=()=>{throw Error('fixture patch failure');};
  f.scrollTo(12000);
  assert.equal(f.errors.length,1);assert.equal(f.doc.isRendering,false);assert.equal(f.frames.size,0);
  assert.equal(f.visibleComplete(),false);
  f.doc.r.rerender=render;f.scrollTo(12000);
  assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
});

test('partial disposal cancels queued rendering without touching the disconnected document',()=>{
  const f=partialFixture();f.start();f.flush();const count=f.windows.length;
  f.doc.addChangement(['diff-v1',[{width:600,height:100}]]);
  f.doc.disposeList.forEach(dispose=>dispose());
  assert.equal(f.frames.size,0);f.flush();assert.equal(f.windows.length,count);
});


test('full and partial identity palettes bypass filtering while tinted and inverted pages retain the exact transfer', () => {
  for (const partial of [false,true]) {
    const f = fixture();f.doc.partialRendering=partial;f.event('DOMContentLoaded');
    assert.equal(f.properties.get('--tiptoptyp-preview-filter'),'none');
    assert.match(f.styles[0], /#typst-container g\[data-page-width\] \{ filter: var\(--tiptoptyp-preview-filter, url\(#tiptoptyp-palette\)\); \}/);
    assert.doesNotMatch(f.styles[0], /tiptoptyp-partial-filter/);
    const channels=f.body.children[0].children[0].children[0].children;
    const bg=[24,31,42],fg=[200,210,220];f.palette(bg,fg,'1px solid transparent','1px solid transparent',[79,140,255]);
    assert.equal(f.properties.get('--tiptoptyp-preview-filter'),'url(#tiptoptyp-palette)');
    channels.forEach((channel,index)=>{
      assert.equal(channel.getAttribute('slope'),String((bg[index]-fg[index])/255));
      assert.equal(channel.getAttribute('intercept'),String(fg[index]/255));
    });
    f.palette([0,0,0],[255,255,255],'1px solid transparent','1px solid transparent',[79,140,255]);
    assert.equal(f.properties.get('--tiptoptyp-preview-filter'),'url(#tiptoptyp-palette)');
    channels.forEach(channel=>{assert.equal(channel.getAttribute('slope'),'-1');assert.equal(channel.getAttribute('intercept'),'1');});
    for(const [bg,fg]of[[[254,255,255],[0,0,0]],[[255,255,255],[0,0,1]]]) {
      f.palette(bg,fg,'1px solid transparent','1px solid transparent',[79,140,255]);assert.equal(f.properties.get('--tiptoptyp-preview-filter'),'url(#tiptoptyp-palette)');
    }
    f.palette([255,255,255],[0,0,0],'1px solid transparent','1px solid transparent',[79,140,255]);assert.equal(f.properties.get('--tiptoptyp-preview-filter'),'none');
    channels.forEach(channel=>{assert.equal(channel.getAttribute('slope'),'1');assert.equal(channel.getAttribute('intercept'),'0');});
    assert.equal(f.doc.partialRendering,partial);assert.equal(f.frames.length,0);
  }
});

test('partial scroll reuses one viewport snapshot for controls and both coverage checks',()=>{
  const f=partialFixture();f.start();f.flush();const renders=f.windows.length;
  f.resetGeometryReads();
  for(let i=0;i<100;i++)f.scrollTo(i/10);
  assert.equal(f.windows.length,renders);assert.equal(f.frames.size,0);
  assert.equal(f.domReads,100);
  assert.ok(f.rectReads<=200,`${f.rectReads} rectangles for 100 buffered scroll events`);
});

test('partial synchronous refill refreshes geometry after mutations without rereading it for each coverage check',()=>{
  const f=partialFixture();f.start();f.flush();
  f.doc.addChangement(['diff-v1',Array.from({length:60},()=>({width:800,height:300}))]);
  f.resetGeometryReads();f.scrollTo(12000);
  assert.ok(f.visibleComplete());assert.deepEqual(f.merges,['new','diff-v1']);
  assert.equal(f.windows.at(-1)[2],800);
  assert.ok(f.domReads<=3,`${f.domReads} DOM state reads for one refill`);
  assert.ok(f.rectReads<=6,`${f.rectReads} rectangle reads for one refill`);
  assert.equal(f.frames.size,0);
});


// The pinned initializer creates new handler functions on each invocation;
// retained SVG links must not collect those handlers on every viewport patch.
function linkInitializationFixture(options = {}) {
  const roots = [], receivers = [], argumentsReceived = [];
  const token = {};
  const initializeLinks = function(root, value) {
    roots.push(root); receivers.push(this); argumentsReceived.push(value);
    for (const link of root.getElementsByClassName('pseudo-link')) {
      link.addEventListener('mousemove', function mouseMoveToLink() {});
      link.addEventListener('mouseleave', function mouseLeaveFromLink() {});
    }
    return token;
  };
  const f = partialFixture({...options,initializeLinks}); f.start(); f.flush();
  const links = [];
  const root = f.doc.hookedElem.firstElementChild;
  root.getElementsByClassName = name => name === 'pseudo-link' ? links : [];
  const newLink = () => {
    const listeners = new Map();
    return {listeners,addEventListener(name,handler) {
      if (!listeners.has(name)) listeners.set(name,new Set());
      listeners.get(name).add(handler);
    }};
  };
  return {...f,roots,receivers,argumentsReceived,root,links,newLink,token};
}

test('full and partial viewport patches initialize each retained link once and initialize new links normally',()=>{
  for(const partial of [false,true]) {
  const f=linkInitializationFixture({partial}), first=f.newLink(), second=f.newLink();
  f.links.push(first);
  const argument={};
  assert.equal(f.doc.windowElem.initTypstSvg(f.root,argument),f.token);
  for(let i=0;i<100;i++)f.doc.windowElem.initTypstSvg(f.root,argument);
  assert.equal(first.listeners.get('mousemove').size,1);
  assert.equal(first.listeners.get('mouseleave').size,1);
  assert.equal(f.roots.length,1);
  assert.equal(f.receivers[0],f.doc.windowElem);
  assert.equal(f.argumentsReceived[0],argument);
  f.links.push(second);
  f.doc.windowElem.initTypstSvg(f.root,argument);
  assert.equal(first.listeners.get('mousemove').size,1);
  assert.equal(second.listeners.get('mousemove').size,1);
  assert.equal(second.listeners.get('mouseleave').size,1);
  assert.equal(f.roots.length,2);
  const replacement=f.newLink();
  f.links.splice(0,1,replacement);
  f.doc.windowElem.initTypstSvg(f.root,argument);
  assert.equal(replacement.listeners.get('mousemove').size,1);
  assert.equal(second.listeners.get('mousemove').size,1);
  }
});

test('link initialization delegates slide, content, canvas and unrelated roots without filtering',()=>{
  for(const mode of ['slide','content','canvas','unrelated','disconnected']) {
    const f=linkInitializationFixture(), link=f.newLink();f.links.push(link);
    if(mode==='canvas')f.doc.renderMode='canvas';
    if(mode==='slide')f.doc.previewMode=1;
    if(mode==='content')f.doc.isContentPreview=true;
    if(mode==='disconnected')f.container.documents=[];
    const root=mode==='unrelated'?{getElementsByClassName:()=>f.links}:f.root;
    const receiver={},argument={};
    for(let i=0;i<2;i++)assert.equal(f.doc.windowElem.initTypstSvg.call(receiver,root,argument),f.token);
    assert.equal(link.listeners.get('mousemove').size,2,mode);
    assert.equal(link.listeners.get('mouseleave').size,2,mode);
    assert.equal(f.roots[0],root,mode);
    assert.equal(f.receivers[0],receiver,mode);
    assert.equal(f.argumentsReceived[0],argument,mode);
  }
});

test('partial resize and zoom reuse covered ink without invoking WASM or replacing its nodes',()=>{
  const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(1440);
  const svg=f.doc.hookedElem.firstElementChild, ink=svg.children.find(node=>node.classList.contains('typst-page')&&node.getAttribute('data-dummy')!=='1');
  const renders=f.windows.length;
  f.scroll.clientWidth=594;f.event('resize');f.flush();
  assert.equal(f.windows.length,renders);assert.equal(f.doc.hookedElem.firstElementChild,svg);
  assert.ok(svg.children.includes(ink));assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  f.action({action:'zoom-in'});f.flush();
  assert.equal(f.windows.length,renders);assert.equal(f.doc.hookedElem.firstElementChild,svg);assert.ok(f.visibleComplete());
});

test('geometry reuse preserves ten CSS pixel gaps, paired paper rectangles, root bounds and cursor decoration',()=>{
  const pages=[{width:600,height:600},{width:400,height:600},{width:500,height:600},{width:600,height:600}];
  const f=partialFixture({decorated:true,pages});f.doc.cursorPosition=[1,20,30];f.start();f.flush();
  const svg=f.doc.hookedElem.firstElementChild, renders=f.windows.length;
  f.scroll.clientWidth=594;f.event('resize');f.flush();assert.equal(f.windows.length,renders);
  const groups=svg.children.filter(node=>node.classList.contains('typst-page'));
  for(let i=1;i<groups.length;i++) {
    const previous=groups[i-1],current=groups[i];
    const gap=(Number(current.getAttribute('data-y'))-Number(previous.getAttribute('data-y'))-Number(previous.getAttribute('data-page-height')))*.99;
    assert.ok(Math.abs(gap-10)<1e-8);
    const paper=svg.children.find(node=>node.classList.contains('typst-page-inner')&&Number(node.getAttribute('data-page-number'))===i);
    assert.equal(paper.getAttribute('transform'),`${current.getAttribute('transform')} scale(0.01)`);
  }
  assert.ok(Math.abs(Number(svg.getAttribute('data-height'))-(2400+30/.99))<1e-8);
  const outer=svg.children.find(node=>node.classList.contains('typst-page-outer'));
  assert.equal(Number(outer.getAttribute('height')),Math.ceil(2400+30/.99));
  const cursor=svg.children.find(node=>node.tagName==='circle');
  assert.ok(Math.abs(Number(cursor.getAttribute('r'))*.01*.99-5)<1e-8);
  assert.equal(cursor.getAttribute('transform'),`${groups[0].getAttribute('transform')} scale(0.01)`);
  assert.equal(cursor.getAttribute('cx'),'2000');assert.equal(cursor.getAttribute('cy'),'3000');
});

test('geometry reuse lets resize anchoring resolve refreshed paper offsets and retains a fixed source point',()=>{
  const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(1440);
  const svg=f.doc.hookedElem.firstElementChild, pages=svg.children.filter(node=>node.classList.contains('typst-page'));
  const anchor={page:4,local:200}, renders=f.windows.length, rescale=f.doc.r.rescale;
  f.doc.r.rescale=()=>{rescale();const paper=pages[anchor.page];f.scroll.scrollTop=(Number(paper.getAttribute('data-y'))+anchor.local)*f.scroll.clientWidth/600;};
  f.scroll.clientWidth=594;f.event('resize');f.flush();
  assert.equal(f.windows.length,renders);assert.equal(f.doc.hookedElem.firstElementChild,svg);
  const local=f.scroll.scrollTop/.99-Number(pages[4].getAttribute('data-y'));
  assert.ok(Math.abs(local-anchor.local)<1e-8);assert.ok(f.visibleComplete());
});

test('geometry reuse falls back for incomplete associations, uncovered pages and pending identical-geometry source patches',()=>{
  for(const kind of ['association','uncovered','source','upstream']) {
    const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(1460);
    const renders=f.windows.length, svg=f.doc.hookedElem.firstElementChild;
    if(kind==='association') {
      const paper=svg.children.find(node=>node.classList.contains('typst-page-inner'));
      paper.parentElement=null;svg.children.splice(svg.children.indexOf(paper),1);
    }
    if(kind==='source')f.doc.addChangement(['diff-v1',Array.from({length:48},()=>({width:600,height:300}))]);
    if(kind==='upstream')f.doc.addViewportChange();
    f.scroll.clientWidth=kind==='uncovered'?300:594;f.event('resize');f.flush();
    assert.ok(f.windows.length>renders,kind);assert.ok(f.visibleComplete(),kind);assert.equal(f.frames.size,0,kind);
  }
});


test('partial fallback zoom preserves page-local and gap pointer anchors on distant pages',()=>{
  for(const location of ['page','gap'])for(const reason of ['association','range','upstream']) {
    const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();
    const point=location==='page'?200:305, top=18*310;f.scrollTo(top+point-100);
    const svg=f.doc.hookedElem.firstElementChild,renders=f.windows.length;
    if(reason==='association') {
      const paper=svg.children.find(node=>node.classList.contains('typst-page-inner'));
      paper.parentElement=null;svg.children.splice(svg.children.indexOf(paper),1);
    }
    if(reason==='upstream')f.doc.addViewportChange();
    const zooms=reason==='range'?5:1;
    for(let i=0;i<zooms;i++)f.action({action:reason==='range'?'zoom-out':'zoom-in'});
    f.flush();
    assert.ok(f.windows.length>renders,`${location}/${reason} must exercise the normal patch`);
    const next=f.doc.hookedElem.firstElementChild,scale=f.doc.currentScaleRatio;
    const pages=next.children.filter(node=>node.classList.contains('typst-page'));
    const y=(f.scroll.scrollTop+100)/scale, start=Number(pages[18].getAttribute('data-y'));
    if(location==='page')assert.ok(Math.abs(y-start-point)<1e-8,`${location}/${reason}: ${y-start}`);
    else {
      const end=start+300,gap=Number(pages[19].getAttribute('data-y'))-end;
      assert.ok(Math.abs((y-end)/gap-.5)<1e-8,`${location}/${reason}: ${(y-end)/gap}`);
    }
    assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  }
});


test('far-page Ctrl-wheel fallback preserves its pointer and needs at most one repair patch',()=>{
  const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(18*310+100);
  const renders=f.windows.length;
  f.event('wheel',{ctrlKey:true,deltaMode:0,deltaY:500,clientX:123,clientY:137});f.flush();
  assert.ok(f.windows.length>renders);assert.ok(f.windows.length<=renders+2);
  const svg=f.doc.hookedElem.firstElementChild;
  const page=svg.children.find(node=>node.classList.contains('typst-page')&&Number(node.getAttribute('data-page-number'))===18);
  const local=(f.scroll.scrollTop+100)/f.doc.currentScaleRatio-Number(page.getAttribute('data-y'));
  assert.ok(Math.abs(local-200)<1e-8);assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
});

test('queued source patches never reuse a captured zoom point even when the page dimensions are identical',()=>{
  for(const repaginated of [false,true]) {
    const f=partialFixture({decorated:true});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(18*310+100);
    const top=f.scroll.scrollTop,renders=f.windows.length;
    f.event('wheel',{ctrlKey:true,deltaMode:0,deltaY:-100,clientX:123,clientY:137});
    f.doc.addChangement(['diff-v1',Array.from({length:48},()=>({width:600,height:repaginated?250:300}))]);f.flush();
    // The existing input path anchors raw coordinates until a new source can
    // supply new locations; an old page/local or gap point is no longer valid.
    const expected=(top+100)*Math.exp(.2)-100;
    assert.ok(Math.abs(f.scroll.scrollTop-expected)<1e-8);
    assert.deepEqual(f.merges,['new','diff-v1']);assert.ok(f.windows.length>renders);
    assert.ok(f.windows.length<=renders+2);assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  }
});

test('geometry reuse scales existing leaf cursor ellipses and is idempotent for repeated unchanged resizes',()=>{
  const f=partialFixture({decorated:true}),patch=f.doc.r.rerender;f.doc.cursorPaths=[[0]];
  f.doc.r.rerender=()=>{
    patch();const page=f.doc.hookedElem.firstElementChild.children.find(node=>node.classList.contains('typst-page'));
    const attrs={rx:'5',ry:'10',cx:'2',cy:'3'};
    const ellipse={tagName:'ellipse',parentElement:page,children:[],classList:{contains:name=>name==='typst-svg-cursor'},
      getAttribute:name=>attrs[name]??null,setAttribute(name,value){attrs[name]=String(value);},querySelectorAll:()=>[]};
    page.children.push(ellipse);
  };
  f.start();f.flush();const svg=f.doc.hookedElem.firstElementChild,renders=f.windows.length;
  f.scroll.clientWidth=594;f.event('resize');f.flush();
  const ellipse=svg.querySelectorAll('.typst-svg-cursor')[0],children=svg.children.length;
  assert.equal(f.windows.length,renders);
  assert.ok(Math.abs(Number(ellipse.getAttribute('rx'))*.99-5)<1e-8);
  assert.ok(Math.abs(Number(ellipse.getAttribute('ry'))*.99-10)<1e-8);
  assert.equal(ellipse.getAttribute('cx'),'2');assert.equal(ellipse.getAttribute('cy'),'3');
  for(let i=0;i<10;i++){f.event('resize');f.flush();}
  assert.equal(f.windows.length,renders);assert.equal(svg.children.length,children);
  assert.ok(Math.abs(Number(ellipse.getAttribute('rx'))*.99-5)<1e-8);assert.equal(f.frames.size,0);
  const searches=f.cursorSearches;
  ellipse.parentElement=null;for(const page of svg.children) {
    const index=page.children.indexOf(ellipse);
    if(index>=0){page.children.splice(index,1);break;}
  }
  f.action({action:'zoom-in'});f.flush();
  assert.ok(f.windows.length>renders,'detached cached cursor invalidates reuse');
  assert.equal(f.cursorSearches,searches+1,'discover replacement cursors after patch');
  const replacement=f.doc.hookedElem.firstElementChild.querySelectorAll('.typst-svg-cursor')[0];
  assert.notEqual(replacement,ellipse);assert.ok(f.visibleComplete());
  const patchedSearches=f.cursorSearches;
  f.action({action:'zoom-in'});f.flush();
  assert.equal(f.cursorSearches,patchedSearches,'replacement decorations are reused');
  f.doc.cursorPaths=undefined;
  f.doc.addChangement(['diff-v1',Array.from({length:48},()=>({width:600,height:300}))]);f.flush();
  const cleared=f.cursorSearches;
  f.action({action:'zoom-in'});f.flush();
  assert.equal(f.cursorSearches,cleared,'cleared cursor paths discard the cached list');
});


test('full renderer operation delegates preserve receiver, arguments and thrown errors',()=>{
  for(const method of ['fetchSvgDataByDocMode','shouldMixinCanvas','triggerUpdate','addChangement','postprocessChanges','reset']) {
    const error=Error(`full ${method} failure`),calls=[];
    const f=partialFixture({partial:false,configure(doc){doc[method]=function(...args){calls.push({receiver:this,args});throw error;};}});
    const receiver={partialRendering:false},args=[['new','payload'],{}];
    assert.throws(()=>f.doc[method].call(receiver,...args),value=>value===error);
    assert.equal(calls.length,1);assert.equal(calls[0].receiver,receiver);assert.deepEqual(calls[0].args,args);
    assert.equal(f.frames.size,0);assert.equal(f.windows.length,0);
  }
});

test('SVG link initialization failures remain observable and retry does not mark links as initialized',()=>{
  for(const partial of [false,true]) {
    const error=Error('initializer failure'),calls=[];let fail=true;
    const f=partialFixture({partial,initializeLinks(root,...args){calls.push({receiver:this,args,root});if(fail)throw error;return 'initialized';}});
    f.start();f.flush();f.flush();
    const root=f.doc.hookedElem.firstElementChild,link={};root.getElementsByClassName=()=>[link];
    const receiver={},argument={};
    assert.throws(()=>f.doc.windowElem.initTypstSvg.call(receiver,root,argument),value=>value===error);
    fail=false;assert.equal(f.doc.windowElem.initTypstSvg.call(receiver,root,argument),'initialized');
    assert.equal(f.doc.windowElem.initTypstSvg.call(receiver,root,argument),undefined);
    assert.equal(calls.length,2);assert.equal(calls[0].receiver,receiver);assert.equal(calls[1].receiver,receiver);
    assert.deepEqual(calls[0].args,[argument]);assert.deepEqual(calls[1].args,[argument]);
    assert.deepEqual([...calls[1].root.getElementsByClassName('pseudo-link')],[link]);
  }
});


test('fractional page metadata uses decorated SVG sizes while retaining source render bounds',()=>{
  const pages=Array.from({length:48},()=>({width:595.2755737304688,height:841.8897705078125}));
  const f=partialFixture({decorated:true,pages});f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(18*852+150);
  const svg=f.doc.hookedElem.firstElementChild,renders=f.windows.length;
  for(let step=0;step<5;step++) {
    f.action({action:'zoom-in'});f.flush();
    assert.equal(f.doc.hookedElem.firstElementChild,svg,'covered zoom must retain the SVG');
    assert.equal(f.windows.length,renders,'fractional metadata must not force a WASM patch');
    assert.ok(f.visibleComplete());
    assert.ok(Math.abs(f.doc.currentScaleRatio-1.1**(step+1))<1e-10);
  }
  f.scrollTo(40*852*1.1**5);
  const bounds=f.windows.at(-1);
  // WASM bounds use unrounded document coordinates, not the decorated papers.
  assert.ok(bounds[1]>38*pages[0].height&&bounds[1]<41*pages[0].height);
  assert.ok(Math.abs(bounds[1]/pages[0].height-Math.round(bounds[1]/pages[0].height))<1e-10);
  assert.ok(Math.abs(bounds[3]/pages[0].height-Math.round(bounds[3]/pages[0].height))<1e-10);
  assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
});


test('covered partial zoom lays out once and does not search unchanged page ink for cursors',()=>{
  for(const cursor of [false,true]) {
    const f=partialFixture({decorated:true,pages:Array.from({length:48},()=>({width:595.276,height:841.89}))});f.scroll.clientHeight=200;
    if(cursor)f.doc.cursorPaths=[[0]];
    f.start();f.flush();f.scrollTo(18*852+150);
    const layouts=f.layouts,searches=f.cursorSearches,renders=f.windows.length;
    for(let i=0;i<5;i++){f.action({action:'zoom-in'});f.flush();}
    assert.equal(f.windows.length,renders,'covered zoom retains ink');
    assert.equal(f.layouts-layouts,5,'one physical SVG layout per zoom frame');
    assert.equal(f.cursorSearches-searches,0,'cursor discovery belongs to a patch, not zoom');
    assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  }
});


test('full preview page tracking reads logarithmic paper bounds during zoom and scroll',()=>{
  const f=fixture();let reads=0,y=0;
  const bounds=Array.from({length:1000},(_,index)=>{const height=100+index%5*80,start=y;y+=height+10;return {start,height};});
  f.doc.hookedElem.firstElementChild.children=bounds.map(({start,height})=>({
    tagName:'g',getAttribute:name=>name==='data-page-width'?'600':String(height),
    getScreenCTM(){reads++;return {a:f.doc.currentScaleRatio,d:f.doc.currentScaleRatio,e:0,f:start*f.doc.currentScaleRatio-f.scroll.y};},
  }));
  for(const scale of [.1,1,2,10])for(const index of [0,1,501,998,999]) {
    f.doc.currentScaleRatio=scale;f.scroll.y=bounds[index].start*scale+20;
    let expected=0,overlap=0;
    bounds.forEach(({start,height},i)=>{const top=start*scale-f.scroll.y,n=Math.max(0,Math.min(top+height*scale,600)-Math.max(top,0));if(n>overlap){expected=i;overlap=n;}});
    reads=0;f.event('scroll',{target:f.scroll});
    assert.equal(f.messages.at(-1).page,expected,`scale ${scale}, starting page ${index}`);
    const visible=bounds.filter(({start,height})=>start*scale-f.scroll.y<600&&(start+height)*scale-f.scroll.y>0).length;
    assert.ok(reads<=12+visible,`${reads} bounds reads for ${visible} visible papers`);
  }
  assert.equal(f.frames.length,0);assert.equal(f.demands,0);
});


test('deferred far-page zoom refills its anchored destination on the first patch',()=>{
  const f=partialFixture({decorated:true,pages:Array.from({length:48},()=>({width:595.276,height:841.89}))});
  f.scroll.clientHeight=200;f.start();f.flush();f.scrollTo(18*852+150);
  const renders=f.windows.length,oldScale=600/596;
  const findPaper=()=>f.doc.hookedElem.firstElementChild.children.find(node=>node.classList.contains('typst-page')&&node.getAttribute('data-page-number')==='18');
  const local=(f.scroll.scrollTop+100)/oldScale-Number(findPaper().getAttribute('data-y'));
  f.event('wheel',{ctrlKey:true,deltaMode:0,deltaY:500,clientX:123,clientY:137});f.flush();
  assert.equal(f.windows.length,renders+1,'fetch the anchored destination, not the unscaled scroll position');
  const scale=oldScale*f.doc.currentScaleRatio;
  assert.ok(Math.abs((f.scroll.scrollTop+100)/scale-Number(findPaper().getAttribute('data-y'))-local)<1e-8);
  assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
});

 test('snapshot readiness compares physical fit scale without delaying SVG for a DOM bitmap',()=>{
  let read, finished=0, invalidated=0;
  const snapshot={attach(reader){read=reader;},rendered(){finished++;},invalidate(){invalidated++;},begin(){return true;}};
  const f=partialFixture({decorated:true,zoomSnapshot:snapshot});
  f.start();f.flush();
  assert.equal(read().ready,true);
  const layouts=f.layouts;
  f.action({action:'zoom-in'});
  assert.equal(read().ready,false);
  f.flush();
  assert.equal(f.doc.currentScaleRatio,1.1, 'native bitmap transforms need no extra renderer frame');
  assert.ok(f.layouts<=layouts+1);
  f.flush();
  assert.ok(Math.abs(f.doc.currentScaleRatio-1.1)<1e-6);
  assert.equal(read().ready,true);
  assert.ok(finished>0);
  f.doc.addChangement(['diff-v1',[]]);
  assert.ok(invalidated>0);assert.equal(read().ready,false);
 });


test('full rendering notifies the idle cache only after its final asynchronous completion', () => {
  let read;
  const ready = [];
  const f = partialFixture({ partial: false, zoomSnapshot: {
    attach(reader) { read = reader; },
    invalidate() {},
    rendered() { ready.push(read()?.ready); },
  } });
  f.start();
  f.flush();
  assert.equal(read().ready, false);
  assert.deepEqual(ready, []);
  f.flush();
  assert.equal(read().ready, true);
  assert.deepEqual(ready, [true], 'completion must admit an idle capture without another user input');
  assert.equal(f.frames.size, 0, 'completion adds no polling frames');
});


test('Home and End explicitly navigate both SVG viewers while preserving modified and text-input keys', () => {
  for (const partial of [false, true]) {
    const f = partialFixture({ partial, decorated: true });
    f.start(); f.flush(); f.flush();
    for (const flags of [{metaKey:true},{ctrlKey:true},{altKey:true},{shiftKey:true},{target:{closest:()=>({})}}]) {
      f.event('keydown', {key:'End', ...flags});
      assert.equal(f.scroll.scrollTop, 0);
    }
    f.event('keydown', {key:'End'}); f.flush(); f.flush();
    assert.ok(f.scroll.scrollTop > 14000, 'End reaches the last paper without relying on WebKit scroll focus');
    f.event('keydown', {key:'Home'}); f.flush(); f.flush();
    assert.equal(f.scroll.scrollTop, 0);
    assert.ok(f.visibleComplete());
  }
});


test('keyboard palette inversion invalidates cached pixels before repaint and schedules only idle recapture', () => {
  const events = [];
  const f = partialFixture({ zoomSnapshot: {
    attach() {}, rendered() {},
    invalidate() { events.push('invalidate'); },
    settle() { events.push('settle'); },
  } });
  f.start(); f.flush(); events.length = 0;
  f.event('keydown', {key:'t'});
  assert.deepEqual(events, ['invalidate','settle']);
});

test('native absolute zoom uses one renderer frame and preserves an equal-scale coalesced pan',()=>{
  let request;const helper={attach(){},rendered(){},invalidate(){},begin(){return false;},native(value){request=value;return true;}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();
  f.nativeZoom({scale:1.1,revision:1,x:300,y:200});f.flush();assert.equal(f.doc.currentScaleRatio,1.1);
  const left=f.scroll.scrollLeft,top=f.scroll.scrollTop;
  f.nativeZoom({scale:1.1,revision:2,x:300,y:200,panX:10,panY:20});f.flush();
  assert.equal(request.revision,2);assert.equal(f.doc.currentScaleRatio,1.1);
  assert.ok(Math.abs(f.scroll.scrollLeft-left+10)<1e-6);assert.ok(Math.abs(f.scroll.scrollTop-top+20)<1e-6);
});

test('native resize owns smooth divider work and sends a final absolute scroll target',()=>{
  const helper={attach(){},rendered(){},invalidate(){},begin(){},resize(){return true;},native(){return true;}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();
  const before=f.layouts;f.event('resize',{});f.flush();assert.equal(f.layouts,before);
  f.nativeZoom({revision:1,scale:1,resize:true,scroll:{left:0,top:120}});f.flush();
  assert.equal(f.scroll.scrollTop,120);
});


for(const partial of [false,true]) test(`${partial?'partial':'full'} DOM resize refreshes an active native flight before its sharp handoff`,()=>{
  const f=partialFixture({partial,decorated:true,nativeSnapshot:true});
  f.start();f.flush();f.flush();
  const cache=f.window.tiptoptypZoomSnapshot;
  cache.configure(true);
  f.scroll.clientWidth=560;f.window.innerWidth=560;
  const request={revision:6,scale:1,resize:true,token:'cached-pixels',source:{x:0,y:{page:18,local:75},clientX:23,clientY:37}};
  assert.equal(f.nativeZoom(request),true);f.flush();
  assert.equal(f.doc.cachedDOMState.width,560);
  assert.equal(cache.owns(6,f.doc),true);
  const renders=f.windows.length,layouts=f.layouts;
  // The first flight laid out at560, but AppKit has already queued the next
  // native width480. Only its delayed DOM notification is arriving now.
  f.scroll.clientWidth=480;f.window.innerWidth=480;
  for(let i=0;i<100;i++)f.event('resize');
  assert.ok(f.frames.size<=2,'at most one existing handoff RAF plus one coalesced geometry refresh');
  for(let i=0;i<6;i++)f.flush();
  assert.equal(f.doc.cachedDOMState.width,480,'the active slot must refresh before its queued successor can run');
  assert.equal(cache.inspect().active,false);
  assert.equal(f.messages.filter(message=>message.op==='ready').length,1);
  assert.equal(f.messages.filter(message=>message.op==='ready')[0].revision,6);
  assert.equal(f.windows.length,renders,'same-owner resize must reuse SVG ink');
  assert.ok(f.layouts<=layouts+(partial?2:1),`one geometry refresh uses bounded layout passes: ${f.layouts-layouts}`);
  assert.equal(f.frames.size,0,'geometry refresh must not introduce idle polling');
  assert.equal(f.messages.filter(message=>message.op==='capture').length,0);
  const page=f.doc.hookedElem.firstElementChild.children.filter(node=>node.classList.contains('typst-page'))[18];
  const matrix=page.getScreenCTM();
  assert.ok(Math.abs(matrix.f+75*matrix.d-37)<1e-8,'refresh must retain the active request source anchor');
});

for(const ownerChange of ['revision','document']) test(`DOM resize cannot resurrect native ownership after ${ownerChange} changes`,()=>{
  const f=partialFixture({partial:false,decorated:true,nativeSnapshot:true});
  f.start();f.flush();f.flush();
  const cache=f.window.tiptoptypZoomSnapshot;
  cache.configure(true);
  f.scroll.clientWidth=560;f.window.innerWidth=560;
  const request={revision:6,scale:1,resize:true,token:'cached-pixels'};
  f.nativeZoom(request);f.flush();
  const owner=ownerChange==='document'?{}:f.doc;
  const revision=ownerChange==='revision'?7:6;
  cache.native({...request,revision},owner);
  const layouts=f.layouts;
  f.scroll.clientWidth=480;f.window.innerWidth=480;
  for(let i=0;i<100;i++)f.event('resize');
  for(let i=0;i<4;i++)f.flush();
  assert.equal(f.layouts,layouts,'the stale request must not mutate another owner layout');
  assert.equal(f.doc.cachedDOMState.width,560);
  assert.equal(cache.owns(revision,owner),true,'stale metadata must not release the current owner');
  assert.equal(f.messages.filter(message=>message.op==='ready').length,0);
  assert.equal(f.frames.size,0,'rejected stale work must not poll');
  cache.abandon(revision);f.flush();
  assert.equal(cache.inspect().active,false);
});

test('buffered native anchors preserve page-local points and fixed-size gaps after zoom and resize',()=>{
  for(const resize of [false,true]) for(const gap of [false,true]) {
    const helper={attach(){},rendered(){},invalidate(){},begin(){},scrolled(){},native(){return true;}};
    const f=partialFixture({decorated:true,zoomSnapshot:helper});f.scroll.clientHeight=200;f.start();f.flush();
    f.scrollTo(18*310+100);f.flush();
    const y=gap?{before:18,after:19,fraction:.5}:{page:18,local:75};
    if(resize)f.scroll.clientWidth=480;
    f.nativeZoom({revision:1,scale:resize?1:.7,resize,source:{x:100,y,clientX:123,clientY:137}});f.flush();f.flush();
    const scale=f.doc.currentScaleRatio*f.scroll.clientWidth/600;
    const projected=18*(300*scale+10)+(gap?300*scale+5:75*scale);
    assert.ok(Math.abs(f.scroll.scrollTop-(37+projected-137))<1e-8,`${resize}/${gap}: native and SVG anchor differ`);
    assert.ok(Math.abs(f.scroll.scrollLeft-(23+100*scale-123))<1e-8);
    assert.ok(f.visibleComplete());assert.equal(f.frames.size,0);
  }
});


for (const resize of [true, false]) test(`full native ${resize ? 'divider' : 'equal-scale zoom'} absolute scroll owns delayed sharp layout`, () => {
  let semantic = null;
  const helper = {attach() {}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial: false, decorated: true, zoomSnapshot: helper,
    layoutHook() {
      if (semantic) f.scroll.scrollTop = semantic.top;
    } });
  f.start(); f.flush(); f.flush();
  f.scroll.scrollTop = 14570;
  semantic = {top: 14570};
  f.doc.clearSvgResizeAnchor = () => {semantic = null;};
  // Divider resizing or a coalesced pinch round trip can leave the manual
  // scale unchanged. The frontend then retains its older resize anchor.
  f.scroll.clientWidth = 480;
  const target = {left: 12, top: 14548.24};
  assert.equal(f.nativeZoom({revision: 1, scale: 1, resize, scroll: target}), true);
  f.flush();
  assert.equal(f.scroll.scrollTop, target.top);
  assert.equal(f.scroll.scrollLeft, target.left);
  // A later real SVG patch changes the cached geometry and invokes the
  // frontend's semantic resize restoration during postprocessing.
  f.doc.r.rerender(); f.doc.postprocessChanges();
  assert.equal(f.scroll.scrollTop, target.top, 'delayed SVG layout must preserve the native absolute scroll');
  assert.equal(f.scroll.scrollLeft, target.left);
  assert.equal(semantic, null, 'explicit native scroll retires the competing resize anchor');
  assert.equal(f.frames.size, 0);
  assert.deepEqual(f.errors, []);
});


for (const resize of [true, false]) test(`full ${resize ? 'divider resize' : 'zoom'} preserves page ink with fixed gaps and no WASM patch`, () => {
  let read;
  const helper = {attach(reader) {read = reader;}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial: false, decorated: true, zoomSnapshot: helper, pages: Array.from({length: 512}, () => ({width: 600, height: 300}))});
  f.scroll.clientHeight = 200; f.start(); f.flush(); f.flush();
  f.scrollTo(180 * 310 + 100); f.flush(); f.flush();
  const renders = f.windows.length;
  if (resize) f.scroll.clientWidth = 300;
  f.nativeZoom({revision: 1, scale: resize ? 1 : .5, resize, source: {x: 100, y: {page: 180, local: 200}, clientX: 123, clientY: 137}});
  f.flush(); f.flush();
  const svg = f.doc.hookedElem.firstElementChild, papers = svg.children.filter(node => node.classList.contains('typst-page'));
  const scale = f.scroll.clientWidth / 600 * f.doc.currentScaleRatio;
  const target = papers[180].getScreenCTM();
  assert.ok(Math.abs(target.f + 200 * target.d - 137) < 1e-8, 'snapshot and sharp page-local ink must share the anchor');
  for (const index of [1, 180, 511]) {
    const gap = (Number(papers[index].getAttribute('data-y')) - Number(papers[index - 1].getAttribute('data-y')) - 300) * scale;
    assert.ok(Math.abs(gap - 10) < 1e-8, `page ${index} gap must remain ten CSS pixels, got ${gap}`);
  }
  assert.equal(f.windows.length, renders, 'full geometry changes must not invoke WASM');
  assert.equal(read().geometry.pages.length, 512, 'full snapshots need authoritative page offsets');
  assert.ok(Math.abs(read().geometry.source_scale - scale) < 1e-8);
  assert.equal(read().ready, true);
  assert.deepEqual(f.errors, []);
  assert.equal(f.frames.size, 0);
});

test('full layout geometry refreshes after equal-total-height source changes and keeps cursor sizes stable', () => {
  let read;
  const helper = {attach(reader) {read = reader;}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial: false, decorated: true, zoomSnapshot: helper, pages: [{width:600,height:300},{width:400,height:300},{width:600,height:300}]});
  f.doc.cursorPosition = [2, 20, 30]; f.start(); f.flush(); f.flush();
  const previous = read().geometry.pages;
  f.doc.addChangement(['diff-v1', [{width:600,height:250},{width:400,height:350},{width:600,height:300}]]); f.flush(); f.flush();
  const next = read();
  assert.notEqual(next.geometry.pages, previous);
  assert.equal(next.geometry.pages[0][1], 250);
  assert.equal(next.geometry.pages[1][1], 350);
  const renders = f.windows.length;
  f.nativeZoom({revision:1,scale:2}); f.flush();
  const svg = f.doc.hookedElem.firstElementChild;
  const cursor = svg.children.find(node => node.tagName === 'circle');
  assert.ok(Math.abs(Number(cursor.getAttribute('r')) * .01 * 2 - 5) < 1e-8);
  assert.equal(f.windows.length, renders);
  assert.equal(read().ready, true);
  assert.deepEqual(f.errors, []);
});


test('full idle rescale and unchanged postprocessing retain immutable snapshot geometry without WASM work', () => {
  let read;
  const helper = {attach(reader) {read = reader;}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial: false, decorated: true, zoomSnapshot: helper});
  f.start(); f.flush(); f.flush();
  const geometry = read().geometry.pages, layouts = f.layouts, renders = f.windows.length;
  for (let index = 0; index < 1000; index++) f.doc.r.rescale();
  assert.equal(f.layouts, layouts);
  assert.equal(read().geometry.pages, geometry);
  f.doc.postprocessChanges();
  assert.equal(read().geometry.pages, geometry, 'identical paper layout must not invalidate an idle capture');
  assert.equal(f.windows.length, renders);
  assert.equal(f.frames.size, 0);
});

test('full native source anchors and captures use actual SVG CTM rather than rounded viewport width', () => {
  let read;
  const helper = {attach(reader) {read = reader;}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial: false, decorated: true, zoomSnapshot: helper});
  f.start(); f.flush(); f.flush();
  const svg = f.doc.hookedElem.firstElementChild;
  svg.getScreenCTM = () => ({a: f.doc.currentScaleRatio * .99997, d: f.doc.currentScaleRatio * .99997,
    e: 23.3 - f.scroll.scrollLeft, f: 37.2 - f.scroll.scrollTop});
  f.nativeZoom({revision:1,scale:.5,source:{x:100,y:{page:18,local:200},clientX:123,clientY:137}}); f.flush();
  const frame = read(), matrix = svg.getScreenCTM();
  assert.equal(frame.geometry.source_scale, matrix.d);
  assert.ok(Math.abs(matrix.e + 100 * matrix.a - 123) < 1e-8);
  assert.ok(Math.abs(matrix.f + (frame.geometry.pages[18][0] + 200) * matrix.d - 137) < 1e-8);
  assert.equal(frame.ready, true);
});

test('full unsupported paper associations preserve ordinary rescale and discard piecewise capture metadata', () => {
  let read;
  const helper = {attach(reader) {read = reader;}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial:false,decorated:true,zoomSnapshot:helper});
  f.start(); f.flush(); f.flush();
  const svg = f.doc.hookedElem.firstElementChild;
  const background = svg.children.find(node => node.classList.contains('typst-page-inner'));
  background.parentElement = null;
  const renders = f.windows.length;
  f.nativeZoom({revision:1,scale:1.1,source:{x:100,y:{page:1,local:30},clientX:123,clientY:137}}); f.flush();
  assert.equal(f.doc.currentScaleRatio, 1.1);
  assert.equal(read().geometry.pages.length, 0);
  assert.equal(f.windows.length, renders);
  assert.deepEqual(f.errors, []);
  assert.equal(f.frames.size, 0);
});


for (const input of ['wheel', 'keyboard']) for (const location of ['page', 'gap']) test(`full uncached ${input} zoom preserves its ${location} pointer anchor`, () => {
  const f = partialFixture({partial:false,decorated:true,pages:Array.from({length:512},()=>({width:600,height:300}))});
  f.scroll.clientHeight = 200; f.start(); f.flush(); f.flush();
  f.scrollTo(180 * 310 + (location === 'page' ? 200 : 305) - 100);
  const renders = f.windows.length;
  if (input === 'wheel') f.event('wheel', {ctrlKey:true,deltaMode:0,deltaY:Math.log(2)/.002,clientX:123,clientY:137});
  else f.event('keydown', {metaKey:true,key:'-'});
  f.flush(); f.flush();
  const papers = f.doc.hookedElem.firstElementChild.children.filter(node=>node.classList.contains('typst-page'));
  const before = papers[180].getScreenCTM(), after = papers[181].getScreenCTM();
  const y = location === 'page' ? before.f + 200 * before.d : (before.f + 300 * before.d + after.f) / 2;
  assert.ok(Math.abs(y - 137) < 1e-8, `source point moved away from the zoom anchor: ${y}`);
  assert.equal(f.windows.length, renders, 'uncached full zoom only updates geometry');
  assert.equal(f.frames.size, 0);
  assert.deepEqual(f.errors, []);
});


test('full late invalid native source anchors after source shrink are harmless and valid zoom recovers', () => {
  const helper = {attach() {}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial:false,decorated:true,zoomSnapshot:helper});
  f.start(); f.flush(); f.flush();
  f.doc.addChangement(['diff-v1', [{width:600,height:300},{width:600,height:300}]]); f.flush(); f.flush();
  const invalid = [{before:18,after:19,fraction:.5}, {before:-1,after:1,fraction:.5},
    {before:.5,after:1,fraction:.5}, {before:1,after:0,fraction:.5}, {before:0,after:1,fraction:NaN},
    {page:18,local:30}, {page:-1,local:30}, {page:.5,local:30}, {page:0,local:Infinity}, null];
  for (const [index, y] of invalid.entries()) assert.doesNotThrow(() => {
    f.nativeZoom({revision:index+1,scale:1+index*.01,source:{x:100,y,clientX:123,clientY:137}}); f.flush(); f.flush();
    assert.ok(Number.isFinite(f.scroll.scrollTop));
    assert.ok(Number.isFinite(f.scroll.scrollLeft));
  }, 'a delayed or malformed source point must not strand the renderer');
  f.nativeZoom({revision:invalid.length+1,scale:1.2,source:{x:100,y:{page:1,local:50},clientX:123,clientY:137}}); f.flush();
  const paper = f.doc.hookedElem.firstElementChild.children.filter(node=>node.classList.contains('typst-page'))[1];
  const matrix = paper.getScreenCTM();
  assert.ok(Math.abs(matrix.f + 50 * matrix.d - 137) < 1e-8);
  assert.deepEqual(f.errors, []);
  assert.equal(f.frames.size, 0);
});

test('full source projection permits finite page-local coordinates outside the page for edge clamping', () => {
  const helper = {attach() {}, rendered() {}, invalidate() {}, begin() {}, scrolled() {}, native() {return true;}};
  const f = partialFixture({partial:false,decorated:true,zoomSnapshot:helper});
  f.start(); f.flush(); f.flush();
  for (const [index, local] of [-50, 350].entries()) {
    f.nativeZoom({revision:index+1,scale:1.1,source:{x:100,y:{page:1,local},clientX:123,clientY:137}}); f.flush();
    const paper = f.doc.hookedElem.firstElementChild.children.filter(node=>node.classList.contains('typst-page'))[1];
    const matrix = paper.getScreenCTM();
    assert.ok(Math.abs(matrix.f + local * matrix.d - 137) < 1e-8);
  }
  assert.deepEqual(f.errors, []);
});

for (const partial of [false, true]) test(`${partial ? 'partial' : 'full'} paper origin remains consistent through fractional resize and a replacement patch`, () => {
  const pages=[{width:600,height:301},{width:400,height:307},{width:600,height:313}];
  const f=partialFixture({partial,decorated:true,pages});
  f.start();f.flush();f.flush();
  const expected=partial?'xMinYMin':null;
  assert.equal(f.doc.hookedElem.firstElementChild.getAttribute('preserveAspectRatio'),expected);
  f.scroll.clientWidth=593;f.event('resize');f.flush();f.flush();
  assert.equal(f.doc.hookedElem.firstElementChild.getAttribute('preserveAspectRatio'),expected);
  f.doc.addChangement(['diff-v1',pages]);f.flush();f.flush();
  assert.equal(f.doc.hookedElem.firstElementChild.getAttribute('preserveAspectRatio'),expected);
  assert.deepEqual(f.errors,[]);
});

for (const partial of [false,true]) test(`${partial?'partial':'full'} duplicate socket registration keeps one live renderer and source updates working`,()=>{
  let read,accepted=0;
  const helper={attach(fn){read=fn;},rendered(){},invalidate(){},begin(){},scrolled(){},native(){accepted++;return true;}};
  const f=partialFixture({partial,decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();
  for(let reconnect=0;reconnect<3;reconnect++)f.container.documents.push(f.container.documents[0]);
  f.action({action:'zoom-in'});f.flush();f.flush();
  assert.equal(f.doc.currentScaleRatio,1.1,'reconnect disabled zoom while keeping source alive');
  assert.equal(f.nativeZoom({revision:1,scale:1.2,token:null}),true);f.flush();f.flush();
  assert.equal(accepted,1);assert.equal(read().scale,1.2);
  f.doc.addChangement(['diff-v1',[{width:600,height:350}]]);f.flush();f.flush();
  assert.equal(read().geometry.pages[0][1],350);assert.deepEqual(f.errors,[]);
});
test('ready requires fitted SVG geometry to match the current live container width',()=>{
  let read;
  const helper={attach(fn){read=fn;},rendered(){},invalidate(){},begin(){},scrolled(){}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();
  assert.equal(read().ready,true);f.scroll.clientWidth=593;
  assert.equal(read().ready,false,'old cached DOM width was treated as the current sharp frame');
  f.event('resize');f.flush();f.flush();assert.equal(read().ready,true);
});

test('distinct live renderer identities stay ambiguous after reconnect deduplication',()=>{
  let read;
  const helper={attach(fn){read=fn;},rendered(){},invalidate(){},begin(){},scrolled(){},native(){throw Error('must not choose an ambiguous document');}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();
  f.container.documents.push({impl:{...f.doc}});assert.equal(read(),null);
  assert.equal(f.nativeZoom({revision:1,scale:1.2,token:null}),false);
  f.action({action:'zoom-in'});f.flush();assert.equal(f.doc.currentScaleRatio,1);
});

for (const replacement of [false,true]) test(`accepted native request terminates when its renderer ${replacement?'is replaced':'disappears'} before the queued frame`,()=>{
  const abandoned=[];
  const helper={attach(){},rendered(){},invalidate(){},begin(){},scrolled(){},native(){return true;},abandon(revision){abandoned.push(revision);}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();
  const original=f.container.documents[0];
  assert.equal(f.nativeZoom({revision:7,scale:1.2,token:null}),true);
  f.container.documents=replacement?[{impl:{...f.doc}}]:[];f.flush();
  assert.deepEqual(abandoned,[7],'accepted native flight was silently discarded');
  f.container.documents=[original];assert.equal(f.nativeZoom({revision:8,scale:1.3,token:null}),true);f.flush();f.flush();
  assert.equal(f.doc.currentScaleRatio,1.3);assert.deepEqual(abandoned,[7]);
});

for(const dispose of [false,true]) test(`renderer ${dispose?'disposal':'reset'} retires an already applied native flight before acknowledgement`,()=>{
  const retired=[];
  const helper={attach(){},rendered(){},invalidate(){},begin(){},scrolled(){},native(){return true;},retireDocument(doc){retired.push(doc);return true;}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();retired.length=0;
  assert.equal(f.nativeZoom({revision:9,scale:1.2,token:null}),true);f.flush();assert.equal(f.doc.currentScaleRatio,1.2);
  if(dispose)f.doc.disposeList.forEach(fn=>fn());else f.doc.reset();
  assert.deepEqual(retired,[f.doc]);
});

test('repeated socket registration retains one renderer entry and does not grow per-frame lookup work',()=>{
  const f=partialFixture({decorated:true});f.start();f.flush();f.flush();
  const original=f.container.documents[0], renders=f.windows.length;
  for(let i=0;i<10000;i++)f.container.documents.push(original);
  assert.equal(f.container.documents.length,1);assert.equal(f.windows.length,renders);assert.equal(f.frames.size,0);
});

test('source reset cannot replay a cancelled native anchor from the queued input frame',()=>{
  let owns=false;
  const helper={attach(){},rendered(){},invalidate(){},begin(){},scrolled(){},native(){owns=true;return true;},
    owns(){return owns;},retireDocument(){owns=false;return true;},abandon(){owns=false;}};
  const f=partialFixture({decorated:true,zoomSnapshot:helper});f.start();f.flush();f.flush();
  f.doc.addChangement(['new',[{width:600,height:350},{width:600,height:350}]]);
  assert.equal(f.nativeZoom({revision:10,scale:1.2,token:null,source:{x:0,y:{page:18,local:30},clientX:0,clientY:0}}),true);
  f.flush();f.flush();assert.equal(f.doc.currentScaleRatio,1,'cancelled input was applied to reset source');assert.deepEqual(f.errors,[]);
});


function sourceContextFixture(partial = false) {
  const f = fixture();
  f.doc.partialRendering = partial;
  const svg = f.doc.hookedElem.firstElementChild;
  svg.isConnected = true;
  svg.contains = target => {
    for (let node = target; node; node = node.parentElement) if (node === svg) return true;
    return false;
  };
  const attrs = { 'data-page-number': '0', 'data-page-width': '600', 'data-page-height': '800' };
  const matrix = { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 };
  const page = { tagName: 'g', parentElement: svg, classList: { contains: name => name === 'typst-page' },
    getAttribute: name => attrs[name] ?? null, getScreenCTM: () => matrix };
  svg.children = [page];
  const target = { isConnected: true, parentElement: page, closest: selector => selector === '.typst-page' ? page : null,
    click() { throw Error('right-click source navigation must not follow links'); },
    dispatchEvent() { throw Error('source navigation must not synthesize clicks'); } };
  const calls = [];
  f.doc.hookedElem.sourceMappingHandler = function(event) { calls.push({ receiver: this, event }); };
  return { ...f, svg, page, target, attrs, matrix, calls };
}

for (const partial of [false, true]) test(`${partial ? 'partial' : 'full'} right-click reveals the clicked source once without following links`, () => {
  const f = sourceContextFixture(partial);
  const event = f.event('contextmenu', { target: f.target, clientX: 123.25, clientY: 287.5 });
  assert.ok(event.prevented && event.stopped);
  const request = f.messages.at(-1);
  assert.equal(request.type, 'preview-context-menu');
  assert.equal(request.x, 123.25); assert.equal(request.y, 287.5);
  assert.ok(Number.isSafeInteger(request.token));
  assert.equal(f.calls.length, 0, 'opening the menu must not navigate');
  assert.equal(f.revealSource(request.token), true);
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].receiver, f.doc.hookedElem);
  assert.equal(f.calls[0].event.target, f.page);
  assert.equal(f.calls[0].event.clientX, 123.25);
  assert.equal(f.calls[0].event.clientY, 287.5);
  assert.equal(f.revealSource(request.token), false);
  assert.equal(f.calls.length, 1);
  assert.equal(f.frames.length, 0, 'menus add no rendering or idle work');
});

test('new context requests supersede old tokens without allowing stale choices to consume them', () => {
  const f = sourceContextFixture();
  f.event('contextmenu', { target: f.target, clientX: 10, clientY: 20 });
  const first = f.messages.at(-1).token;
  f.event('contextmenu', { target: f.target, clientX: 30, clientY: 40 });
  const second = f.messages.at(-1).token;
  assert.ok(second > first);
  for (const invalid of [first, -1, NaN, Infinity, 1.5, String(second), null]) {
    assert.equal(f.revealSource(invalid), false);
  }
  assert.equal(f.calls.length, 0);
  assert.equal(f.revealSource(second), true);
  assert.equal(f.calls[0].event.clientX, 30);
  assert.equal(f.calls[0].event.clientY, 40);
});

for (const stale of ['page', 'svg', 'renderer', 'missing renderer', 'handler', 'dummy page', 'zoom', 'scroll', 'resize', 'page transform', 'page size', 'page identity']) {
  test(`source context choices reject a stale ${stale}`, () => {
    const f = sourceContextFixture();
    f.event('contextmenu', { target: f.target, clientX: 10, clientY: 20 });
    const token = f.messages.at(-1).token;
    if (stale === 'page') f.svg.children = [];
    else if (stale === 'svg') f.doc.hookedElem.firstElementChild = { ...f.svg };
    else if (stale === 'renderer') f.container.documents = [{ impl: { ...f.doc } }];
    else if (stale === 'missing renderer') f.container.documents = [];
    else if (stale === 'handler') f.doc.hookedElem.sourceMappingHandler = () => assert.fail('replacement handler invoked');
    else if (stale === 'dummy page') f.attrs['data-dummy'] = '1';
    else if (stale === 'zoom') f.doc.currentScaleRatio = 1.2;
    else if (stale === 'scroll') f.scroll.y = 15;
    else if (stale === 'resize') f.scroll.clientWidth = 700;
    else if (stale === 'page transform') f.matrix.e = 1;
    else if (stale === 'page size') f.attrs['data-page-height'] = '810';
    else if (stale === 'page identity') f.attrs['data-page-number'] = '2';
    assert.equal(f.revealSource(token), false);
    assert.equal(f.revealSource(token), false, 'matching requests are retired even when their page becomes stale');
    assert.equal(f.calls.length, 0);
  });
}

test('unmapped pages, unsupported handlers and invalid click positions retain ordinary browser context menus', () => {
  const cases = [
    f => ({ target: null }),
    f => ({ target: {} }),
    f => ({ target: { ...f.target, isConnected: false } }),
    f => ({ clientX: 601 }),
    f => ({ clientY: 801 }),
    f => { f.matrix.a = 0; return {}; },
    f => { f.matrix.e = NaN; return {}; },
    f => ({ target: { ...f.target, parentElement: null } }),
    f => { f.attrs['data-dummy'] = '1'; return {}; },
    f => { f.attrs['data-page-width'] = '0'; return {}; },
    f => { f.attrs['data-page-number'] = '-1'; return {}; },
    f => { delete f.attrs['data-page-number']; return {}; },
    f => { f.attrs['data-page-number'] = ' '; return {}; },
    f => { delete f.doc.hookedElem.sourceMappingHandler; return {}; },
    f => { f.doc.renderMode = 'canvas'; return {}; },
    () => ({ clientX: NaN }),
    () => ({ clientX: Infinity }),
    () => ({ clientX: '10' }),
    () => ({ clientX: -1 }),
    () => ({ clientY: -1 }),
    () => ({ clientY: NaN }),
    () => ({ clientY: Infinity }),
  ];
  for (const invalidate of cases) {
    const f = sourceContextFixture();
    const event = f.event('contextmenu', { target: f.target, clientX: 10, clientY: 20, ...invalidate(f) });
    assert.equal(event.prevented, false);
    assert.equal(event.stopped, false);
    assert.equal(f.messages.length, 0);
    assert.equal(f.calls.length, 0);
  }
});

test('a text-node target is normalized safely and later ordinary context menus retire its request', () => {
  const f = sourceContextFixture();
  f.event('contextmenu', { target: { nodeType: 3, parentElement: f.target }, clientX: 10, clientY: 20 });
  assert.equal(f.revealSource(f.messages.at(-1).token), true);
  assert.equal(f.calls[0].event.target, f.page);
  f.event('contextmenu', { target: f.target, clientX: 10, clientY: 20 });
  const token = f.messages.at(-1).token;
  const event = f.event('contextmenu', { target: null, clientX: 10, clientY: 20 });
  assert.equal(event.prevented, false);
  assert.equal(f.revealSource(token), false);
  assert.equal(f.calls.length, 1);
});


test('source mapping hook failures retire the request and never escape as menu errors', async () => {
  for (const asynchronous of [false, true]) {
    const f = sourceContextFixture();
    let calls = 0;
    f.doc.hookedElem.sourceMappingHandler = () => {
      calls++;
      if (asynchronous) return Promise.reject(Error('socket closed'));
      throw Error('socket closed');
    };
    f.event('contextmenu', { target: f.target, clientX: 10, clientY: 20 });
    const token = f.messages.at(-1).token;
    assert.equal(f.revealSource(token), asynchronous);
    assert.equal(f.revealSource(token), false);
    await Promise.resolve();
    assert.equal(calls, 1);
  }
});


for (const partial of [false, true]) for (const change of ['new', 'diff-v1', 'reset', 'dispose', 'viewport-change']) {
  test(`${partial ? 'partial' : 'full'} source context survives only viewport updates, never ${change} retirement`, () => {
    const f = partialFixture({ partial, decorated: true });
    f.start(); f.flush(); f.flush();
    const svg = f.doc.hookedElem.firstElementChild;
    svg.isConnected = true;
    svg.contains = target => {
      for (let node = target; node; node = node.parentElement) if (node === svg) return true;
      return false;
    };
    const page = svg.children.find(node => node.classList.contains('typst-page'));
    const target = { isConnected: true, parentElement: page, closest: () => page };
    let calls = 0;
    f.doc.hookedElem.sourceMappingHandler = () => calls++;
    f.event('contextmenu', { target, clientX: 100, clientY: 150 });
    const token = f.messages.at(-1).token;
    if (change === 'dispose') f.doc.disposeList.forEach(dispose => dispose());
    else if (change === 'reset') {
      f.doc.reset();
      f.doc.moduleInitialized = true;
    } else f.doc.addChangement([change, []]);
    assert.equal(f.doc.hookedElem.firstElementChild, svg, 'the request must retire before a same-SVG source patch renders');
    assert.equal(f.revealSource(token), change === 'viewport-change');
    assert.equal(calls, change === 'viewport-change' ? 1 : 0);
    assert.equal(f.revealSource(token), false);
  });
}


for (const partial of [false, true]) test(`${partial ? 'partial' : 'full'} source context maps the visible page when WebKit reports an old page target`, () => {
  const f = sourceContextFixture(partial);
  const attrs = { 'data-page-number': '2', 'data-page-width': '600', 'data-page-height': '800' };
  const oldPage = { tagName:'g', parentElement:f.svg, classList:{contains:name=>name==='typst-page'},
    getAttribute:name=>attrs[name] ?? null, getScreenCTM:()=>({a:1,b:0,c:0,d:1,e:0,f:1620}) };
  f.svg.children.push(oldPage);
  f.target.parentElement = oldPage;
  f.target.closest = () => oldPage;
  const event = f.event('contextmenu', {target:f.target, clientX:123.25, clientY:287.5});
  assert.ok(event.prevented && event.stopped);
  const token = f.messages.at(-1).token;
  // Partial rendering may discard the stale hit-test element before the native menu is chosen.
  f.target.isConnected = false;
  f.svg.children = [f.page];
  assert.equal(f.revealSource(token), true);
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].event.target, f.page);
  assert.equal(f.calls[0].event.clientX, 123.25);
  assert.equal(f.calls[0].event.clientY, 287.5);
});

test('source context resolves transformed page paper and excludes inter-page gaps', () => {
  const f = sourceContextFixture();
  Object.assign(f.matrix, {a:2,b:0,c:.25,d:2,e:10,f:30});
  f.event('contextmenu', {target:f.target, clientX:130, clientY:190});
  assert.equal(f.revealSource(f.messages.at(-1).token), true);
  assert.equal(f.calls[0].event.target, f.page);
  const outside = f.event('contextmenu', {target:f.target, clientX:5, clientY:190});
  assert.equal(outside.prevented, false);
  Object.assign(f.matrix, {a:1,b:0,c:0,d:1,e:0,f:810});
  const gap = f.event('contextmenu', {target:f.target, clientX:100, clientY:805});
  assert.equal(gap.prevented, false);
});


for (const residue of [0, 0.25, 1]) test(`full source navigation smooth scroll survives cursor repaint with ${residue}px layout residue`, () => {
  let changed = false;
  const f = partialFixture({partial:false,decorated:true,
    pages:[{width:200,height:240},{width:200,height:240},{width:200,height:240}],
    layoutHook() { if (changed) { f.scroll.scrollLeft += residue; f.scroll.scrollTop += residue; } }});
  f.start(); f.flush(); f.flush();
  let smoothTarget = null, corrections = 0, anchorsCleared = 0;
  const scrollBy = f.scroll.scrollBy.bind(f.scroll);
  f.scroll.scrollBy = (x,y) => { corrections++; smoothTarget = null; scrollBy(x,y); };
  f.scroll.scrollTo = options => { smoothTarget = options.behavior === 'smooth' ? options.top : null; };
  f.doc.clearSvgResizeAnchor = () => { anchorsCleared++; };
  f.scroll.scrollTo({top:1380,behavior:'smooth'});
  changed = true;
  f.doc.cursorPosition = [3,20,20];
  f.doc.addViewportChange(); f.flush(); f.flush();
  assert.equal(smoothTarget,1380,'normal cursor repaint must not cancel pending source navigation');
  assert.equal(corrections,0,'unchanged paper layout must not interrupt smooth scroll progress');
  assert.ok(anchorsCleared > 0,'a stale frontend resize anchor must still be retired');
});

test('full resize still corrects a visible geometry shift', () => {
  let changed = false;
  const f = partialFixture({partial:false,decorated:true,
    layoutHook() { if (changed) { f.scroll.scrollLeft += 2; f.scroll.scrollTop += 3; } }});
  f.start(); f.flush(); f.flush();
  let corrections = 0;
  const scrollBy = f.scroll.scrollBy.bind(f.scroll);
  f.scroll.scrollBy = (x,y) => { corrections++; scrollBy(x,y); };
  f.scroll.clientWidth = 450;
  changed = true;
  f.event('resize'); f.flush();
  assert.ok(corrections > 0,'visible geometry corrections must still reach the scroll view');
  assert.equal(f.scroll.scrollLeft,0);
  assert.equal(f.scroll.scrollTop,0);
});


const defaultRevealOptions = Object.freeze({enabled:true,style:'Rings',duration_ms:900,size_percent:100});

function revealFixture(options = {}) {
  const calls = [], snapshots = {invalidates:0, settles:0};
  const f = partialFixture({pages:[{width:600,height:300},{width:600,height:300}],
    zoomSnapshot:{attach(){},invalidate(){snapshots.invalidates++;},settle(){snapshots.settles++;},rendered(){}},
    ...options,
    configure(doc) {
      doc.windowElem.handleTypstLocation = function(...args) { calls.push({owner:this,args}); };
      options.configure?.(doc);
    },
  });
  f.start(); f.flush(); f.flush(); f.flush();
  assert.equal(f.frames.size,0);
  return {...f,calls,snapshots,
    get propertyWrites(){return f.propertyWrites;},get layouts(){return f.layouts;},get reads(){return f.reads;},
    locate(page=0,x=100,y=100){f.doc.windowElem.handleTypstLocation(f.doc.hookedElem.firstElementChild,page+1,x,y);},
    beacon(){return f.body.children.find(node=>node.className==='tiptoptyp-reveal-beacon');},
  };
}

for (const partial of [false,true]) {
  test(`${partial?'partial':'full'} explicit native location has a centered finite beacon without changing frontend navigation`, () => {
    const f=revealFixture({partial});
    f.revealIntent(true);
    assert.equal(f.revealActive(),true);
    assert.deepEqual(f.revealState().center,null);
    assert.equal(f.revealState().pending,true);
    f.locate(0,100,80); f.flush();
    assert.deepEqual({...f.revealState(),debug:undefined},{active:true,pending:false,page:0,center:[123,117],viewport:[0,0,646,374],elapsed_ms:0,options:defaultRevealOptions,debug:undefined});
    assert.equal(f.beacon().style.left,'123px');
    assert.equal(f.beacon().style.top,'117px');
    assert.equal(f.beacon().children.length,4);
    assert.equal(f.calls.length,1);
    assert.equal(f.calls[0].owner,f.doc.windowElem);
    assert.deepEqual(f.calls[0].args.slice(1),[1,100,80]);
    assert.equal(f.scroll.scrollTop,0);
    assert.equal(f.frames.size,1);
    f.advance(899); f.flush();
    assert.equal(f.revealActive(),true);
    f.advance(1); f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.beacon(),undefined);
    assert.equal(f.frames.size,0);
    assert.ok(f.snapshots.invalidates>=2);
    assert.ok(f.snapshots.settles>=1);
    assert.deepEqual({...f.revealState(),debug:undefined},{active:false,pending:false,page:null,center:null,viewport:[0,0,646,374],elapsed_ms:0,options:defaultRevealOptions,debug:undefined});
  });
}

for (const partial of [false,true]) for (const pending of [false,true]) {
  test(`${partial?'partial':'full'} ${pending?'pending':'visible'} beacon observations expire even when animation frames are delayed`, () => {
    const f=revealFixture({partial});
    f.revealIntent(true);
    if(!pending){f.locate();f.flush();}
    const limit=pending?3000:900;
    f.advance(limit-1);
    assert.equal(f.revealActive(),true);
    const beacon=f.beacon(),snapshots={...f.snapshots};
    assert.equal(f.frames.size,1);
    f.advance(1);
    const inactive={active:false,pending:false,page:null,center:null,viewport:[0,0,646,374],elapsed_ms:0,options:defaultRevealOptions,debug:undefined};
    for(let index=0;index<100;index++){
      assert.equal(f.revealActive(),false);
      assert.deepEqual({...f.revealState(),debug:undefined},inactive);
    }
    assert.equal(f.beacon(),beacon,'read-only queries leave DOM cleanup to the next animation frame');
    assert.deepEqual(f.snapshots,snapshots,'read-only queries cannot trigger snapshot work');
    assert.equal(f.frames.size,1);
    f.flush();
    assert.equal(f.beacon(),undefined);
    assert.equal(f.frames.size,0,'the eventual cleanup cannot schedule another frame');
    assert.deepEqual({...f.revealState(),debug:undefined},inactive);
    const retired={...f.snapshots};
    for(let index=0;index<100;index++){
      assert.equal(f.revealActive(),false);f.revealState();
    }
    assert.deepEqual(f.snapshots,retired);assert.equal(f.frames.size,0);
  });
}

test('automatic follow keeps the same location hook silent and cancels a previous beacon', () => {
  const f=revealFixture();
  const before={...f.snapshots};
  f.revealIntent(false); f.locate(); f.flush();
  assert.deepEqual(f.snapshots,before);
  assert.equal(f.revealActive(),false);
  assert.equal(f.frames.size,0);
  f.revealIntent(true); f.locate(); f.flush();
  assert.ok(f.beacon());
  f.revealIntent(false); f.locate(); f.flush();
  assert.equal(f.calls.length,3);
  assert.equal(f.beacon(),undefined);
  assert.equal(f.revealActive(),false);
  assert.equal(f.frames.size,0);
});

test('manual location actions arm a beacon and preserve the page-local destination', () => {
  const f=revealFixture();
  f.action({action:'location',value:{page:0,x:145,y:65}}); f.flush();
  assert.deepEqual(f.revealState().center,[168,102]);
  assert.equal(f.revealState().page,0);
  assert.deepEqual(f.calls[0].args.slice(1),[1,145,65]);
});

test('beacons wait for the real destination to enter view and track it during smooth scroll', () => {
  const f=revealFixture({partial:false});
  f.revealIntent(true); f.locate(1,140,150); f.flush();
  assert.equal(f.revealState().pending,true);
  assert.equal(f.revealState().center,null);
  assert.equal(f.beacon(),undefined);
  f.advance(240); f.scroll.scrollTop=220; f.flush();
  assert.equal(f.revealState().pending,false);
  assert.deepEqual(f.revealState().center,[163,277]);
  f.scroll.scrollTop=260; f.advance(100); f.flush();
  assert.deepEqual(f.revealState().center,[163,237]);
  assert.equal(f.beacon().style.top,'237px');
  f.advance(800); f.flush();
  assert.equal(f.revealActive(),false);
  assert.equal(f.frames.size,0);
});

test('pending locations expire within three seconds, including absent callbacks and invisible targets', () => {
  for (const callback of [false,true]) {
    const f=revealFixture();
    f.revealIntent(true);
    if(callback)f.locate(1,100,280);
    f.advance(2999); f.flush();
    assert.equal(f.revealActive(),true);
    assert.equal(f.revealState().pending,true);
    f.advance(1); f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.frames.size,0);
    assert.equal(f.beacon(),undefined);
  }
});

test('beacons project the complete page matrix, including skew and nonuniform scale', () => {
  const f=revealFixture({partial:false});
  const page=f.doc.hookedElem.firstElementChild.children.find(node=>node.classList.contains('typst-page'));
  page.getScreenCTM=()=>({a:1.2,b:0.2,c:0.15,d:0.9,e:30,f:40});
  f.revealIntent(true); f.locate(0,100,80); f.flush();
  assert.deepEqual(f.revealState().center,[162,132]);
});

test('viewport patches rebind the active beacon to the new SVG without restarting its lifetime', () => {
  const f=revealFixture();
  f.revealIntent(true); f.locate(); f.flush();
  const first=f.doc.hookedElem.firstElementChild;
  f.advance(300);
  f.doc.addViewportChange(); f.flush();
  assert.notEqual(f.doc.hookedElem.firstElementChild,first);
  assert.deepEqual(f.revealState().center,[123,137]);
  assert.equal(f.revealState().elapsed_ms,300);
  f.advance(600); f.flush();
  assert.equal(f.revealActive(),false);
  assert.equal(f.frames.size,0);
});

test('source replacement, reset, disposal and disconnection retire an active destination beacon', () => {
  for(const retire of [f=>f.doc.addChangement(['new',[{width:600,height:300}]]),f=>f.doc.reset(),
      f=>f.doc.disposeList.forEach(dispose=>dispose()),f=>{f.container.documents=[];},f=>{f.doc.hookedElem.isConnected=false;}]) {
    const f=revealFixture();
    f.revealIntent(true); f.locate(); f.flush();
    assert.ok(f.beacon());
    retire(f); f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.beacon(),undefined);
    assert.equal(f.frames.size,0);
  }
});

test('beacon styling is pointer transparent and preserves the hidden frontend ripple cleanup animation', () => {
  const f=revealFixture();
  f.event('DOMContentLoaded');
  const css=f.head.children[0].textContent;
  assert.match(css,/\.typst-jump-ripple\s*\{ visibility: hidden !important; pointer-events: none !important;/);
  assert.doesNotMatch(css,/\.typst-jump-ripple\s*\{[^}]*display:/);
  assert.match(css,/\.tiptoptyp-reveal-beacon\s*\{[^}]*position: fixed;[^}]*pointer-events: none;[^}]*var\(--tiptoptyp-reveal-accent\)/);
  assert.match(css,/animation: tiptoptyp-reveal-ring [^;]+ ease-out/);
  assert.match(css,/animation-delay:/);
});


for (const partial of [false,true]) test(`${partial?'partial':'full'} active beacon changes theme accent without restarting or rendering and identical palettes do no work`, () => {
  const f=revealFixture({partial});
  const palette=accent=>f.palette([255,255,255],[0,0,0],'1px solid transparent','1px solid transparent',accent);
  palette([79,140,255]);
  assert.equal(f.properties.get('--tiptoptyp-reveal-accent'),'rgb(79,140,255)');
  assert.equal(f.properties.get('--tiptoptyp-reveal-wash'),'rgba(79,140,255,0.2)');
  f.revealIntent(true); f.locate(); f.flush(); f.advance(300); f.flush();
  const beacon=f.beacon(),snapshots={...f.snapshots},layouts=f.layouts,reads=f.reads;
  const before=f.propertyWrites;
  palette([166,218,149]);
  assert.equal(f.propertyWrites-before,2,'accent changes only the two inherited beacon colors');
  assert.equal(f.properties.get('--tiptoptyp-reveal-accent'),'rgb(166,218,149)');
  assert.equal(f.properties.get('--tiptoptyp-reveal-wash'),'rgba(166,218,149,0.2)');
  assert.equal(f.beacon(),beacon);
  assert.equal(f.revealState().elapsed_ms,300);
  assert.deepEqual(f.snapshots,snapshots,'beacons are excluded from screenshot caches');
  assert.equal(f.layouts,layouts);assert.equal(f.reads,reads);
  const writes=f.propertyWrites;
  for(let index=0;index<100;index++)palette([166,218,149]);
  assert.equal(f.propertyWrites,writes);assert.deepEqual(f.snapshots,snapshots);
  assert.equal(f.frames.size,1,'only the existing finite animation remains scheduled');
  f.advance(600);f.flush();
  assert.equal(f.beacon(),undefined);assert.equal(f.frames.size,0);
  palette([166,218,149]);
  assert.equal(f.propertyWrites,writes);assert.equal(f.frames.size,0);
});

test('late frontend location hooks are installed by the next explicit intent without wrapper growth', () => {
  const f=revealFixture();
  let calls=0;
  const replacement=function(elem,page,x,y){calls++;assert.equal(this,f.doc.windowElem);return [elem,page,x,y];};
  f.doc.windowElem.handleTypstLocation=replacement;
  f.revealIntent(true);
  const wrapper=f.doc.windowElem.handleTypstLocation;
  const svg=f.doc.hookedElem.firstElementChild;
  assert.deepEqual(wrapper.call(f.doc.windowElem,svg,1,100,80).slice(1),[1,100,80]);
  f.flush();
  assert.deepEqual(f.revealState().center,[123,117]);
  f.revealIntent(true);
  assert.equal(f.doc.windowElem.handleTypstLocation,wrapper);
  f.locate(); f.flush();
  assert.equal(calls,2);
  assert.equal(f.frames.size,1);
});

test('frontend location exceptions remain observable and retire the pending beacon', () => {
  const expected=new Error('native frontend location failed');
  const f=revealFixture({configure(doc){doc.windowElem.handleTypstLocation=()=>{throw expected;};}});
  f.revealIntent(true);
  assert.throws(()=>f.locate(),error=>error===expected);
  assert.equal(f.revealActive(),false);
  assert.equal(f.frames.size,0);
});


test('navigation decoration diff arriving immediately after the real jump preserves its pending beacon', () => {
  for(const partial of [true,false]) {
    const f=revealFixture({partial});
    f.revealIntent(true); f.locate(0,100,80);
    f.doc.addChangement(['diff-v1',[{width:600,height:300},{width:600,height:300}]]);
    f.flush(); f.flush();
    assert.equal(f.revealActive(),true,'source-scroll decorations must not cancel their reveal');
    assert.deepEqual(f.revealState().center,[123,117]);
    f.advance(900); f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.frames.size,0);
  }
});


test('incremental patches rebind the revealed page matrix without extending its lifetime', () => {
  for(const partial of [true,false]) {
    const f=revealFixture({partial});
    f.scroll.scrollTop=250;
    f.revealIntent(true); f.locate(1,100,80); f.flush();
    assert.deepEqual(f.revealState().center,[123,177]);
    f.advance(300);
    f.doc.addChangement(['diff-v1',[{width:600,height:400},{width:600,height:300}]]);
    f.flush(); f.flush();
    assert.equal(f.revealActive(),true);
    assert.deepEqual(f.revealState().center,[123,277]);
    assert.equal(f.revealState().elapsed_ms,300);
    f.advance(600); f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.frames.size,0);
  }
});


for (const partial of [false,true]) {
  test(`${partial?'partial':'full'} reveal options can disable feedback while preserving the native location action`, () => {
    const f=revealFixture({partial});
    assert.deepEqual(f.revealState().options,defaultRevealOptions);
    const options={...defaultRevealOptions,enabled:false};
    f.revealOptions(options);
    const snapshots={...f.snapshots},layouts=f.layouts,reads=f.reads;
    for(let index=0;index<100;index++){
      f.revealIntent(true);f.locate(0,100,80);f.flush();
      assert.equal(f.revealActive(),false);
      assert.equal(f.beacon(),undefined);
      assert.equal(f.frames.size,0);
    }
    assert.equal(f.calls.length,100,'turning off the effect must not suppress actual navigation');
    assert.deepEqual(f.snapshots,snapshots,'disabled effects do not touch screenshot caches');
    assert.equal(f.layouts,layouts);assert.equal(f.reads,reads);
    assert.deepEqual(f.revealState().options,options);
    f.revealOptions(defaultRevealOptions);
    f.revealIntent(true);f.locate();f.flush();
    assert.equal(f.revealActive(),true);
  });

  for(const options of [
    {enabled:true,style:'Rings',duration_ms:250,size_percent:50},
    {enabled:true,style:'Pulse',duration_ms:1100,size_percent:125},
    {enabled:true,style:'Spotlight',duration_ms:2000,size_percent:200},
  ]) test(`${partial?'partial':'full'} ${options.style} honors its configured duration and size with one finite animation`, () => {
    const f=revealFixture({partial});
    f.revealOptions(options);
    assert.deepEqual(f.revealState().options,options);
    f.revealIntent(true);f.locate(0,100,80);f.flush();
    assert.equal(f.revealActive(),true);
    assert.deepEqual(f.revealState().center,[123,117]);
    assert.equal(f.calls.length,1);
    assert.equal(f.frames.size,1);
    assert.equal(f.beacon().getAttribute('data-style'),options.style);
    f.advance(options.duration_ms-1);f.flush();
    assert.equal(f.revealActive(),true);
    f.advance(1);f.flush();
    assert.equal(f.revealActive(),false);
    assert.equal(f.beacon(),undefined);
    assert.equal(f.frames.size,0);
    const snapshots={...f.snapshots};
    for(let index=0;index<100;index++){f.revealState();f.revealActive();}
    assert.deepEqual(f.snapshots,snapshots);
    assert.equal(f.frames.size,0);
  });

  test(`${partial?'partial':'full'} changed reveal options retire pending and visible effects without restarting or rendering`, () => {
    const f=revealFixture({partial});
    for(const visible of [false,true]){
      f.revealOptions(defaultRevealOptions);
      f.revealIntent(true);
      if(visible){f.locate();f.flush();f.advance(200);f.flush();}
      assert.equal(f.revealActive(),true);
      const layouts=f.layouts,reads=f.reads;
      f.revealOptions({...defaultRevealOptions,style:'Spotlight',duration_ms:1500,size_percent:150});
      assert.equal(f.revealActive(),false);
      assert.equal(f.beacon(),undefined);
      assert.equal(f.frames.size,0);
      assert.equal(f.layouts,layouts);assert.equal(f.reads,reads);
      f.flush();assert.equal(f.frames.size,0,'changing options cannot arm a replacement cue');
    }
  });

  test(`${partial?'partial':'full'} equal reveal options do no work while idle or during an active cue`, () => {
    const f=revealFixture({partial});
    f.revealOptions(defaultRevealOptions);
    for(const active of [false,true]){
      if(active){f.revealIntent(true);f.locate();f.flush();f.advance(300);f.flush();}
      const beacon=f.beacon(),snapshots={...f.snapshots},writes=f.propertyWrites;
      const layouts=f.layouts,reads=f.reads,frames=f.frames.size;
      for(let index=0;index<100;index++)f.revealOptions({...defaultRevealOptions});
      assert.equal(f.beacon(),beacon);assert.deepEqual(f.snapshots,snapshots);
      assert.equal(f.propertyWrites,writes);assert.equal(f.layouts,layouts);assert.equal(f.reads,reads);
      assert.equal(f.frames.size,frames);
      if(active){assert.equal(f.revealState().elapsed_ms,300);f.advance(600);f.flush();}
    }
  });
}

test('reveal option bounds are sanitized and invalid styles use the default', () => {
  const f=revealFixture();
  f.revealOptions({...defaultRevealOptions,style:'impossible',duration_ms:0,size_percent:0});
  assert.deepEqual(f.revealState().options,{...defaultRevealOptions,duration_ms:250,size_percent:50});
  f.revealOptions({...defaultRevealOptions,duration_ms:9000,size_percent:9000});
  assert.deepEqual(f.revealState().options,{...defaultRevealOptions,duration_ms:2000,size_percent:200});
  f.revealOptions({...defaultRevealOptions,duration_ms:NaN,size_percent:Infinity});
  assert.deepEqual(f.revealState().options,defaultRevealOptions);
  assert.equal(f.revealActive(),false);assert.equal(f.frames.size,0);
});

test('reveal preferences set before DOM mount survive frontend startup without premature work', () => {
  let locations=0;
  const f=partialFixture({lateContainer:true,configure(doc){doc.windowElem.handleTypstLocation=()=>locations++;}});
  const options={enabled:true,style:'Spotlight',duration_ms:1500,size_percent:175};
  const writes=f.propertyWrites,layouts=f.layouts,reads=f.reads;
  f.revealOptions(options);
  assert.deepEqual(f.revealState().options,options);
  assert.equal(f.body.children.length,0);assert.equal(f.head.children.length,0);
  assert.equal(f.propertyWrites,writes);assert.equal(f.layouts,layouts);assert.equal(f.reads,reads);
  f.event('DOMContentLoaded');f.start();f.flush();f.flush();f.flush();
  assert.deepEqual(f.revealState().options,options);
  f.revealIntent(true);
  f.doc.windowElem.handleTypstLocation(f.doc.hookedElem.firstElementChild,1,100,80);
  f.flush();
  const beacon=f.body.children.find(node=>node.className==='tiptoptyp-reveal-beacon');
  assert.equal(beacon.getAttribute('data-style'),'Spotlight');
  assert.equal(locations,1);assert.equal(f.revealActive(),true);
  f.advance(1500);f.flush();
  assert.equal(f.revealActive(),false);assert.equal(f.frames.size,0);
});

for (const partial of [false,true]) test(`${partial?'partial':'full'} failed queued zoom releases its serial owner and the next input recovers`,()=>{
  let fail=false;
  const expected=Error('injected zoom layout failure');
  const f=partialFixture({partial,decorated:true,nativeSnapshot:true,layoutHook(){if(fail)throw expected;}});
  f.start();f.flush();f.flush();
  const cache=f.window.tiptoptypZoomSnapshot;cache.configure(true);
  fail=true;
  f.nativeZoom({revision:31,token:null,scale:1.2,x:120,y:90});
  assert.throws(()=>f.flush(),error=>error===expected,'render errors must remain observable');
  assert.equal(cache.inspect().active,false,'a failed RAF must release the serial request');
  const failures=f.messages.filter(m=>m.op==='render-failed');
  assert.equal(failures.length,1);
  assert.equal(failures[0].revision,31);
  assert.equal(failures[0].scale,f.doc.currentScaleRatio);
  fail=false;
  f.nativeZoom({revision:32,token:null,scale:1.3,x:120,y:90});
  for(let i=0;i<6;i++)f.flush();
  assert.equal(cache.inspect().active,false);
  assert.equal(f.messages.filter(m=>m.op==='ready').at(-1).revision,32);
  assert.equal(f.frames.size,0,'no retry or idle render loop');
});

test('partial patch failures release native zoom without hiding the reported error',()=>{
  const f=partialFixture({decorated:true,nativeSnapshot:true});f.start();f.flush();f.flush();
  const cache=f.window.tiptoptypZoomSnapshot;cache.configure(true);
  // A real scroll element clamps zoom-out movement at its document edges.
  for(const method of ['scrollTo','scrollBy']) {
    const original=f.scroll[method];
    f.scroll[method]=function(...args){original.apply(this,args);this.scrollTop=Math.max(0,this.scrollTop);this.scrollLeft=Math.max(0,this.scrollLeft);};
  }
  const render=f.doc.r.rerender, expected=Error('injected zoom patch failure');
  f.doc.r.rerender=()=>{throw expected;};
  f.nativeZoom({revision:41,token:null,scale:.4,x:120,y:90});f.flush();
  assert.equal(f.errors.filter(error=>error===expected).length,1);
  assert.equal(cache.inspect().active,false);
  assert.equal(f.messages.filter(m=>m.op==='render-failed').at(-1).revision,41);
  assert.equal(f.doc.isRendering,false);
  assert.equal(f.frames.size,0);
  f.doc.r.rerender=render;
  f.nativeZoom({revision:42,token:null,scale:.5,x:120,y:90});
  for(let i=0;i<6;i++)f.flush();
  assert.equal(f.messages.filter(m=>m.op==='ready').at(-1)?.revision,42);
  assert.ok(f.visibleComplete());
});
