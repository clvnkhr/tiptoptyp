// Native WKWebView palette regression; driven by test-native-preview-palette.py.
// Unlike Chromium, this uses the installed macOS WebKit and an actual NSWindow.
import AppKit
import WebKit

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let url = URL(string: CommandLine.arguments[1])!
let output = URL(fileURLWithPath: CommandLine.arguments[3], isDirectory: true)
let replay = !CommandLine.arguments.contains("--without-load-replay")
let config = WKWebViewConfiguration()
let adapter = try String(contentsOfFile: CommandLine.arguments[2], encoding: .utf8)
config.userContentController.addUserScript(WKUserScript(source: adapter, injectionTime: .atDocumentStart, forMainFrameOnly: true))
// Creation-time appearance differs from the current appearance on reload.
config.userContentController.addUserScript(WKUserScript(source: "window.tiptoptypSetPalette?.([255,255,255],[0,0,0]);", injectionTime: .atDocumentStart, forMainFrameOnly: true))
let view = WKWebView(frame: NSRect(x: 0, y: 0, width: 800, height: 600), configuration: config)
let window = NSWindow(contentRect: view.frame, styleMask: .borderless, backing: .buffered, defer: false)
window.title = "Tinymist palette QA"
window.contentView = view
window.orderFrontRegardless()

class Delegate: NSObject, WKNavigationDelegate {
    var stage = 0
    let palettes = [([0,0,0],[255,255,255]), ([24,31,42],[200,210,220]), ([245,239,228],[42,35,26]), ([24,31,42],[200,210,220])]
    let names = ["dark", "comfy-dark", "comfy-light", "reload-dark"]
    func apply(_ view: WKWebView) {
        window.title = "Tinymist palette QA — " + names[stage]
        let (bg, fg) = palettes[stage]
        view.evaluateJavaScript("window.tiptoptypSetPalette?.(\(bg),\(fg));")
    }
    func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
        // The native app's Finished callback invalidates its queued-palette cache.
        // The Rust regression checks that invalidation independently.
        if stage == 0 || replay { apply(view) }
        waitForContent(view)
    }
    func waitForContent(_ view: WKWebView) {
        view.evaluateJavaScript("document.getElementById('typst-container')?.documents?.[0]?.impl?.hookedElem?.querySelectorAll('g[data-page-width]').length > 0") { value, error in
            if (value as? Bool) == true {
                view.evaluateJavaScript("getComputedStyle(document.getElementById('typst-container')).filter === 'none' && getComputedStyle(document.querySelector('#typst-container g[data-page-width]')).filter.includes('tiptoptyp-palette')") { safe, _ in
                    guard (safe as? Bool) == true else {
                        print("FAIL: filter must be inside SVG pages; native snapshots cannot validate HTML-host composition")
                        exit(1)
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now()+0.2) { self.capture(view) }
                }
            } else {
                DispatchQueue.main.asyncAfter(deadline: .now()+0.1) { self.waitForContent(view) }
            }
        }
    }
    func capture(_ view: WKWebView) {

        view.takeSnapshot(with: nil) { image, error in
            guard let data = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data),
                  let png = bitmap.representation(using: .png, properties: [:]) else {
                print("FAIL: native snapshot unavailable: \(String(describing: error))"); exit(2)
            }
            try! png.write(to: output.appendingPathComponent("wkwebview-\(self.names[self.stage]).png"))
            let (bg, fg) = self.palettes[self.stage]
            var paper = 0, ink = 0
            for y in stride(from: 0, to: bitmap.pixelsHigh, by: 3) {
                for x in stride(from: 0, to: bitmap.pixelsWide, by: 3) {
                    var samples = [Int](repeating: 0, count: bitmap.samplesPerPixel)
                    bitmap.getPixel(&samples, atX: x, y: y)
                    let rgb = Array(samples.prefix(3))
                    if zip(rgb,bg).allSatisfy({ abs($0-$1) <= 3 }) { paper += 1 }
                    if zip(rgb,fg).allSatisfy({ abs($0-$1) <= 3 }) { ink += 1 }
                }
            }
            print("\(self.names[self.stage]): paper=\(paper), ink=\(ink)")
            guard paper > 10000 && ink > 10 else { print("FAIL: rendered palette mismatch"); exit(1) }
            self.stage += 1
            if self.stage == self.palettes.count {
                if CommandLine.arguments.contains("--hold") {
                    print("Holding final dark preview for on-screen inspection")
                    DispatchQueue.main.asyncAfter(deadline: .now()+60) { exit(0) }
                } else { exit(0) }
                return
            }
            if self.stage == 3 {
                // A theme change immediately before navigation can land on the
                // old document. Without replay, the new page stays light.
                self.apply(view)
                view.reload()
            } else {
                self.apply(view)
                DispatchQueue.main.asyncAfter(deadline: .now()+0.2) { self.capture(view) }
            }
        }
    }
}
let delegate = Delegate()
view.navigationDelegate = delegate
view.load(URLRequest(url: url))
DispatchQueue.main.asyncAfter(deadline: .now()+(CommandLine.arguments.contains("--hold") ? 90 : 30)) { print("FAIL: native preview timed out"); exit(3) }
app.run()
