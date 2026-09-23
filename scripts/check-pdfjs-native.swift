// Opt-in WKWebView probe. Captures the child view's framebuffer, not the desktop.
import Cocoa
import WebKit

@MainActor
final class PdfJsProbe: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    var window: NSWindow!
    var web: WKWebView!
    var attempts = 0
    let url = URL(string: CommandLine.arguments[1])!
    let output = CommandLine.arguments[2]

    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 120, y: 120, width: 1100, height: 740),
                          styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "tiptoptyp PDF.js native QA"
        let parent = NSView(frame: NSRect(x: 0, y: 0, width: 1100, height: 740))
        web = WKWebView(frame: NSRect(x: 200, y: 20, width: 880, height: 700))
        web.navigationDelegate = self
        web.allowsMagnification = false
        parent.addSubview(web)
        window.contentView = parent
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        web.load(URLRequest(url: url))
        ready()
    }

    func ready() {
        web.evaluateJavaScript("Boolean(window.tiptoptypPdf?.activeFrame?.contentWindow.PDFViewerApplication?.isInitialViewSet && window.tiptoptypPdf.activeFrame.contentDocument.querySelector('#viewer canvas'))") { value, error in
            if value as? Bool == true {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.exercise() }
            } else if self.attempts < 200 {
                self.attempts += 1
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { self.ready() }
            } else {
                self.web.evaluateJavaScript("JSON.stringify({ready:document.readyState,body:document.body?.innerText.slice(0,500)})") { value, _ in
                    print("WKWebView readiness failed: \(value ?? "missing"), \(String(describing: error))")
                    exit(1)
                }
            }
        }
    }

    func exercise() {
        DispatchQueue.main.asyncAfter(deadline: .now() + 40) {
            self.web.evaluateJavaScript("JSON.stringify({status:document.getElementById('status')?.textContent, frames:[...document.querySelectorAll('iframe')].map(f=>({class:f.className,ready:f.contentDocument.readyState,pages:f.contentWindow.PDFViewerApplication?.pdfDocument?.numPages,inert:f.contentDocument.body?.inert,visible:f.contentWindow.PDFViewerApplication?.pdfViewer?._getVisiblePages().views.map(v=>({id:v.id,state:v.view.renderingState,text:!!v.view.textLayer?.div.querySelector('.endOfContent')}))}))})") { value, error in
                let message = "Native reload timeout state: \(value ?? "missing") \(String(describing:error))\n"
                FileHandle.standardError.write(Data(message.utf8))
                exit(1)
            }
        }
        web.callAsyncJavaScript("""
        const surface = window.tiptoptypPdf.activeFrame.contentWindow;
        const document = surface.document;
        let app = surface.PDFViewerApplication;
        app.pdfLinkService.setHash('page=12&zoom=150,0,400');
        await new Promise(resolve => setTimeout(resolve, 500));
        const before = app.pdfViewer.currentScale;
        const container = document.getElementById('viewerContainer');
        for (const [type, scale] of [['gesturestart',1], ['gesturechange',1.2], ['gestureend',1.2]]) {
          const event = new Event(type, {bubbles:true,cancelable:true});
          Object.assign(event, {scale,clientX:300,clientY:250});
          container.dispatchEvent(event);
        }
        await new Promise(resolve => setTimeout(resolve, 700));
        const after = app.pdfViewer.currentScale;
        if (Math.abs(after / before - 1.2) > .01) throw new Error('Gesture scale did not reach PDF.js');
        if (container.scrollTop < 1000) throw new Error('Native viewer did not scroll');
        let reloadBlankFrames = 0;
        for (let reload = 0; reload < 3; reload++) {
        const previous = window.tiptoptypPdf.activeFrame;
        const saved = { page: app.pdfViewer.currentPageNumber, scale: app.pdfViewer.currentScale, top: app.pdfViewer.container.scrollTop };
        const originalFetch = window.fetch;
        // Exercise an actual staged replacement in WKWebView using the same
        // fixture bytes under a fresh host revision; the fixture server is read-only here.
        window.fetch = async (...args) => {
          const response = await originalFetch(...args);
          if (args[0] !== '../state.json') return response;
          const state = await response.json();
          return new Response(JSON.stringify({...state, revision: state.revision + 1000 + reload}));
        };
        let measuring = true;
        function sample() {
          if (!measuring) return;
          const viewer = window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication.pdfViewer;
          if (!viewer._getVisiblePages().views.some(({view}) => view.renderingState === 3)) reloadBlankFrames++;
          requestAnimationFrame(sample);
        }
        requestAnimationFrame(sample);
        try { await window.tiptoptypPdf.refresh(); }
        finally { window.fetch = originalFetch; measuring = false; }
        if (window.tiptoptypPdf.activeFrame === previous) throw new Error('Native replacement did not commit');
        app = window.tiptoptypPdf.activeFrame.contentWindow.PDFViewerApplication;
        const replacement = window.tiptoptypPdf.activeFrame.contentDocument.getElementById('viewerContainer');
        if (app.pdfViewer.currentPageNumber !== saved.page || Math.abs(app.pdfViewer.currentScale - saved.scale) > .001 || Math.abs(replacement.scrollTop - saved.top) > 4) throw new Error('Native reload moved the viewport');
        if (reloadBlankFrames) throw new Error('Native reload exposed blank frames');
        }
        window.dispatchEvent(new CustomEvent('tiptoptyp-preview-zoom', {detail:'reset'}));
        app.pdfLinkService.setHash('page=1&zoom=page-width');
        await new Promise(resolve => setTimeout(resolve, 700));
        if (app.pdfViewer.currentScaleValue !== 'page-width') throw new Error('Zoom reset failed');
        return JSON.stringify({reloadCount:3, reloadBlankFrames, pages:app.pdfDocument.numPages, before, after, scale:app.pdfViewer.currentScaleValue,
          canvases:window.tiptoptypPdf.activeFrame.contentDocument.querySelectorAll('#viewer canvas').length, userAgent:navigator.userAgent});
        """, arguments: [:], in: nil, in: .page) { result in
            switch result {
            case .success(let value):
                print("WKWebView: \(value)")
                self.capture()
            case .failure(let error):
                print("WKWebView interaction failed: \(error)")
                exit(1)
            }
        }
    }

    func capture() {
        web.takeSnapshot(with: nil) { image, error in
            guard let image, let tiff = image.tiffRepresentation,
                  let bitmap = NSBitmapImageRep(data: tiff),
                  let png = bitmap.representation(using: .png, properties: [:]) else {
                print("WKWebView snapshot failed: \(String(describing: error))")
                exit(1)
            }
            do { try png.write(to: URL(fileURLWithPath: self.output)) }
            catch { print(error); exit(1) }
            print("Captured \(self.output)")
            NSApplication.shared.terminate(nil)
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        decisionHandler(action.request.url?.host == url.host ? .allow : .cancel)
    }
}

MainActor.assumeIsolated {
    let application = NSApplication.shared
    let probe = PdfJsProbe()
    application.delegate = probe
    application.setActivationPolicy(.accessory)
    application.run()
}
