// Native input adapter. It has no access to editor command handlers or state.
import AppKit
import ApplicationServices

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2)
}
func emit(_ value: Any) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}
let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { fail("missing command") }
if command == "preflight" {
    let trusted = AXIsProcessTrusted()
    let input = CGPreflightPostEventAccess()
    let desktop = NSWorkspace.shared.frontmostApplication != nil
    emit(["accessibility": trusted, "input": input, "desktop": desktop])
    exit(trusted && input && desktop ? 0 : 2)
}
guard args.count >= 2, let pid = Int32(args[1]),
      let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated else { fail("target process is not running") }
if command == "activate" {
    app.activate(options: [.activateAllWindows])
    let deadline = Date().addingTimeInterval(3)
    while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && Date() < deadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.02))
    }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fail("target app did not activate") }
    emit(["active": true]); exit(0)
}
if command == "windows" {
    let element = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value) == .success else { fail("cannot inspect native windows") }
    let windows = value as! [AXUIElement]
    var result: [[String: Any]] = []
    for window in windows {
        var row: [String: Any] = [:]
        for key in [kAXTitleAttribute, kAXMainAttribute, kAXFocusedAttribute, kAXMinimizedAttribute] {
            var field: CFTypeRef?
            if AXUIElementCopyAttributeValue(window, key as CFString, &field) == .success { row[key] = field }
        }
        result.append(row)
    }
    emit(["windows": result]); exit(0)
}
guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fail("refusing input: target app is not foreground") }
let source = CGEventSource(stateID: .hidSystemState)
if command == "key" {
    guard args.count == 4, let code = UInt16(args[2]) else { fail("key requires keycode and flags") }
    var flags: CGEventFlags = []
    for flag in args[3].split(separator: "+") {
        switch flag {
        case "cmd": flags.insert(.maskCommand)
        case "ctrl": flags.insert(.maskControl)
        case "alt": flags.insert(.maskAlternate)
        case "shift": flags.insert(.maskShift)
        case "none": break
        default: fail("unknown modifier")
        }
    }
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else { fail("cannot create keyboard event") }
        event.flags = flags
        event.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
} else if command == "click" {
    guard args.count == 4, let x = Double(args[2]), let y = Double(args[3]), x.isFinite, y.isFinite else { fail("click requires finite screen coordinates") }
    let point = CGPoint(x: x, y: y)
    for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { fail("cannot create mouse event") }
        event.setIntegerValueField(.mouseEventClickState, value: type == .mouseMoved ? 0 : 1)
        event.post(tap: .cghidEventTap)
        // Preserve distinct move/down/up delivery across native event-loop turns.
        Thread.sleep(forTimeInterval: 0.05)
    }
} else { fail("unknown command") }
emit(["posted": command, "pid": pid])
