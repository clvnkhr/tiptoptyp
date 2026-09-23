// Opt-in real-browser integration. Set PLAYWRIGHT_MODULE to an installed
// playwright package path; no browser dependency is shipped with the app.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createInterface } from "node:readline";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const root = resolve(import.meta.dirname, "..");
const temp = mkdtempSync(join(tmpdir(), "tiptoptyp-pdfjs-"));
const evidence = resolve(root, ".tiptoptyp/screenshots/agent-review/pdfjs");
mkdirSync(evidence, { recursive: true });
const tool = process.env.TIPTOPTYP_TEST_TYPST || join(root, "toolchain/bin",
  readdirSync(join(root, "toolchain/bin")).find(name => name.startsWith("typst-") && !name.endsWith(".exe")));
function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}
for (const pages of [1, 24]) {
  run(tool, ["compile", "--input", `pages=${pages}`, "scripts/fixtures/pdfjs.typ", join(temp, `${pages}.pdf`)]);
}
const build = run("cargo", ["test", ...(process.env.PDFJS_RELEASE === "1" ? ["--release"] : []), "--bin", "tiptoptyp", "--no-run", "--message-format=json"]);
const binary = build.split("\n").filter(Boolean).map(line => JSON.parse(line)).find(item => item.executable)?.executable;
assert.ok(binary);
const server = spawn(binary, ["--ignored", "--exact", "pdfjs::tests::browser_fixture", "--nocapture"], {
  cwd: root, env: { ...process.env, TIPTOPTYP_PDFJS_FIXTURE: join(temp, "24.pdf"), TIPTOPTYP_PDFJS_SHORT_FIXTURE: join(temp, "1.pdf") },
  stdio: ["pipe", "pipe", "inherit"],
});
let receive;
const inbox = [];
const lines = createInterface({ input: server.stdout });
lines.on("line", line => {
  if (!line.startsWith("{")) return;
  const value = JSON.parse(line);
  if (receive) { const callback = receive; receive = null; callback(value); }
  else inbox.push(value);
});
function message() {
  if (inbox.length) return Promise.resolve(inbox.shift());
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Fixture server timed out")), 15000);
    receive = value => { clearTimeout(timer); resolve(value); };
  });
}
const browser = await chromium.launch({
  executablePath: process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
});
try {
  const { url } = await message();
  const page = await browser.newPage({ viewport: { width: 900, height: 700 }, deviceScaleFactor: 2 });
  const errors = [];
  const retiredRequests = [];
  const remote = [];
  const requests = new Set();
  page.on("request", request => requests.add(request.url()));
  page.on("requestfinished", request => requests.delete(request.url()));
  page.on("pageerror", error => { errors.push(String(error)); console.error(error); });
  page.on("console", message => { if (message.type() === "error") console.error(message.text()); });
  page.on("requestfailed", request => {
    if (request.failure()?.errorText === "net::ERR_ABORTED") retiredRequests.push(request);
    else errors.push(`${request.url()}: ${request.failure()?.errorText}`);
  });
  await page.route("**/*", route => {
    if (new URL(route.request().url()).origin === new URL(url).origin) return route.continue();
    remote.push(route.request().url());
    return route.abort();
  });
  await page.addInitScript(() => {
    window.messages = [];
    window.ipc = { postMessage: message => window.messages.push(JSON.parse(message)) };
  });
  await page.goto(url, { waitUntil: "domcontentloaded" }).catch(async error => {
    console.error("Pending requests", [...requests]);
    console.error(await page.evaluate(() => ({ ready: document.readyState, body: document.body?.innerText.slice(0,300), messages: window.messages })));
    throw error;
  });
  await page.waitForFunction(() => window.messages.some(event => event.type === "loaded"), null, { timeout: 45000 }).catch(async error => {
    console.error(await page.evaluate(() => ({ messages: window.messages, status: document.getElementById("status")?.textContent,
      frames: [...document.querySelectorAll("iframe")].map(f => ({ text: f.contentDocument?.body.innerText.slice(0,300),
        initialized: !!f.contentWindow.PDFViewerApplication?.initialized,
        pages: f.contentWindow.PDFViewerApplication?.pdfDocument?.numPages,
        visible: f.contentWindow.PDFViewerApplication?.pdfViewer?._getVisiblePages().views.map(v => ({id:v.id, state:v.view.renderingState, text:!!v.view.textLayer, end:!!v.view.textLayer?.div.querySelector(".endOfContent")})) })) })));
    throw error;
  });
  const viewer = {
    locator: selector => page.frameLocator("iframe.active").locator(selector),
    async evaluate(fn) { const frame = await page.locator("iframe.active").elementHandle(); return (await frame.contentFrame()).evaluate(fn); },
    async waitForFunction(fn) { const frame = await page.locator("iframe.active").elementHandle(); return (await frame.contentFrame()).waitForFunction(fn); },
    async waitForSelector(selector) { return this.locator(selector).waitFor(); },
  };
  const state = () => viewer.evaluate(() => ({
    page: PDFViewerApplication.pdfViewer.currentPageNumber,
    pages: PDFViewerApplication.pdfDocument.numPages,
    scale: PDFViewerApplication.pdfViewer.currentScale,
    scaleValue: PDFViewerApplication.pdfViewer.currentScaleValue,
    top: document.getElementById("viewerContainer").scrollTop,
    canvases: document.querySelectorAll("#viewer canvas").length,
  }));
  assert.equal((await state()).pages, 24);
  assert.equal((await state()).scaleValue, "page-width");
  // These viewer features predate the backlog triage: exercise the real
  // bundled controls before marking document search/thumbnails complete.
  await viewer.locator("#viewFindButton").click();
  await viewer.locator("#findInput").fill("Scroll freely");
  await viewer.waitForFunction(() => PDFViewerApplication.findController.pageMatches
    .reduce((sum, matches) => sum + matches.length, 0) === 23);
  await viewer.locator("#findNextButton").click();
  await viewer.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 3);
  await viewer.locator("#viewFindButton").click();
  await viewer.locator("#viewsManagerToggleButton").click();
  await viewer.waitForSelector('#thumbnailsView .thumbnail[page-number="1"] img');
  assert.equal(await viewer.locator("#thumbnailsView .thumbnail").count(), 24);
  await viewer.locator('#thumbnailsView .thumbnail[page-number="1"] .thumbnailImageContainer').click();
  await viewer.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 1);
  await viewer.waitForFunction(() => {
    const image = document.querySelector('#thumbnailsView .thumbnail[page-number="1"] img');
    return image.complete && image.naturalWidth > 0;
  });
  await viewer.locator("#viewsManagerToggleButton").click();
  await viewer.waitForSelector('.annotationLayer a[href*="example.com"]');
  const external = viewer.locator('.annotationLayer a[href*="example.com"]');
  assert.equal(await external.getAttribute("target"), "_blank");
  assert.match(await external.getAttribute("rel"), /noreferrer/);
  await viewer.locator('.annotationLayer a[href*="#"]').first().click();
  await viewer.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 24);
  await viewer.waitForFunction(() => document.querySelector('[data-page-number="24"] canvas'));
  await viewer.locator('[data-page-number="24"] .annotationLayer a[href*="#"]').click();
  await viewer.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 1);
  await page.mouse.move(480, 380);
  await page.mouse.wheel(0, 1500);
  await viewer.waitForFunction(() => document.getElementById("viewerContainer").scrollTop > 1000);
  const beforeZoom = await state();
  await viewer.locator("#zoomInButton").click();
  assert.ok((await state()).scale > beforeZoom.scale);
  await viewer.evaluate(() => {
    PDFViewerApplication.pdfViewer.currentScaleValue = 1.75;
    PDFViewerApplication.pdfLinkService.setHash("page=12&zoom=175,0,100");
  });
  await page.waitForTimeout(500);
  const beforeReload = await state();
  async function publish(command) {
    const received = message();
    server.stdin.write(`${command}\n`);
    await received;
    await page.evaluate(() => window.tiptoptypPdf.refresh());
    await page.waitForTimeout(500);
  }
  await publish("reload");
  const afterReload = await state();
  console.log({ beforeReload, afterReload });
  assert.equal((await state()).page, beforeReload.page);
  assert.ok(Math.abs((await state()).scale - beforeReload.scale) < 0.001);
  assert.ok(Math.abs((await state()).top - beforeReload.top) < 4);
  await publish("dark");
  assert.equal(await viewer.locator("html").getAttribute("class"), "tiptoptyp-dark");
  assert.equal((await state()).page, beforeReload.page);
  await publish("short");
  assert.equal((await state()).pages, 1);
  assert.equal((await state()).page, 1);
  await publish("switch");
  assert.equal((await state()).page, 1);
  assert.equal((await state()).scaleValue, "page-width");
  // A large jump renders the destination and retains a bounded canvas buffer.
  await viewer.evaluate(() => PDFViewerApplication.pdfLinkService.setHash("page=24"));
  await viewer.waitForFunction(() => document.querySelector('[data-page-number="24"] canvas'));
  assert.ok((await state()).canvases < 16);
  await page.setViewportSize({ width: 450, height: 700 });
  await page.waitForTimeout(500);
  await viewer.evaluate(() => PDFViewerApplication.pdfLinkService.setHash("page=1&zoom=page-width"));
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(evidence, "chromium-narrow.png") });
  await page.setViewportSize({ width: 900, height: 700 });
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(evidence, "chromium-viewer.png") });
  // Same document, viewport, zoom and warm browser for both paths. The baseline
  // exercises the pinned viewer's previous close/open behavior directly.
  const baseline = await page.evaluate(async () => {
    const app = window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication;
    const bytes = await app.pdfDocument.getData();
    let running = true, blankFrames = 0, sampledFrames = 0;
    function sample() {
      if (!running) return;
      sampledFrames++;
      if (!app.pdfViewer._getVisiblePages().views.some(({view}) => view.renderingState === 3)) blankFrames++;
      requestAnimationFrame(sample);
    }
    requestAnimationFrame(sample);
    const start = performance.now();
    app.initialBookmark = "page=1&zoom=page-width";
    await app.open({ data: bytes });
    await app.pdfViewer.onePageRendered;
    running = false;
    return { elapsedMs: performance.now() - start, blankFrames, sampledFrames };
  });
  await page.evaluate(() => {
    window.stagingMeasurement = { start: performance.now(), blankFrames: 0, sampledFrames: 0, maxFrames: 0, running: true };
    function sample() {
      const m = window.stagingMeasurement;
      if (!m.running) return;
      m.sampledFrames++;
      m.maxFrames = Math.max(m.maxFrames, document.querySelectorAll("iframe").length);
      const app = window.tiptoptypPdf.activeFrame?.contentWindow.PDFViewerApplication;
      if (!app?.pdfViewer._getVisiblePages().views.some(({view}) => view.renderingState === 3)) m.blankFrames++;
      requestAnimationFrame(sample);
    }
    requestAnimationFrame(sample);
  });
  const published = message(); server.stdin.write("reload\n"); await published;
  await page.evaluate(() => window.tiptoptypPdf.refresh());
  const staged = await page.evaluate(() => {
    const m = window.stagingMeasurement; m.running = false;
    return { elapsedMs: performance.now() - m.start, blankFrames: m.blankFrames, sampledFrames: m.sampledFrames, maxFrames: m.maxFrames };
  });
  assert.equal(staged.blankFrames, 0);
  assert.ok(staged.maxFrames <= 2);
  for (const request of retiredRequests) assert.ok(request.frame().isDetached(), `Unexpected canceled request in a live viewer: ${request.url()}`);
  assert.deepEqual(errors, []);
  assert.deepEqual(remote, []);
  const result = { baseline, staged, buildProfile: process.env.PDFJS_RELEASE === "1" ? "release" : "debug", fixture: "24 pages, 900x700, page-width, warm viewer", browser: await browser.version(), platform: process.platform, arch: process.arch, beforeReload, afterReload, final: await state(), errors, remote, url };
  writeFileSync(join(evidence, "browser-results.json"), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result, null, 2));
  if (process.env.PDFJS_NATIVE === "1") {
    const probe = join(temp, "native-probe");
    run("swiftc", ["-O", "scripts/check-pdfjs-native.swift", "-o", probe]);
    const native = run(probe, [url, join(evidence, "wkwebview.png")]);
    writeFileSync(join(evidence, "native-results.txt"), native);
    console.log(native);
  }
} finally {
  server.stdin.end("quit\n");
  await browser.close();
}
