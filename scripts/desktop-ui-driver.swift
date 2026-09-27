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
func nativeWindows() -> [AXUIElement] {
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 4)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { fail("cannot inspect native windows") }
    return windows
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}
if command == "zoom-focused" {
    let application = AXUIElementCreateApplication(pid)
    guard let focused = attribute(application, kAXFocusedWindowAttribute),
          let zoom = attribute(focused as! AXUIElement, kAXZoomButtonAttribute) else { fail("no focused window zoom button") }
    let result = AXUIElementPerformAction(zoom as! AXUIElement, kAXPressAction as CFString)
    guard result == .success else { fail("native zoom failed: \(result.rawValue)") }
    emit(["zoomed": true]); exit(0)
}
if command == "minimize-focused" {
    let application = AXUIElementCreateApplication(pid)
    guard let focused = attribute(application, kAXFocusedWindowAttribute) else { fail("no focused window") }
    let result = AXUIElementSetAttributeValue(focused as! AXUIElement, kAXMinimizedAttribute as CFString, kCFBooleanTrue)
    guard result == .success else { fail("accessibility minimize failed: \(result.rawValue)") }
    emit(["minimized": true]); exit(0)
}
if command == "resize-focused" {
    guard args.count == 4, let width = Double(args[2]), let height = Double(args[3]) else { fail("resize-focused requires width and height") }
    let application = AXUIElementCreateApplication(pid)
    guard let focused = attribute(application, kAXFocusedWindowAttribute) else { fail("no focused window") }
    var size = CGSize(width: width, height: height)
    let value = AXValueCreate(.cgSize, &size)!
    let result = AXUIElementSetAttributeValue(focused as! AXUIElement, kAXSizeAttribute as CFString, value)
    guard result == .success else { fail("accessibility resize failed: \(result.rawValue)") }
    emit(["resized": true]); exit(0)
}
if ["raise", "close", "restore"].contains(command) {
    guard args.count == 3 else { fail("window action requires exact title") }
    let matches = nativeWindows().filter { attribute($0, kAXTitleAttribute) as? String == args[2] }
    guard matches.count == 1 else { fail("window title is missing or ambiguous") }
    let window = matches[0]
    let status: AXError
    if command == "close" {
        guard let button = attribute(window, kAXCloseButtonAttribute) else { fail("window has no close button") }
        status = AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString)
    } else if command == "restore" {
        status = AXUIElementSetAttributeValue(window, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
    } else {
        status = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
    }
    guard status == .success else { fail("native window action failed: \(status.rawValue)") }
    emit(["performed": command]); exit(0)
}
if command == "foreground" {
    emit(["pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1]); exit(0)
}
if command == "finder" {
    guard let finder = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.finder").first else { fail("Finder is unavailable") }
    finder.activate(options: [])
    emit(["pid": finder.processIdentifier]); exit(0)
}
if command == "windows" {
    let windows = nativeWindows()
    var result: [[String: Any]] = []
    for window in windows {
        var row: [String: Any] = [:]
        for key in [kAXTitleAttribute, kAXMainAttribute, kAXFocusedAttribute, kAXMinimizedAttribute] {
            var field: CFTypeRef?
            if AXUIElementCopyAttributeValue(window, key as CFString, &field) == .success { row[key] = field }
        }
        if let value = attribute(window, kAXSizeAttribute), CFGetTypeID(value) == AXValueGetTypeID() {
            var size = CGSize.zero
            if AXValueGetValue(value as! AXValue, .cgSize, &size) { row["size"] = [size.width, size.height] }
        }
        result.append(row)
    }
    emit(["windows": result]); exit(0)
}
if command == "ax-set-text" {
    guard args.count == 3 else { fail("ax-set-text requires text") }
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 4)
    func setFirst(_ element: AXUIElement) -> Bool {
        if (attribute(element, kAXRoleAttribute) as? String) == kAXTextFieldRole,
           AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, args[2] as CFTypeRef) == .success {
            return true
        }
        var children: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &children) == .success,
              let values = children as? [AXUIElement] else { return false }
        return values.contains { setFirst($0) }
    }
    guard nativeWindows().contains(where: { setFirst($0) }) else { fail("no text field was available") }
    emit(["set": "text"]); exit(0)
}
if command == "ax-press-title" {
    guard args.count == 3 else { fail("ax-press-title requires title") }
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 4)
    func press(_ element: AXUIElement) -> Bool {
        if (attribute(element, kAXTitleAttribute) as? String) == args[2] {
            return AXUIElementPerformAction(element, kAXPressAction as CFString) == .success
        }
        var children: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &children) == .success,
              let values = children as? [AXUIElement] else { return false }
        return values.contains { press($0) }
    }
    guard nativeWindows().contains(where: { press($0) }) else { fail("button title was unavailable") }
    emit(["pressed": args[2]]); exit(0)
}
if command == "ax-press-description" {
    guard args.count == 3 else { fail("ax-press-description requires description") }
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 4)
    func press(_ element: AXUIElement) -> Bool {
        if (attribute(element, kAXDescriptionAttribute) as? String) == args[2] {
            return AXUIElementPerformAction(element, kAXPressAction as CFString) == .success
        }
        var children: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &children) == .success,
              let values = children as? [AXUIElement] else { return false }
        return values.contains { press($0) }
    }
    guard nativeWindows().contains(where: { press($0) }) else { fail("element description was unavailable") }
    emit(["pressed": args[2]]); exit(0)
}
guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fail("refusing input: target app is not foreground (actual: \(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "unknown") pid \(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1), target \(pid))") }
let source = CGEventSource(stateID: .hidSystemState)
if command == "paste" {
    guard args.count == 3 else { fail("paste requires fixture text") }
    let board = NSPasteboard.general
    let previous: [NSPasteboardItem] = (board.pasteboardItems ?? []).map { item in
        let copy = NSPasteboardItem()
        for type in item.types { if let data = item.data(forType: type) { copy.setData(data, forType: type) } }
        return copy
    }
    board.clearContents()
    board.setString(args[2], forType: .string)
    let written = board.changeCount
    defer {
        // Never overwrite a newer clipboard change made outside the fixture.
        if board.changeCount == written { board.clearContents(); board.writeObjects(previous) }
    }
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: down) else { fail("cannot create paste event") }
        event.flags = .maskCommand
        event.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    emit(["posted": "paste", "pid": pid])
    fflush(stdout)
    // Runner acknowledges the read-only content fingerprint before restoration.
    _ = readLine()
} else if command == "text" {
    guard args.count == 3 else { fail("text requires a string") }
    guard !args[2].contains("\n") && !args[2].contains("\r") else { fail("use acknowledged paste for multiline fixtures") }
    // Quartz carries only a bounded UTF-16 payload per event. Never truncate
    // a long fixture or split a surrogate pair across events.
    var chunks: [[UInt16]] = []
    var current: [UInt16] = []
    for scalar in args[2].unicodeScalars {
        let units = Array(String(scalar).utf16)
        if current.count + units.count > 16 { chunks.append(current); current = [] }
        current.append(contentsOf: units)
    }
    if !current.isEmpty { chunks.append(current) }
    for units in chunks {
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { fail("cannot create text event") }
            event.flags = []
            event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
            event.post(tap: .cghidEventTap)
            Thread.sleep(forTimeInterval: 0.05)
        }
    }
} else if command == "key" {
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
} else if command == "click" || command == "double-click" || command == "move" {
    guard args.count == 4, let x = Double(args[2]), let y = Double(args[3]), x.isFinite, y.isFinite else { fail("click requires finite screen coordinates") }
    let point = CGPoint(x: x, y: y)
    let types: [CGEventType] = command == "move" ? [.mouseMoved] : [.leftMouseDown, .leftMouseUp]
    let repetitions = command == "double-click" ? 2 : 1
    for repetition in 0..<repetitions {
      for type in types {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { fail("cannot create mouse event") }
        event.flags = []
        event.setIntegerValueField(.mouseEventClickState, value: type == .mouseMoved ? 0 : 1)
        event.post(tap: .cghidEventTap)
        // Preserve distinct move/down/up delivery across native event-loop turns.
        Thread.sleep(forTimeInterval: 0.05)
      }
      if repetition == 0 && command == "double-click" {
        Thread.sleep(forTimeInterval: 0.08)
      }
    }
} else if command == "drag" {
    guard args.count == 6,
          let x1 = Double(args[2]), let y1 = Double(args[3]),
          let x2 = Double(args[4]), let y2 = Double(args[5]),
          [x1, y1, x2, y2].allSatisfy(\.isFinite) else { fail("drag requires four finite coordinates") }
    let start = CGPoint(x: x1, y: y1)
    let end = CGPoint(x: x2, y: y2)
    guard let down = CGEvent(mouseEventSource: source, mouseType: .leftMouseDown, mouseCursorPosition: start, mouseButton: .left) else { fail("cannot create drag start") }
    down.flags = []
    down.setIntegerValueField(.mouseEventClickState, value: 1)
    down.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.08)
    for step in 1...8 {
        let fraction = CGFloat(step) / 8.0
        let point = CGPoint(x: x1 + (x2 - x1) * Double(fraction), y: y1 + (y2 - y1) * Double(fraction))
        guard let moved = CGEvent(mouseEventSource: source, mouseType: .leftMouseDragged, mouseCursorPosition: point, mouseButton: .left) else { fail("cannot create drag move") }
        moved.flags = []
        moved.setIntegerValueField(.mouseEventClickState, value: 1)
        moved.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    guard let up = CGEvent(mouseEventSource: source, mouseType: .leftMouseUp, mouseCursorPosition: end, mouseButton: .left) else { fail("cannot create drag end") }
    up.flags = []
    up.setIntegerValueField(.mouseEventClickState, value: 1)
    up.post(tap: .cghidEventTap)
} else { fail("unknown command") }
emit(["posted": command, "pid": pid])
