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
const build = run("cargo", ["test", "--bin", "tiptoptyp", "--no-run", "--message-format=json"]);
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
  const remote = [];
  const requests = new Set();
  page.on("request", request => requests.add(request.url()));
  page.on("requestfinished", request => requests.delete(request.url()));
  page.on("pageerror", error => { errors.push(String(error)); console.error(error); });
  page.on("console", message => { if (message.type() === "error") console.error(message.text()); });
  page.on("requestfailed", request => errors.push(`${request.url()}: ${request.failure()?.errorText}`));
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
  await page.waitForFunction(() => window.messages.some(event => event.type === "loaded"), { timeout: 20000 });
  const state = () => page.evaluate(() => ({
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
  await page.locator("#viewFindButton").click();
  await page.locator("#findInput").fill("Scroll freely");
  await page.waitForFunction(() => PDFViewerApplication.findController.pageMatches
    .reduce((sum, matches) => sum + matches.length, 0) === 23);
  await page.locator("#findNextButton").click();
  await page.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 3);
  await page.locator("#viewFindButton").click();
  await page.locator("#viewsManagerToggleButton").click();
  await page.waitForSelector('#thumbnailsView .thumbnail[page-number="1"] img');
  assert.equal(await page.locator("#thumbnailsView .thumbnail").count(), 24);
  await page.locator('#thumbnailsView .thumbnail[page-number="1"] .thumbnailImageContainer').click();
  await page.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 1);
  await page.waitForFunction(() => {
    const image = document.querySelector('#thumbnailsView .thumbnail[page-number="1"] img');
    return image.complete && image.naturalWidth > 0;
  });
  await page.locator("#viewsManagerToggleButton").click();
  await page.waitForSelector('.annotationLayer a[href*="example.com"]');
  const external = page.locator('.annotationLayer a[href*="example.com"]');
  assert.equal(await external.getAttribute("target"), "_blank");
  assert.match(await external.getAttribute("rel"), /noreferrer/);
  await page.locator('.annotationLayer a[href*="#"]').first().click();
  await page.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 24);
  await page.waitForFunction(() => document.querySelector('[data-page-number="24"] canvas'));
  await page.locator('[data-page-number="24"] .annotationLayer a[href*="#"]').click();
  await page.waitForFunction(() => PDFViewerApplication.pdfViewer.currentPageNumber === 1);
  await page.mouse.move(480, 380);
  await page.mouse.wheel(0, 1500);
  await page.waitForFunction(() => document.getElementById("viewerContainer").scrollTop > 1000);
  const beforeZoom = await state();
  await page.locator("#zoomInButton").click();
  assert.ok((await state()).scale > beforeZoom.scale);
  await page.evaluate(() => {
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
  assert.equal((await state()).page, beforeReload.page);
  assert.ok(Math.abs((await state()).scale - beforeReload.scale) < 0.001);
  assert.ok(Math.abs((await state()).top - beforeReload.top) < 4);
  await publish("dark");
  assert.equal(await page.locator("html").getAttribute("class"), "tiptoptyp-dark");
  assert.equal((await state()).page, beforeReload.page);
  await publish("short");
  assert.equal((await state()).pages, 1);
  assert.equal((await state()).page, 1);
  await publish("switch");
  assert.equal((await state()).page, 1);
  assert.equal((await state()).scaleValue, "page-width");
  // A large jump renders the destination and retains a bounded canvas buffer.
  await page.evaluate(() => PDFViewerApplication.pdfLinkService.setHash("page=24"));
  await page.waitForFunction(() => document.querySelector('[data-page-number="24"] canvas'));
  assert.ok((await state()).canvases < 16);
  await page.setViewportSize({ width: 450, height: 700 });
  await page.waitForTimeout(500);
  await page.evaluate(() => PDFViewerApplication.pdfLinkService.setHash("page=1&zoom=page-width"));
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(evidence, "chromium-narrow.png") });
  await page.setViewportSize({ width: 900, height: 700 });
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(evidence, "chromium-viewer.png") });
  assert.deepEqual(errors, []);
  assert.deepEqual(remote, []);
  const result = { browser: await browser.version(), platform: process.platform, arch: process.arch, beforeReload, afterReload, final: await state(), errors, remote, url };
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
