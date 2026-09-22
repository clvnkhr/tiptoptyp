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
        web.evaluateJavaScript("Boolean(window.PDFViewerApplication?.isInitialViewSet && document.querySelector('#viewer canvas'))") { value, error in
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
        web.callAsyncJavaScript("""
        const app = PDFViewerApplication;
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
        window.dispatchEvent(new CustomEvent('tiptoptyp-preview-zoom', {detail:'reset'}));
        app.pdfLinkService.setHash('page=1&zoom=page-width');
        await new Promise(resolve => setTimeout(resolve, 700));
        if (app.pdfViewer.currentScaleValue !== 'page-width') throw new Error('Zoom reset failed');
        return JSON.stringify({pages:app.pdfDocument.numPages, before, after, scale:app.pdfViewer.currentScaleValue,
          canvases:document.querySelectorAll('#viewer canvas').length, userAgent:navigator.userAgent});
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
