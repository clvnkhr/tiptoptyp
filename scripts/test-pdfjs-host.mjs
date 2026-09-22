import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../src/pdfjs/host.js", import.meta.url), "utf8");
const settle = () => new Promise(resolve => setImmediate(resolve));
const state = (revision, document = 1) => ({ revision, document, url: `/pdf/${revision}`, filename: "fixture.pdf", dark: false });
const response = value => ({ ok: true, json: async () => value, arrayBuffer: async () => new ArrayBuffer(4) });

async function fixture() {
  const window = new EventTarget();
  const document = new EventTarget();
  const container = new EventTarget();
  const options = {};
  const messages = [];
  const opens = [];
  const zooms = [];
  const listeners = new Map();
  let latest = state(1);
  let fetchPdf = async () => response(null);
  let dark = false;
  const eventBus = {
    on(name, listener) { if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name).add(listener); },
    off(name, listener) { listeners.get(name)?.delete(listener); },
    dispatch(name, value) { for (const listener of [...(listeners.get(name) || [])]) listener(value); },
  };
  const app = {
    initializedPromise: Promise.resolve(), eventBus,
    pdfViewer: {}, pdfDocument: { numPages: 24 },
    pdfLinkService: { setHash(hash) { app.hash = hash; } },
    async open() { opens.push(app.initialBookmark); eventBus.dispatch("documentinit", {}); },
    zoomIn() { zooms.push("in"); }, zoomOut() { zooms.push("out"); },
    updateZoom(...args) { zooms.push(args); },
  };
  Object.assign(window, { PDFViewerApplication: app, PDFViewerApplicationOptions: { set(key, value) { options[key] = value; } }, ipc: { postMessage(message) { messages.push(JSON.parse(message)); } } });
  Object.assign(document, { getElementById: () => container, documentElement: { classList: { toggle(_name, enabled) { dark = enabled; } } } });
  vm.runInNewContext(source, { window, document, Uint8Array, Number, fetch: async url => url === "../state.json" ? response(latest) : fetchPdf(url) });
  document.dispatchEvent(new Event("webviewerloaded"));
  await settle();
  return { app, window, container, options, messages, opens, zooms, eventBus,
    dark: () => dark,
    async publish(value) { latest = value; await window.tiptoptypPdf.refresh(); },
    fetchPdf(fn) { fetchPdf = fn; },
  };
}

test("offline initialization disables PDF scripting and preserves the location only for the same document", async () => {
  const f = await fixture();
  assert.equal(f.options.enableScripting, false);
  assert.equal(f.options.isEvalSupported, false);
  assert.equal(f.options.defaultUrl, "");
  assert.equal(f.options.annotationEditorMode, -1);
  assert.deepEqual(f.opens, ["zoom=page-width"]);
  f.eventBus.dispatch("updateviewarea", { location: { pageNumber: 20, scale: 175, left: 30, top: 80 } });
  await f.publish(state(2));
  assert.equal(f.app.hash, "page=20&zoom=175,30,80");
  f.app.pdfDocument.numPages = 3;
  await f.publish(state(3));
  assert.equal(f.app.hash, "page=3&zoom=175,30,80");
  await f.publish(state(4, 2));
  assert.equal(f.app.hash, "zoom=page-width");
});

test("unchanged refreshes and theme changes do not reopen the PDF", async () => {
  const f = await fixture();
  for (let i = 0; i < 100; i++) await f.publish(state(1));
  await f.publish({ ...state(1), dark: true });
  assert.equal(f.opens.length, 1);
  assert.equal(f.dark(), true);
});

test("rapid rebuilds skip a superseded download and keep a single open at a time", async () => {
  const f = await fixture();
  let finish;
  f.fetchPdf(url => url === "/pdf/2" ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(response(null)));
  const loading = f.publish(state(2));
  await settle();
  await f.publish(state(3));
  finish({ ok: false, status: 409 });
  await loading;
  assert.equal(f.opens.length, 2);
  assert.ok(!f.messages.some(message => message.type === "error"));
});

test("a failed load removes its initialization listener and a newer revision can recover", async () => {
  const f = await fixture();
  const open = f.app.open;
  f.app.open = async () => { throw new Error("bad PDF"); };
  await f.publish(state(2));
  assert.equal(f.messages.at(-1).type, "error");
  f.app.open = open;
  await f.publish(state(3));
  assert.equal(f.messages.at(-1).type, "loaded");
});

test("WebKit pinch and host zoom actions share PDF.js's scale", async () => {
  const f = await fixture();
  function gesture(type, scale) {
    const event = new Event(type, { cancelable: true });
    Object.assign(event, { scale, clientX: 120, clientY: 220 });
    f.container.dispatchEvent(event);
    assert.ok(event.defaultPrevented);
  }
  gesture("gesturestart", 1);
  gesture("gesturechange", 1.5);
  gesture("gesturechange", 1.8);
  gesture("gestureend", 1.8);
  assert.equal(f.zooms[0][1], 1.5);
  assert.equal(f.zooms[1][1], 1.2);
  assert.equal(f.zooms[0][2].join(","), "120,220");
  f.window.dispatchEvent(new CustomEvent("tiptoptyp-preview-zoom", { detail: "reset" }));
  assert.equal(f.app.pdfViewer.currentScaleValue, "page-width");
});
