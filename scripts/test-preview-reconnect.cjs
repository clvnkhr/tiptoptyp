// Owned headless browser/server regression for the pinned Tinymist reconnect path.
// Optional TIPTOPTYP_PREVIEW_RECONNECT_ADAPTER selects a preserved baseline.
const { spawn, execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');

const root = path.resolve(__dirname, '..');
const fixture = path.join(root, 'scripts/fixtures/tinymist-zoom-boundaries.typ');
const adapterPath = path.resolve(process.env.TIPTOPTYP_PREVIEW_RECONNECT_ADAPTER || path.join(root, 'src/preview_navigation.js'));
const executable = process.env.TIPTOPTYP_TEST_TINYMIST;
const viewport = { width: 900, height: 700 };
const sha256 = value => crypto.createHash('sha256').update(value).digest('hex');
const adapter = fs.readFileSync(adapterPath, 'utf8');
const original = fs.readFileSync(fixture, 'utf8');
const receipt = {
  started_utc: new Date().toISOString(),
  fixture: { path: fixture, sha256: sha256(original) },
  adapter: { path: adapterPath, sha256: sha256(adapter) },
  viewport, input: 'owned page WebSocket.close, control-channel source changes, and JavaScript adapter actions',
  not_established: 'native editor interaction, native snapshot composition, or physical trackpad input',
  lanes: [], passed: false,
};

const writeReceipt = () => {
  const destination = process.env.TIPTOPTYP_PREVIEW_RECONNECT_RECEIPT;
  if (destination) fs.writeFileSync(destination, JSON.stringify(receipt, null, 2) + '\n');
};

async function lane(chromium, mode) {
  const result = { requested_backend: `tinymist-${mode}`, partial: null, observations: [], errors: [], passed: false };
  receipt.lanes.push(result);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ttt-preview-reconnect-'));
  const file = path.join(directory, 'main.typ');
  fs.writeFileSync(file, original);
  const command = [executable, 'preview', '--no-open', `--partial-rendering=${mode === 'partial'}`,
    '--root', directory, '--host', '127.0.0.1:0', '--data-plane-host', '127.0.0.1:0',
    '--control-plane-host', '127.0.0.1:0', '--invert-colors', 'never', file];
  result.command = command;
  const server = spawn(command[0], command.slice(1));
  let log = '', browser, page;
  const watchdog = setTimeout(() => { server.kill(); browser?.close(); }, 60000);
  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(Error(`Tinymist startup timed out: ${log}`)), 20000);
      const receive = chunk => {
        log = (log + chunk).slice(-12000);
        if (/Static file server listening on: ([\d.:]+)/.test(log) && /Control panel server listening on: ([\d.:]+)/.test(log)) { clearTimeout(timeout); resolve(); }
      };
      server.stdout.on('data', receive); server.stderr.on('data', receive);
      server.on('error', error => { clearTimeout(timeout); reject(error); });
      server.on('exit', code => { clearTimeout(timeout); reject(Error(`Tinymist exited ${code}: ${log}`)); });
    });
    browser = await chromium.launch({ headless: true,
      ...(process.env.TIPTOPTYP_TEST_CHROME ? { executablePath: process.env.TIPTOPTYP_TEST_CHROME } : {}) });
    result.browser_version = browser.version();
    page = await browser.newPage({ viewport });
    page.on('pageerror', error => result.errors.push(error.message));
    await page.addInitScript(() => {
      // Expose the production frame reader without pretending to verify native pixels.
      window.tiptoptypZoomSnapshot = { attach: reader => window.readSnapshot = reader,
        rendered() {}, settle() {}, invalidate() {}, begin() {}, scrolled() {},
        request: () => false, resize: () => false, native: () => true };
      window.ipc = { postMessage() {} };
    });
    await page.addInitScript(adapter);
    await page.goto(`http://${log.match(/Static file server listening on: ([\d.:]+)/)[1]}`);
    const ready = expectedPages => page.waitForFunction(expected => {
      const doc = document.getElementById('typst-container')?.documents?.[0]?.impl;
      return doc?.moduleInitialized && !doc.isRendering && !doc.patchQueue.length &&
        doc.kModule.retrievePagesInfo().length === expected &&
        doc.hookedElem.firstElementChild?.querySelectorAll('g.typst-page').length === expected;
    }, expectedPages, { timeout: 15000 });
    const paint = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    result.phase = 'initial-render';
    await ready(48);
    await page.evaluate(async url => {
      window.reconnectControl = new WebSocket(url);
      await new Promise((resolve, reject) => { window.reconnectControl.onopen = resolve; window.reconnectControl.onerror = reject; });
    }, `ws://${log.match(/Control panel server listening on: ([\d.:]+)/)[1]}`);
    result.partial = await page.evaluate(() => document.getElementById('typst-container').documents[0].impl.partialRendering);
    assert.equal(result.partial, mode === 'partial', 'verify the actual frontend backend');
    for (let reconnect = 1; reconnect <= 2; reconnect++) {
      result.phase = `socket-reconnect-${reconnect}`;
      await page.evaluate(() => {
        const container = document.getElementById('typst-container');
        window.previousReconnectSocket = container.typstWebsocket;
        container.typstWebsocket.close(1000, 'isolated reconnect regression');
      });
      await page.waitForFunction(() => {
        const container = document.getElementById('typst-container');
        return container.typstWebsocket !== window.previousReconnectSocket &&
          container.typstWebsocket?.readyState === WebSocket.OPEN && container.documents.length > 0;
      }, null, { timeout: 15000 });
      result.phase = `reconnected-render-${reconnect}`;
      await ready(47 + reconnect);
      const source = original.replace('#let page-count = 48', `#let page-count = ${48 + reconnect}`);
      result.phase = `source-response-${reconnect}`;
      await page.evaluate(({ file, source }) => window.reconnectControl.send(JSON.stringify({
        event: 'updateMemoryFiles', files: { [file]: source },
      })), { file, source });
      await ready(48 + reconnect);
      const observation = await page.evaluate(({ reconnect, expectedPages }) => {
        const container = document.getElementById('typst-container'), doc = container.documents[0].impl;
        const scroll = doc.hookedElem.parentElement;
        return { reconnect, document_entries: container.documents.length,
          unique_renderers: new Set(container.documents.map(value => value.impl)).size,
          source_pages: doc.kModule.retrievePagesInfo().length, expected_pages: expectedPages,
          source_responded: doc.kModule.retrievePagesInfo().length === expectedPages,
          scale_before: doc.currentScaleRatio, scroll_before: scroll.scrollTop,
          target_page: reconnect * 2, snapshot_available: !!window.readSnapshot() };
      }, { reconnect, expectedPages: 48 + reconnect });
      observation.source_sha256 = sha256(source);
      await page.evaluate(pageIndex => window.tiptoptypPreviewAction({ action: 'page', value: pageIndex }), observation.target_page);
      await paint();
      Object.assign(observation, await page.evaluate(pageIndex => {
        const doc = document.getElementById('typst-container').documents[0].impl, scroll = doc.hookedElem.parentElement;
        const paper = [...doc.hookedElem.firstElementChild.querySelectorAll('g.typst-page')][pageIndex];
        const top = paper.getScreenCTM().f - scroll.getBoundingClientRect().top;
        return { navigation_worked: Math.abs(top) <= 1.1, target_paper_top: top, scroll_after: scroll.scrollTop };
      }, observation.target_page));
      await page.evaluate(() => window.tiptoptypPreviewAction({ action: 'zoom-in' }));
      await paint();
      observation.scale_after_zoom = await page.evaluate(() => document.getElementById('typst-container').documents[0].impl.currentScaleRatio);
      observation.zoom_worked = Math.abs(observation.scale_after_zoom - observation.scale_before * 1.1) < 1e-9;
      const scale = 1 + reconnect * .15;
      observation.native_request_accepted = await page.evaluate(({ reconnect, scale }) => window.tiptoptypNativeZoom({ revision: reconnect, scale, token: null }), { reconnect, scale });
      await paint();
      observation.scale_after_native = await page.evaluate(() => document.getElementById('typst-container').documents[0].impl.currentScaleRatio);
      observation.native_request_applied = observation.native_request_accepted && Math.abs(observation.scale_after_native - scale) < 1e-9;
      observation.passed = observation.document_entries === 1 && observation.unique_renderers === 1 &&
        observation.source_responded && observation.snapshot_available && observation.navigation_worked &&
        observation.zoom_worked && observation.native_request_applied;
      result.observations.push(observation); writeReceipt();
    }
    result.phase = 'complete';
    result.passed = result.errors.length === 0 && result.observations.every(observation => observation.passed);
    if (!result.passed) result.failure = 'Source updated after reconnect, but the adapter failed navigation, zoom, or native request acceptance.';
  } catch (error) {
    result.failure = String(error?.message || error); result.server_log = log;
    if (page && !page.isClosed()) result.failure_observation = await page.evaluate(() => {
      const container = document.getElementById('typst-container'), doc = container?.documents?.[0]?.impl;
      return { documents: container?.documents?.length, socket_state: container?.typstWebsocket?.readyState,
        initialized: doc?.moduleInitialized, rendering: doc?.isRendering, queued: doc?.patchQueue?.length,
        pages: doc?.moduleInitialized ? doc.kModule.retrievePagesInfo().length : null };
    }).catch(failure => ({ error: String(failure.message) }));
  } finally {
    clearTimeout(watchdog);
    await browser?.close(); server.kill();
    if (server.exitCode === null && server.signalCode === null) await new Promise(resolve => server.once('exit', resolve));
    fs.rmSync(directory, { recursive: true, force: true }); writeReceipt();
  }
  return result;
}

(async () => {
  assert.ok(executable, 'Set TIPTOPTYP_TEST_TINYMIST to the pinned sidecar');
  receipt.tinymist = { path: path.resolve(executable), sha256: sha256(fs.readFileSync(executable)),
    version: execFileSync(executable, ['--version'], { encoding: 'utf8' }).trim() };
  const { chromium } = require(process.env.TIPTOPTYP_PLAYWRIGHT || 'playwright');
  for (const mode of ['full', 'partial']) {
    const result = await lane(chromium, mode);
    console.log(`Real Tinymist ${mode} WebSocket reconnect: ${result.passed ? 'passed' : 'FAILED'} (${result.observations.length}/2 reconnect observations).`);
    if (result.failure) console.error(result.failure);
  }
  receipt.passed = receipt.lanes.length === 2 && receipt.lanes.every(result => result.passed);
  writeReceipt();
  if (!receipt.passed) process.exitCode = 1;
})().catch(error => { receipt.failure = String(error?.message || error); writeReceipt(); console.error(error); process.exitCode = 1; });
