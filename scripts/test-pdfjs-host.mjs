import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
const source = readFileSync(new URL("../src/pdfjs/host.js", import.meta.url), "utf8");
const settle = () => new Promise(resolve => setImmediate(resolve));
const state = (revision, document = 1) => ({ revision, document, url: `/pdf/${revision}`, filename: "fixture.pdf", dark: false });
const response = value => ({ ok: true, json: async () => value, arrayBuffer: async () => new ArrayBuffer(4) });
async function fixture() {
  const window = new EventTarget(), document = new EventTarget();
  const frames = new Set(), messages = [], status = {};
  let latest = state(1), load = async () => {}, restore = async () => {};
  const make = () => {
    const api = { activate() {}, focusState() { return null; }, version: 0, position: { page: 12 }, snapshot() { return this.position; },
      async load(...args) { await load(...args); },
      async restore(value) { this.restored = value; await restore(value); },
      theme(value) { this.dark = value; } };
    return { contentWindow: { tiptoptypSurface: api }, remove() { frames.delete(this); } };
  };
  Object.assign(document, { getElementById: () => status, createElement: make,
    body: { append(frame) { frames.add(frame); queueMicrotask(() => { const e = new Event("tiptoptyp-surface-ready"); e.detail = frame.contentWindow; document.dispatchEvent(e); }); } } });
  window.ipc = { postMessage(message) { messages.push(JSON.parse(message)); } };
  vm.runInNewContext(source, { window, document, Uint8Array, AbortController, DOMException,
    fetch: async url => response(url === "../state.json" ? latest : null) });
  window.dispatchEvent(new Event("DOMContentLoaded")); await settle();
  return { window, frames, messages, status,
    active: () => window.tiptoptypPdf.activeFrame,
    load(fn) { load = fn; }, restore(fn) { restore = fn; },
    async publish(value) { latest = value; await window.tiptoptypPdf.refresh(); } };
}
test("unchanged revisions and theme changes reuse the active viewer", async () => {
  const f = await fixture(), active = f.active();
  for (let i = 0; i < 100; i++) await f.publish(state(1));
  await f.publish({ ...state(1), dark: true });
  assert.equal(f.active(), active); assert.equal(f.frames.size, 1);
  assert.equal(active.contentWindow.tiptoptypSurface.dark, true);
});
test("old viewer remains interactive until replacement rendering completes", async () => {
  const f = await fixture(), active = f.active(); let finish;
  f.load(() => new Promise(resolve => { finish = resolve; }));
  const pending = f.publish(state(2)); await settle();
  assert.equal(f.active(), active); assert.equal(active.className, "active");
  assert.equal(f.frames.size, 2);
  active.contentWindow.tiptoptypSurface.position = { page: 20 };
  finish(); await pending;
  assert.notEqual(f.active(), active); assert.equal(f.frames.size, 1);
  assert.deepEqual(f.active().contentWindow.tiptoptypSurface.restored, { page: 20 });
});
test("navigation during rendering is restored again before committing", async () => {
  const f = await fixture(), api = f.active().contentWindow.tiptoptypSurface;
  let count = 0;
  f.restore(async () => { if (++count === 1) { api.version++; api.position = { page: 22 }; } });
  await f.publish(state(2));
  assert.equal(count, 2); assert.deepEqual(f.active().contentWindow.tiptoptypSurface.restored, { page: 22 });
});
test("failed updates preserve successful content and can retry", async () => {
  const f = await fixture(), active = f.active();
  f.load(async () => { throw new Error("bad PDF"); });
  await f.publish(state(2));
  assert.equal(f.active(), active); assert.equal(f.frames.size, 1);
  assert.equal(f.messages.at(-1).type, "update-error"); assert.equal(f.status.hidden, false);
  f.load(async () => {}); await f.publish(state(2));
  assert.notEqual(f.active(), active); assert.equal(f.status.hidden, true);
});
test("superseded renders cannot replace newer content and frames stay bounded", async () => {
  const f = await fixture(); let finish;
  f.load(() => new Promise(resolve => { finish = resolve; }));
  const slow = f.publish(state(2)); await settle();
  f.load(async () => {}); await f.publish(state(3));
  const newest = f.active(); finish(); await slow;
  assert.equal(f.active(), newest); assert.equal(f.frames.size, 1);
});
test("switching documents does not restore the previous document position", async () => {
  const f = await fixture(); await f.publish(state(2, 2));
  assert.equal(f.active().contentWindow.tiptoptypSurface.restored, null);
});
