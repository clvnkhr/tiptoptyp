/* tiptoptyp's adapter for the pinned Mozilla generic viewer. */
(() => {
  "use strict";
  const notify = (type, message = "") => window.ipc?.postMessage(JSON.stringify({ type, message }));
  let app;
  let desired = null;
  let displayed = null;
  let location = null;
  let busy = false;
  let refreshSerial = 0;

  function bookmark(position, pageCount = Infinity) {
    if (!position) return "zoom=page-width";
    return `page=${Math.min(position.pageNumber, pageCount)}&zoom=${position.scale},${position.left},${position.top}`;
  }

  async function drain() {
    if (busy || !app) return;
    busy = true;
    let loading = null;
    try {
      while (desired && desired.revision !== displayed?.revision) {
        const next = desired;
        loading = next;
        const response = await fetch(next.url);
        if (next !== desired) continue;
        if (!response.ok) throw new Error(`PDF load failed (${response.status})`);
        const data = new Uint8Array(await response.arrayBuffer());
        if (next !== desired) continue;
        const restore = next.document === displayed?.document ? location : null;
        // Page count is unknown until parsing finishes. Start with a valid
        // bookmark, then clamp the saved destination before initial layout.
        app.initialBookmark = bookmark(restore, 1);
        let onInitialized;
        const initialized = new Promise(resolve => { onInitialized = resolve; });
        app.eventBus.on("documentinit", onInitialized, { once: true });
        try {
          await app.open({ data, filename: next.filename });
          app.initialBookmark = bookmark(restore, app.pdfDocument.numPages);
          await initialized;
        } finally {
          app.eventBus.off("documentinit", onInitialized);
        }
        app.pdfLinkService.setHash(bookmark(restore, app.pdfDocument.numPages));
        displayed = next;
        notify("loaded");
      }
    } catch (error) {
      if (loading === desired) notify("error", String(error));
    } finally {
      busy = false;
      if (desired && desired.revision !== displayed?.revision && loading !== desired) drain();
    }
  }

  async function refresh() {
    const serial = ++refreshSerial;
    try {
      const response = await fetch("../state.json");
      if (!response.ok) throw new Error(`PDF state failed (${response.status})`);
      const state = await response.json();
      if (serial !== refreshSerial || !state) return;
      document.documentElement.classList.toggle("tiptoptyp-dark", state.dark);
      // Theme changes do not reload the document or cancel a pending load.
      if (desired?.revision !== state.revision) desired = state;
      await drain();
    } catch (error) {
      notify("error", String(error));
    }
  }

  window.tiptoptypPdf = { refresh };
  document.addEventListener("webviewerloaded", () => {
    const options = window.PDFViewerApplicationOptions;
    for (const [key, value] of Object.entries({
      defaultUrl: "",
      defaultZoomValue: "page-width",
      disablePreferences: true,
      disableHistory: true,
      viewOnLoad: 1,
      scrollModeOnLoad: 0,
      spreadModeOnLoad: 0,
      enableScripting: false,
      isEvalSupported: false,
      annotationEditorMode: -1,
      enableAltTextModelDownload: false,
      enableComment: false,
      supportsPrinting: false,
      supportsDownloading: false,
      externalLinkTarget: 2,
      // Limit an individual canvas to 16 Mi pixels (64 MiB RGBA). PDF.js's
      // rendering queue and page-view buffer retain only the visible vicinity.
      maxCanvasPixels: 16777216,
    })) options.set(key, value);
    const viewer = window.PDFViewerApplication;
    viewer.initializedPromise.then(() => {
      app = viewer;
      app.eventBus.on("updateviewarea", event => { location = event.location; });
      // macOS WebKit delivers trackpad pinch as GestureEvents, while Chromium
      // uses Ctrl-wheel. Feed both into PDF.js's one anchored scale.
      let gestureScale = null;
      const container = document.getElementById("viewerContainer");
      container.addEventListener("gesturestart", event => {
        event.preventDefault();
        gestureScale = 1;
      }, { passive: false });
      container.addEventListener("gesturechange", event => {
        event.preventDefault();
        if (gestureScale === null || !Number.isFinite(event.scale) || event.scale <= 0) return;
        app.updateZoom(null, event.scale / gestureScale, [event.clientX, event.clientY]);
        gestureScale = event.scale;
      }, { passive: false });
      container.addEventListener("gestureend", event => {
        event.preventDefault();
        gestureScale = null;
      }, { passive: false });
      window.addEventListener("wheel", event => {
        if (gestureScale !== null && (event.ctrlKey || event.metaKey)) {
          event.preventDefault();
          event.stopImmediatePropagation();
        }
      }, { passive: false, capture: true });
      window.addEventListener("tiptoptyp-preview-zoom", event => {
        if (event.detail === "in") app.zoomIn();
        if (event.detail === "out") app.zoomOut();
        if (event.detail === "reset") app.pdfViewer.currentScaleValue = "page-width";
      });
      notify("ready");
      refresh();
    }).catch(error => notify("error", String(error)));
  }, { once: true });

  // This surface follows the editor's artifact. Opening another file in the
  // generic viewer would detach it silently from that artifact.
  document.addEventListener("keydown", event => {
    if ((event.metaKey || event.ctrlKey) && ["o", "s", "p"].includes(event.key.toLowerCase())) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);
  document.addEventListener("drop", event => {
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
})();
