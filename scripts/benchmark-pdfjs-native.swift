// Opt-in benchmark of the bundled PDF.js in native macOS WKWebView.
// Usage: probe SERVER_URL SCRIPT.js A.pdf B.pdf OUTPUT.json
import Cocoa
import WebKit

@MainActor
final class Benchmark: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var web: WKWebView!
    var started = CFAbsoluteTimeGetCurrent()
    var attempts = 0
    func applicationDidFinishLaunching(_ notification: Notification) {
        let config = WKWebViewConfiguration()
        // Isolate persisted browser data; assets warm naturally within this run.
        config.websiteDataStore = .nonPersistent()
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 900, height: 700), configuration: config)
        window = NSWindow(contentRect: web.frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "PDF reload benchmark"
        window.contentView = web
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        started = CFAbsoluteTimeGetCurrent()
        web.load(URLRequest(url: URL(string: CommandLine.arguments[1])!))
        DispatchQueue.main.asyncAfter(deadline: .now() + 240) { fputs("Benchmark timed out\n", stderr); exit(1) }
        ready()
    }
    func ready() {
        web.evaluateJavaScript("Boolean(window.tiptoptypPdf?.activeFrame)") { value, error in
            if value as? Bool == true { self.run() }
            else if self.attempts < 600 {
                self.attempts += 1
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { self.ready() }
            } else { print("Readiness failed: \(String(describing: error))"); exit(1) }
        }
    }
    func run() {
        do {
            let script = try String(contentsOfFile: CommandLine.arguments[2], encoding: .utf8)
            let a = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[3])).base64EncodedString()
            let b = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[4])).base64EncodedString()
            let cold = (CFAbsoluteTimeGetCurrent() - started) * 1000
            web.callAsyncJavaScript(script, arguments: ["a64": a, "b64": b, "coldMs": cold], in: nil, in: .page) { result in
                switch result {
                case .success(let value):
                    do {
                        guard let text = value as? String else { throw NSError(domain: "Missing result", code: 1) }
                        try text.write(toFile: CommandLine.arguments[5], atomically: true, encoding: .utf8)
                        print("Saved \(CommandLine.arguments[5])")
                        NSApplication.shared.terminate(nil)
                    } catch { print(error); exit(1) }
                case .failure(let error): print(error); exit(1)
                }
            }
        } catch { print(error); exit(1) }
    }
}
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let benchmark = Benchmark()
    app.delegate = benchmark
    app.setActivationPolicy(.accessory)
    app.run()
}
