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
    await page.addInitScript(() => {
      window.messages = [];
      window.ipc = { postMessage: message => window.messages.push(JSON.parse(message)) };
    });
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
    // Check rendered pixels, not merely CSS attributes: both paper and ink
    // must respond to appearance changes without rebuilding the document.
    const assertPalette = async (bg,fg) => {
      await page.evaluate(([bg,fg]) => window.tiptoptypSetPalette(bg,fg),[bg,fg]);
      const placement = await page.evaluate(() => ({
        host: getComputedStyle(document.getElementById('typst-container')).filter,
        page: getComputedStyle(document.querySelector('#typst-container g[data-page-width]')).filter,
        paper: getComputedStyle(document.querySelector('#typst-container .typst-page-inner')).fill
      }));
      assert.equal(placement.host, 'none', 'never filter the HTML host: WKWebView snapshots hide its compositor failure');
      assert.ok(placement.page.includes('#tiptoptyp-palette'), JSON.stringify(placement));
      assert.equal(placement.paper, `rgb(${bg.join(', ')})`);
      const png = await page.screenshot();
      const counts = await page.evaluate(async ({png,bg,fg}) => {
        const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${png}`)).blob());
        const canvas = document.createElement('canvas'); canvas.width=bitmap.width; canvas.height=bitmap.height;
        const ctx=canvas.getContext('2d'); ctx.drawImage(bitmap,0,0); bitmap.close();
        const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
        let paper=0,ink=0;
        for(let i=0;i<pixels.length;i+=4) {
          const matches=color=>color.every((c,j)=>Math.abs(pixels[i+j]-c)<=3);
          if(matches(bg)) paper++; if(matches(fg)) ink++;
        }
        return {paper,ink};
      },{png:png.toString('base64'),bg,fg});
      assert.ok(counts.paper>300000,`paper palette ${bg}: ${JSON.stringify(counts)}`);
      assert.ok(counts.ink>10,`text palette ${fg}: ${JSON.stringify(counts)}`);
    };
    for (const [bg,fg] of [[[0,0,0],[255,255,255]],[[24,31,42],[200,210,220]],[[245,239,228],[42,35,26]],[[255,255,255],[0,0,0]]]) {
      await assertPalette(bg,fg);
    }
    // Actual document clicks exercise the capture handler and the pinned
    // frontend together; directly invoking our location action cannot do that.
    await page.locator('svg a').first().click();
    await page.waitForFunction(() => window.messages.at(-1)?.page === 1 && window.messages.at(-1)?.back);
    await page.evaluate(() => window.tiptoptypPreviewAction({action:'back'}));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 0 && window.messages.at(-1)?.forward);
    await page.evaluate(() => window.tiptoptypPreviewAction({action:'forward'}));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 1);
    // A page button after an internal link must retire that link's resize
    // anchor. Otherwise the next layout can pull us back to the old link.
    await page.evaluate(() => window.tiptoptypPreviewAction({action:'page',value:2}));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 2);
    await page.setViewportSize({width:760,height:600});
    await page.waitForTimeout(500);
    assert.equal(await page.evaluate(() => window.messages.at(-1)?.page), 2, 'resizing must retain page-button navigation');
    await page.setViewportSize({width:800,height:600});
    await page.waitForTimeout(500);
    await page.evaluate(() => window.tiptoptypPreviewAction({action:'page',value:0}));
    await page.waitForFunction(() => window.messages.at(-1)?.page === 0);
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
      let rerenders=0; const original=doc.r.rerender; doc.r.rerender=(...args)=>{rerenders++;return original.apply(doc.r,args);};
      const start=performance.now();
      for(let i=0;i<50;i++) window.tiptoptypSetPalette(i%2?[255,255,255]:[24,31,42],i%2?[0,0,0]:[200,210,220]);
      const paletteMs=performance.now()-start;
      const nodesBefore=doc.hookedElem.childElementCount;
      await settle();
      const idleRerenders=rerenders;
      window.tiptoptypPreviewAction({action:'zoom-in'}); await settle(); const zoom=doc.currentScaleRatio;
      window.tiptoptypPreviewAction({action:'fit'}); await settle(); const fit=doc.currentScaleRatio;
      const field=document.createElement('input'); field.id='test-query'; document.body.append(field); field.focus();
      const inputBefore=scroll.scrollTop; field.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true})); await settle();
      return {before,jumped,back,forward,zoom,fit,paletteMs,idleRerenders,nodesBefore,nodesAfter:doc.hookedElem.childElementCount,inputBefore,inputAfter:scroll.scrollTop,htmlControls:document.getElementById('tiptoptyp-preview-controls')!==null};
    });
    assert.ok(result.jumped>result.before+100,JSON.stringify(result));
    assert.ok(Math.abs(result.back-result.before)<2,JSON.stringify(result));
    assert.ok(Math.abs(result.forward-result.jumped)<2,JSON.stringify(result));
    assert.ok(result.zoom>1); assert.equal(result.fit,1);
    assert.equal(result.idleRerenders,0); assert.equal(result.nodesBefore,result.nodesAfter);
    assert.equal(result.inputBefore,result.inputAfter); assert.equal(result.htmlControls,false);
    const field = page.locator('#test-query');
    await field.fill('before');
    await field.press(process.platform === 'darwin' ? 'Meta+A' : 'Control+A');
    await field.pressSequentially('tjk');
    assert.equal(await field.inputValue(), 'tjk', 'viewer shortcuts must not consume text edits');
    assert.equal(await page.evaluate(() => window.messages.filter(m=>m.type==='invert-preview').length), 0);
    await field.blur();
    await page.keyboard.press('t');
    await page.waitForFunction(() => window.messages.some(m=>m.type==='invert-preview'));
    // The standalone control connection, like an editor, supplies unsaved text.
    // CLI disk watching is not the update path under test.
    await page.evaluate(({file, source}) => window.control.send(JSON.stringify({
      event:'updateMemoryFiles', files:{[file]:source}
    })), {file, source:source+'\n#pagebreak()\n= Fourth\nUpdated preview.'});
    await page.waitForFunction(()=>window.outline?.length===4 && window.messages.some(m=>m.type==='preview-state' && m.count===4)).catch(async error => {
      throw Error(`${error.message}\n${JSON.stringify({errors,state:await page.evaluate(()=>({outline:window.outline,messages:window.messages.slice(-8)})),log:log.slice(-2000)})}`);
    });
    await page.evaluate(() => {
      const position = window.outline[3].position;
      window.tiptoptypPreviewAction({action:'location',value:{page:position.page_no-1,x:position.x,y:position.y}});
    });
    await page.waitForFunction(() => window.messages.at(-1)?.page === 3);
    await assertPalette([24,31,42],[200,210,220]);
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({browser:await browser.version(),platform:process.platform,arch:process.arch,viewport:[800,600],adapterSha256:crypto.createHash('sha256').update(adapter).digest('hex'),scenarios:['internal link/back/forward','page navigation across resize','compiled outline','zoom/fit','rendered light/dark palette without rerender','text editing and viewer shortcuts','live source update'],result},null,2));
  } finally {
    clearTimeout(watchdog); await browser?.close(); server.kill(); fs.rmSync(directory,{recursive:true,force:true});
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
