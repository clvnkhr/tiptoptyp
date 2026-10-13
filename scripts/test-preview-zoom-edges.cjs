// Real pinned Tinymist edge geometry. Headless, owned browser/server only.
// The snapshot notification stub exposes the production geometry reader;
// it does not fake document layout, scroll clamping or SVG rendering.
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const root = path.resolve(__dirname, '..');
const fixture = path.join(__dirname, 'fixtures/tinymist-zoom-stress.typ');
const read = file => fs.readFileSync(file, 'utf8');
async function lane(mode) {
  const { chromium } = require(process.env.TIPTOPTYP_PLAYWRIGHT || 'playwright');
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ttt-zoom-edges-'));
  const file = path.join(directory, 'main.typ');
  fs.copyFileSync(fixture, file);
  const executable = process.env.TIPTOPTYP_TEST_TINYMIST;
  assert.ok(executable, 'Set TIPTOPTYP_TEST_TINYMIST to the pinned sidecar');
  const server = spawn(executable, ['preview', '--no-open', `--partial-rendering=${mode === 'partial'}`,
    '--root', directory, '--host', '127.0.0.1:0', '--data-plane-host', '127.0.0.1:0',
    '--control-plane-host', '127.0.0.1:0', '--invert-colors', 'never', file]);
  let log = '', browser;
  const watchdog = setTimeout(() => { server.kill(); browser?.close(); }, 180000);
  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(Error(`Startup timed out: ${log.slice(-2000)}`)), 20000);
      const receive = chunk => {
        log = (log + chunk).slice(-16000);
        if (/Static file server listening on: ([\d.:]+)/.test(log)) { clearTimeout(timeout); resolve(); }
      };
      server.stdout.on('data', receive); server.stderr.on('data', receive);
      server.on('error', error => { clearTimeout(timeout); reject(error); });
      server.on('exit', code => { clearTimeout(timeout); reject(Error(`Sidecar exited ${code}`)); });
    });
    browser = await chromium.launch({headless: true,
      ...(process.env.TIPTOPTYP_TEST_CHROME ? {executablePath: process.env.TIPTOPTYP_TEST_CHROME} : {})});
    const page = await browser.newPage({viewport: {width: 900, height: 700}});
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.tiptoptypZoomSnapshot = {attach: fn => window.readSnapshot = fn, rendered() {}, settle() {},
        invalidate() {}, begin: () => false, request: () => false, resize: () => false, scrolled() {}, native: () => true};
      window.ipc = {postMessage() {}};
    });
    await page.addInitScript(read(path.join(root, 'src/preview_navigation.js')));
    await page.goto(`http://${log.match(/Static file server listening on: ([\d.:]+)/)[1]}`);
    await page.waitForFunction(() => document.getElementById('typst-container')?.documents?.[0]?.impl?.moduleInitialized);
    await page.waitForTimeout(3000);
    assert.equal(await page.evaluate(() => readSnapshot().doc.partialRendering), mode === 'partial');
    const snapshot = () => page.evaluate(() => {
      const f = readSnapshot();
      return {geometry: f.geometry, scale: f.scale, fitWidth: f.fitWidth};
    });
    const cases = [];
    for (const edge of ['top', 'bottom']) for (const scale of [.95, 1.1, .5]) for (const anchor of [[5, 5], [895, 695]]) {
      await page.evaluate(() => tiptoptypPreviewAction({action: 'fit'}));
      await page.waitForTimeout(400);
      await page.evaluate(edge => readSnapshot().doc.hookedElem.parentElement.scrollTo(1e8, edge === 'bottom' ? 1e9 : 0), edge);
      await page.waitForTimeout(600);
      const before = await snapshot();
      assert.equal(before.scale, 1);
      assert.equal(await page.evaluate(({scale, anchor}) => tiptoptypNativeZoom({revision: 1, scale,
        x: anchor[0], y: anchor[1], panX: 0, panY: 0}), {scale, anchor}), true);
      await page.waitForTimeout(700);
      const after = await snapshot();
      assert.equal(after.scale, scale, `${mode} ${edge} ${scale} at ${anchor}`);
      cases.push({mode, edge, scale, anchor, before, after});
    }
    assert.deepEqual(errors, []);
    return cases;
  } finally {
    clearTimeout(watchdog);
    await browser?.close(); server.kill();
    await new Promise(resolve => { if (server.exitCode !== null || server.signalCode !== null) resolve(); else server.once('exit', resolve); });
    fs.rmSync(directory, {recursive: true, force: true});
  }
}
(async () => {
  const cases = [...await lane('full'), ...await lane('partial')];
  const output = {tinymist: '0.15.2', fixture_sha256: crypto.createHash('sha256').update(fs.readFileSync(fixture)).digest('hex'), viewport: [900, 700], cases};
  const reference = JSON.parse(read(path.join(root, 'tests/fixtures/tinymist-zoom-edges.json')));
  assert.equal(output.fixture_sha256, reference.fixture_sha256);
  for (let i = 0; i < cases.length; i++) {
    for (const phase of ['before', 'after']) for (const field of ['paper', 'viewport', 'scroll', 'view']) {
      cases[i][phase].geometry[field].forEach((n, j) => assert.ok(Math.abs(n - reference.cases[i][phase].geometry[field][j]) <= 1,
        `${cases[i].mode} ${cases[i].edge} ${cases[i].scale}: ${phase}.${field}[${j}] changed`));
    }
  }
  const destination = process.env.TIPTOPTYP_ZOOM_EDGE_RECEIPT;
  if (destination) fs.writeFileSync(destination, JSON.stringify(output, null, 2) + '\n');
  console.log(`Real full/partial Tinymist edge geometry: ${cases.length} cases passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
