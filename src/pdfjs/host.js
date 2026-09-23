/* Stable host: retain the interactive successful viewer until its replacement
   has rendered. At most one active and one staging frame exist. */
(() => {
  "use strict";
  document.addEventListener("webviewerloaded", event => {
    const source = event.detail?.source;
    if (source && source !== window) source.document.dispatchEvent(new source.CustomEvent("webviewerloaded"));
  });
  let active = null, pending = null, desired = null, serial = 0;
  const notify = (type, message = "") => window.ipc?.postMessage(JSON.stringify({ type, message }));
  const status = () => document.getElementById("status");
  function discard(surface) { surface?.controller.abort(); surface?.frame.remove(); }
  async function replace(state) {
    discard(pending);
    const frame = document.createElement("iframe");
    frame.title = "PDF preview";
    frame.className = "staging";
    const controller = new AbortController();
    const surface = { frame, controller, state };
    pending = surface;
    try {
      const loaded = new Promise((resolve, reject) => {
        const ready = event => {
          if (event.detail === frame.contentWindow) { document.removeEventListener("tiptoptyp-surface-ready", ready); resolve(); }
        };
        document.addEventListener("tiptoptyp-surface-ready", ready);
        controller.signal.addEventListener("abort", () => document.removeEventListener("tiptoptyp-surface-ready", ready), { once: true });
        frame.onerror = () => reject(new Error("PDF viewer could not start"));
        controller.signal.addEventListener("abort", () => reject(new DOMException("Superseded", "AbortError")), { once: true });
      });
      loaded.catch(() => {});
      frame.src = "frame.html";
      document.body.append(frame);
      const response = await fetch(state.url, { signal: controller.signal });
      if (!response.ok) throw new Error(`PDF load failed (${response.status})`);
      const data = new Uint8Array(await response.arrayBuffer());
      await loaded;
      if (pending !== surface) return;
      surface.api = frame.contentWindow.tiptoptypSurface;
      await surface.api.load(data, state);
      if (pending !== surface) return;
      // Navigation may continue while parsing/rendering. Restore the latest
      // position again if it changed; never jump back to an earlier bookmark.
      let version;
      do {
        version = active?.api.version;
        await surface.api.restore(active?.state.document === state.document ? active.api.snapshot() : null);
        if (pending !== surface) return;
      } while (version !== active?.api.version);
      const previous = active;
      const hadFocus = previous && document.activeElement === previous.frame;
      surface.api.theme(desired.dark);
      surface.api.activate(hadFocus ? previous.api.focusState() : null);
      frame.className = "active";
      active = surface;
      pending = null;
      if (hadFocus) frame.contentWindow.focus();
      discard(previous);
      status().hidden = true;
      notify("loaded");
    } catch (error) {
      if (pending !== surface) return;
      pending = null;
      discard(surface);
      status().textContent = `Preview update failed: ${error}. Click to retry.`;
      status().hidden = false;
      notify(active ? "update-error" : "error", String(error));
    }
  }
  async function refresh() {
    const request = ++serial;
    try {
      const response = await fetch("../state.json");
      if (!response.ok) throw new Error(`PDF state failed (${response.status})`);
      const state = await response.json();
      if (request !== serial || !state) return;
      desired = state;
      active?.api.theme(state.dark);
      pending?.api?.theme(state.dark);
      if (state.revision !== active?.state.revision && state.revision !== pending?.state.revision) await replace(state);
    } catch (error) { notify(active ? "update-error" : "error", String(error)); }
  }
  window.tiptoptypPdf = { refresh, get activeFrame() { return active?.frame; } };
  window.addEventListener("tiptoptyp-preview-zoom", event => {
    const target = active?.frame.contentWindow;
    if (target) target.dispatchEvent(new target.CustomEvent(event.type, { detail: event.detail }));
  });
  window.addEventListener("DOMContentLoaded", () => {
    status().onclick = () => { if (desired) replace(desired); };
    notify("ready");
    refresh();
  }, { once: true });
})();
