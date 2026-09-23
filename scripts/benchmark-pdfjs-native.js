// Body passed to WKWebView.callAsyncJavaScript by benchmark-pdfjs-native.swift.
// Files are preloaded to exclude compilation and PDF disk/network transfer.
const arrays = [a64, b64].map(s => Uint8Array.from(atob(s), c => c.charCodeAt(0)));
const pause = ms => new Promise(r => setTimeout(r, ms));
const frame = () => new Promise(r => requestAnimationFrame(r));
const summary = values => {
  const sorted = [...values].sort((a,b) => a-b);
  return { samples_ms: values, p50_ms: (sorted[14]+sorted[15])/2, p95_ms: sorted[28] };
};
// Older macOS WebKit lacks ReadableStream async iteration. Consume the
// public streaming API directly, as the viewer's text layer does.
async function textContent(page) {
  const reader=page.streamTextContent().getReader();
  const result={items:[],styles:{}};
  try { for(;;) { const {value,done}=await reader.read(); if(done)break;
    result.items.push(...value.items);Object.assign(result.styles,value.styles);
  }} finally { reader.releaseLock(); }
  return result;
}
const app = window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication;
const pages = app.pdfDocument.numPages;
const pageIndex = Math.floor(pages/2);
const results = { pages, page_index:pageIndex, scale:4.29, viewport:[innerWidth,innerHeight],
  device_pixel_ratio:devicePixelRatio, user_agent:navigator.userAgent,
  cold_host_ready_ms:coldMs, warmups:5, samples:30 };
// Match the native worker's 4.29 physical pixels per PDF point. At this
// zoom the viewport intersects only the requested middle page.
const viewerScale = 4.29/(devicePixelRatio*96/72);
app.pdfLinkService.setHash(`page=${pageIndex+1}&zoom=${viewerScale*100},0,450`);
await pause(1000);
if(app.pdfViewer.currentPageNumber!==pageIndex+1)throw Error(`Initial anchor is page ${app.pdfViewer.currentPageNumber}`);
const initialState = await (await fetch('../state.json')).json();
const originalFetch = window.fetch;
let currentBytes = arrays[0], revision = initialState.revision;
window.fetch = async (input, options) => {
  if (input === '../state.json') return new Response(JSON.stringify({...initialState, revision}));
  if (input === initialState.url) return new Response(currentBytes, {headers:{'Content-Type':'application/pdf'}});
  return originalFetch(input, options);
};
const staged = [], presented = [], blanks = [];
let firstStaged;
try {
  for (let i=0;i<35;i++) {
    currentBytes = arrays[(i+1)%2]; revision++;
    let blank=0, measuring=true;
    const sample = () => {
      if (!measuring) return;
      const v=window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication.pdfViewer;
      if (!v._getVisiblePages().views.some(({view})=>view.renderingState===3)) blank++;
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
    const start=performance.now();
    const previous=window.tiptoptypPdf.activeFrame;
    await window.tiptoptypPdf.refresh();
    const committed=performance.now()-start;
    if (window.tiptoptypPdf.activeFrame===previous) throw Error('No staged replacement');
    const next=window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication;
    if(next.pdfViewer.currentPageNumber!==pageIndex+1) throw Error(`Page moved: expected ${pageIndex+1}, got ${next.pdfViewer.currentPageNumber}`);
    // rAF is a presentation opportunity, not a scanout/display latency sensor.
    await frame(); await frame();
    const visible=performance.now()-start;
    measuring=false;
    const text=(await textContent(await next.pdfDocument.getPage(pageIndex+1))).items.map(x=>x.str).join(' ');
    if(!text.includes((i+1)%2 ? 'Updated freely' : 'Scroll freely')) throw Error('Stale PDF text after replacement');
    if(i===0)firstStaged=committed;
    if(i>=5){staged.push(committed);presented.push(visible);blanks.push(blank);}
    await pause(80);
  }
} finally { window.fetch=originalFetch; }
results.staged_commit={...summary(staged),first_ms:firstStaged,blank_frames:blanks};
results.staged_two_animation_frames=summary(presented);
const active=window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication;
results.visible_pages=active.pdfViewer._getVisiblePages().views.map(({id,view})=>({id,canvas:[view.canvas?.width,view.canvas?.height]}));
// Separately measure a persistent PDF.js engine. This bypasses the full viewer,
// making a useful engine/preparation comparison, not a shipping viewer timing.
const pdfjs=window.tiptoptypPdf.activeFrame.contentWindow.pdfjsLib;
pdfjs.GlobalWorkerOptions.workerSrc=new URL('../build/pdf.worker.mjs',location.href).href;
const worker=new pdfjs.PDFWorker();
let retained, retainedTask;
const canvas=document.createElement('canvas');
canvas.width=1802;canvas.height=2360;
const context=canvas.getContext('2d');
async function open(bytes) {
  const task=pdfjs.getDocument({data:bytes.slice(),worker,
    cMapUrl:new URL('./cmaps/',location.href).href,cMapPacked:true,
    standardFontDataUrl:new URL('./standard_fonts/',location.href).href,
    wasmUrl:new URL('./wasm/',location.href).href});
  const doc=await task.promise;retainedTask=task;
  // The PDFium worker builds the full page catalog and outline on each revision.
  for(let i=1;i<=doc.numPages;i++)(await doc.getPage(i)).getViewport({scale:1});
  await doc.getOutline();
  return doc;
}
results.engine=[];
for(const mode of ['retained_worker_changed_pdf','retained_worker_viewport']) {
  const times=[]; let first;
  for(let i=0;i<35;i++) {
    const start=performance.now();
    if(mode==='retained_worker_changed_pdf'||!retained){
      if(retained)await retainedTask.destroy();
      retained=await open(arrays[i%2]);
    }
    const page=await retained.getPage(pageIndex+1);
    const viewport=page.getViewport({scale:4.29});
    await page.render({canvasContext:context,viewport,transform:[1802/viewport.width,0,0,2360/viewport.height,0,0]}).promise;
    const text=await textContent(page);
    const annotations=await page.getAnnotations();
    // Force RGBA readback, matching the native worker's materialized pixel buffer.
    const rgba=context.getImageData(0,0,1802,2360);
    if(rgba.data.length!==1802*2360*4)throw Error('Bad raster size');
    if(!text.items.length)throw Error('No text');
    const elapsed=performance.now()-start;
    if(i===0)first=elapsed;
    if(i>=5)times.push(elapsed);
    results.text_items=text.items.length;results.annotations=annotations.length;
  }
  results.engine.push({mode,first_ms:first,...summary(times)});
}
await retainedTask.destroy();worker.destroy();
return JSON.stringify(results,null,2);
