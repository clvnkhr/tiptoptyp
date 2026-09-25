// Opt-in test against the real pinned frontend. No production browser dependency.
// TIPTOPTYP_PLAYWRIGHT=/path/to/playwright TIPTOPTYP_TEST_TINYMIST=/path/to/tinymist
const { chromium } = require(process.env.TIPTOPTYP_PLAYWRIGHT || 'playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
(async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ttt-preview-e2e-'));
  const file = path.join(directory, 'main.typ');
  const source = '= First\n#link(<second>)[Continue]\n#pagebreak()\n= Second <second>\nSecond page.\n#pagebreak()\n= Third\nLast page.';
  fs.writeFileSync(file, source);
  const adapter = fs.readFileSync(path.join(__dirname, '../src/preview_navigation.js'), 'utf8');
  const executable = process.env.TIPTOPTYP_TEST_TINYMIST;
  assert.ok(executable, 'Set TIPTOPTYP_TEST_TINYMIST to the pinned binary');
  const server = spawn(executable, ['preview','--no-open','--root',directory,'--host','127.0.0.1:0','--data-plane-host','127.0.0.1:0','--control-plane-host','127.0.0.1:0','--invert-colors','never',file]);
  let log = '';
  let browser;
  const watchdog = setTimeout(() => { server.kill(); process.exitCode = 1; browser?.close(); }, 60000);
  try {
    await new Promise((resolve,reject) => {
      const read = chunk => { log += chunk; if (/Static file server listening on: ([\d.:]+)/.test(log) && /Control panel server listening on: ([\d.:]+)/.test(log)) resolve(); };
      server.stdout.on('data',read); server.stderr.on('data',read);
      server.on('exit',code=>reject(Error(`Tinymist exited ${code}: ${log}`)));
      setTimeout(()=>reject(Error(`Preview startup timed out: ${log}`)),10000).unref();
    });
    const url = `http://${log.match(/Static file server listening on: ([\d.:]+)/)[1]}`;
    const control = `ws://${log.match(/Control panel server listening on: ([\d.:]+)/)[1]}`;
    browser = await chromium.launch({ headless:true, ...(process.env.TIPTOPTYP_TEST_CHROME ? {executablePath:process.env.TIPTOPTYP_TEST_CHROME}: {}) });
    const page = await browser.newPage({viewport:{width:800,height:600}});
    const errors = []; page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(adapter);
    await page.goto(url);
    await page.waitForFunction(()=>document.getElementById('typst-container')?.documents?.[0]?.impl?.moduleInitialized);
    await page.evaluate(async control=> {
      window.control = new WebSocket(control);
      window.control.onmessage=e=>{ const message=JSON.parse(e.data); if(message.event==='outline') window.outline=message.items; };
      await new Promise(resolve=>window.control.onopen=resolve);
    },control);
    fs.appendFileSync(file,'\n');
    await page.waitForFunction(()=>window.outline?.length===3);
    await page.waitForTimeout(1500);
    const result = await page.evaluate(async () => {
      const doc = document.getElementById('typst-container').documents[0].impl;
      const scroll = doc.hookedElem.parentElement;
      const settle = () => new Promise(resolve=>setTimeout(resolve,1000));
      const before = scroll.scrollTop;
      const target = window.outline[1].position;
      window.tiptoptypPreviewAction({action:'location',value:{page:target.page_no-1,x:target.x,y:target.y}});
      await settle();
      const jumped = scroll.scrollTop;
      window.tiptoptypPreviewAction({action:'back'}); await settle(); const back = scroll.scrollTop;
      window.tiptoptypPreviewAction({action:'forward'}); await settle(); const forward = scroll.scrollTop;
      let rerenders=0; const original=doc.r.rerender; doc.r.rerender=(...args)=>{rerenders++;return original(...args);};
      const start=performance.now();
      for(let i=0;i<50;i++) window.tiptoptypSetPalette(i%2?[255,255,255]:[24,31,42],i%2?[0,0,0]:[200,210,220]);
      const paletteMs=performance.now()-start;
      const nodesBefore=doc.hookedElem.childElementCount;
      await settle();
      const idleRerenders=rerenders;
      window.tiptoptypPreviewAction({action:'zoom-in'}); await settle(); const zoom=doc.currentScaleRatio;
      window.tiptoptypPreviewAction({action:'fit'}); await settle(); const fit=doc.currentScaleRatio;
      const field=document.createElement('input'); document.body.append(field); field.focus();
      const inputBefore=scroll.scrollTop; field.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true})); await settle();
      return {before,jumped,back,forward,zoom,fit,paletteMs,idleRerenders,nodesBefore,nodesAfter:doc.hookedElem.childElementCount,inputBefore,inputAfter:scroll.scrollTop,htmlControls:document.getElementById('tiptoptyp-preview-controls')!==null};
    });
    assert.ok(result.jumped>result.before+100,JSON.stringify(result));
    assert.ok(Math.abs(result.back-result.before)<2,JSON.stringify(result));
    assert.ok(Math.abs(result.forward-result.jumped)<2,JSON.stringify(result));
    assert.ok(result.zoom>1); assert.equal(result.fit,1);
    assert.equal(result.idleRerenders,0); assert.equal(result.nodesBefore,result.nodesAfter);
    assert.equal(result.inputBefore,result.inputAfter); assert.equal(result.htmlControls,false);
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({browser:await browser.version(),platform:process.platform,arch:process.arch,viewport:[800,600],adapterSha256:crypto.createHash('sha256').update(adapter).digest('hex'),result},null,2));
  } finally {
    clearTimeout(watchdog); await browser?.close(); server.kill(); fs.rmSync(directory,{recursive:true,force:true});
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
