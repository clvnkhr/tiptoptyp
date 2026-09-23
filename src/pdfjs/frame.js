/* One isolated PDF.js surface. The parent stages replacements before swapping. */
(() => {
  "use strict";
  let app, location = null, version = 0;
  let readyResolve, readyReject;
  const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
  function bookmark(position, count) {
    return position ? `page=${Math.min(position.pageNumber, count)}&zoom=${position.scale},${position.left},${position.top}` : "zoom=page-width";
  }
  window.tiptoptypSurface = {
    ready,
    get version() { return version; },
    focusState() {
      const element = document.activeElement;
      return { id: element?.id, start: element?.selectionStart, end: element?.selectionEnd };
    },
    activate(focus) {
      document.body.inert = false;
      if (!focus?.id) return;
      const element = document.getElementById(focus.id);
      element?.focus({ preventScroll: true });
      if (typeof focus.start === "number") element?.setSelectionRange(focus.start, focus.end);
    },
    snapshot() {
      const container = document.getElementById("viewerContainer");
      return { location, top: container.scrollTop, left: container.scrollLeft, pages: app.pdfDocument.numPages, rotation: app.pdfViewer.pagesRotation,
        scroll: app.pdfViewer.scrollMode, spread: app.pdfViewer.spreadMode,
        sidebar: app.viewsManager.visibleView,
        find: app.findBar.findField.value, findOpen: app.findBar.opened,
        findOptions: Object.fromEntries(["caseSensitive", "entireWord", "highlightAll", "matchDiacritics"].map(key => [key, app.findBar[key].checked])) };
    },
    async load(data, state) {
      await ready;
      document.documentElement.classList.toggle("tiptoptyp-dark", state.dark);
      let initialized;
      const initialization = new Promise(resolve => { initialized = resolve; });
      app.eventBus.on("documentinit", initialized, { once: true });
      try {
        await app.open({ data, filename: state.filename });
        await initialization;
        await app.pdfViewer.pagesPromise;
      } finally { app.eventBus.off("documentinit", initialized); }
    },
    async restore(state) {
      if (state) {
        app.pdfViewer.pagesRotation = state.rotation;
        app.pdfViewer.scrollMode = state.scroll;
        app.pdfViewer.spreadMode = state.spread;
        const sidebar = app.viewsManager;
        if (state.sidebar) sidebar?.switchView(state.sidebar, true);
        else sidebar?.close();
        for (const [key, value] of Object.entries(state.findOptions)) app.findBar[key].checked = value;
        if (state.findOpen) app.findBar.open(); else app.findBar.close();
        if (state.find && app.findBar.findField.value !== state.find) {
          app.findBar.findField.value = state.find;
          await new Promise(resolve => {
            const found = event => {
              if (event.state === 3) return;
              app.eventBus.off("updatefindcontrolstate", found);
              app.findController._scrollMatches = false;
              resolve();
            };
            app.eventBus.on("updatefindcontrolstate", found);
            app.eventBus.dispatch("find", { source: app.findBar, type: "again", query: state.find,
              ...state.findOptions, findPrevious: false });
          });
        }
      }
      app.pdfLinkService.setHash(bookmark(state?.location, app.pdfDocument.numPages));
      if (state && state.pages === app.pdfDocument.numPages) {
        const container = document.getElementById("viewerContainer");
        container.scrollTop = state.top;
        container.scrollLeft = state.left;
      }
      app.pdfViewer.update();
      // Wait for the visible pages, not merely the parsed document. The frame
      // keeps full layout bounds while transparent so the rendering queue runs.
      await new Promise((resolve, reject) => {
        const events = ["pagerendered", "textlayerrendered", "annotationlayerrendered"];
        let settled = false;
        const cleanup = () => { clearTimeout(timeout); events.forEach(e => app.eventBus.off(e, check)); };
        const check = () => {
          if (settled) return;
          if (state && state.pages === app.pdfDocument.numPages) {
            const container = document.getElementById("viewerContainer");
            const top = Math.min(state.top, container.scrollHeight - container.clientHeight);
            if (Math.abs(container.scrollTop - top) > 1) {
              container.scrollTop = top;
              container.scrollLeft = state.left;
              app.pdfViewer.update();
              requestAnimationFrame(check);
              return;
            }
          }
          const pages = app.pdfViewer._getVisiblePages().views;
          if (!pages.length || pages.some(({ view }) => view.renderingState !== 3 ||
            (view.textLayer && !view.textLayer.div.querySelector(".endOfContent")))) return;
          settled = true; cleanup(); resolve();
        };
        const timeout = setTimeout(() => { settled = true; cleanup(); reject(new Error("PDF visible-page rendering timed out")); }, 30000);
        events.forEach(e => app.eventBus.on(e, check));
        requestAnimationFrame(check);
      });
    },
    theme(dark) { document.documentElement.classList.toggle("tiptoptyp-dark", dark); },
  };
  document.addEventListener("webviewerloaded", () => {
    document.body.inert = true;
    const options = window.PDFViewerApplicationOptions;
    for (const [key, value] of Object.entries({
      defaultUrl: "",
      defaultZoomValue: "page-width",
      disablePreferences: true,
      disableHistory: true,
      viewOnLoad: 1,
      sidebarViewOnLoad: 0,
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
      app.eventBus.on("updateviewarea", event => { location = event.location; version++; });
      for (const event of ["rotationchanging", "sidebarviewchanged", "find", "scrollmodechanged", "spreadmodechanged"]) app.eventBus.on(event, () => version++);
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
      readyResolve();
      parent.document.dispatchEvent(new CustomEvent("tiptoptyp-surface-ready", { detail: window }));
    }).catch(readyReject);
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
