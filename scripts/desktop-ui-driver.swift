// Native input adapter. It has no access to editor command handlers or state.
import AppKit
import ApplicationServices
import Carbon

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2)
}
func emit(_ value: Any) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}
// TIS queries inspect only already-enabled input sources. A scoped fixture
// changes the current source temporarily; it never enables/disables sources or
// writes the user's input-method configuration.
func inputProperty<T>(_ source: TISInputSource, _ key: CFString, _: T.Type) -> T? {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return nil }
    return Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue() as? T
}
func inputSourceID(_ source: TISInputSource) -> String {
    inputProperty(source, kTISPropertyInputSourceID, String.self) ?? ""
}
func currentInputSourceID() -> String {
    inputSourceID(TISCopyCurrentKeyboardInputSource().takeRetainedValue())
}
func enabledInputSources() -> [TISInputSource] {
    TISCreateInputSourceList(nil, false).takeRetainedValue() as! [TISInputSource]
}
func inputSourceRows() -> [[String: Any]] {
    let selected = currentInputSourceID()
    return enabledInputSources().map { source in
        let id = inputSourceID(source)
        return ["id": id,
                "name": inputProperty(source, kTISPropertyLocalizedName, String.self) ?? "",
                "enabled": inputProperty(source, kTISPropertyInputSourceIsEnabled, Bool.self) ?? false,
                "selectable": inputProperty(source, kTISPropertyInputSourceIsSelectCapable, Bool.self) ?? false,
                "selected": id == selected]
    }
}
func waitForInputSource(_ id: String) -> Bool {
    let deadline = Date().addingTimeInterval(2)
    while currentInputSourceID() != id && Date() < deadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.02))
    }
    return currentInputSourceID() == id
}
func scopedInputSource(_ requested: String) throws {
    let original = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    let originalID = inputSourceID(original)
    guard let source = enabledInputSources().first(where: {
        inputSourceID($0) == requested
            && inputProperty($0, kTISPropertyInputSourceIsEnabled, Bool.self) == true
            && inputProperty($0, kTISPropertyInputSourceIsSelectCapable, Bool.self) == true
    }) else {
        throw NSError(domain: "DesktopInputSource", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: "requested input source is not already enabled and selectable"])
    }
    defer {
        let current = currentInputSourceID()
        let external = current != requested && current != originalID
        // Preserve any newer source selected by the human during the fixture.
        let status = external ? OSStatus(paramErr) : TISSelectInputSource(original)
        let restored = !external && status == noErr && waitForInputSource(originalID)
        emit(["restored": restored, "original": originalID,
              "current": currentInputSourceID(), "external_preserved": external])
        fflush(stdout)
    }
    guard TISSelectInputSource(source) == noErr, waitForInputSource(requested) else {
        throw NSError(domain: "DesktopInputSource", code: 2,
                      userInfo: [NSLocalizedDescriptionKey: "input source did not become selected"])
    }
    emit(["selected": requested, "original": originalID]); fflush(stdout)
    // EOF also restores: a failed/crashed Python journey closes this pipe.
    _ = readLine()
}

let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { fail("missing command") }
if command == "preflight" {
    let trusted = AXIsProcessTrusted()
    let input = CGPreflightPostEventAccess()
    let session = CGSessionCopyCurrentDictionary() as? [String: Any]
    let locked = session?["CGSSessionScreenIsLocked"] as? Bool ?? false
    let desktop = NSWorkspace.shared.frontmostApplication != nil && !locked
    // Observation only: never modify the user's Accessibility Zoom preference.
    let scrollZoom = UserDefaults(suiteName: "com.apple.universalaccess")?.object(forKey: "closeViewScrollWheelToggle") ?? NSNull()
    emit(["accessibility": trusted, "input": input, "desktop": desktop, "locked": locked, "system_scroll_zoom": scrollZoom,
          "screens": NSScreen.screens.map { ["width": $0.frame.width, "height": $0.frame.height, "scale": $0.backingScaleFactor] }])
    exit(trusted && input && desktop ? 0 : 2)
}
guard args.count >= 2, let pid = Int32(args[1]),
      let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated else { fail("target process is not running") }
if command == "input-sources" {
    emit(["selected": currentInputSourceID(), "sources": inputSourceRows()]); exit(0)
}
if command == "input-source-scope" {
    guard args.count == 3 else { fail("input-source-scope requires an enabled source ID") }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        fail("refusing input-source fixture: target app is not foreground")
    }
    do { try scopedInputSource(args[2]) }
    catch { fail(error.localizedDescription) }
    exit(0)
}
if command == "pointer-state" {
    let point = CGEvent(source: nil)?.location ?? .zero
    emit(["left": CGEventSource.buttonState(.combinedSessionState, button: .left),
          "x": point.x, "y": point.y]); exit(0)
}
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
if command == "fullscreen" {
    guard args.count == 4, ["true", "false"].contains(args[3]) else { fail("fullscreen requires exact title and true/false") }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        fail("refusing fullscreen request: target app is not foreground")
    }
    let matches = nativeWindows().filter { attribute($0, kAXTitleAttribute) as? String == args[2] }
    guard matches.count == 1 else { fail("fullscreen window title is missing or ambiguous") }
    let window = matches[0]
    let name = "AXFullScreen" as CFString
    var settable = DarwinBoolean(false)
    guard AXUIElementIsAttributeSettable(window, name, &settable) == .success, settable.boolValue else {
        fail("native window does not support setting AXFullScreen")
    }
    let desired = args[3] == "true"
    let result = AXUIElementSetAttributeValue(window, name, desired ? kCFBooleanTrue : kCFBooleanFalse)
    guard result == .success else { fail("native fullscreen request failed: \(result.rawValue)") }
    // Acceptance is not completion; callers must wait for app/OS state.
    emit(["fullscreen_requested": desired]); exit(0)
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
    let foreground = NSWorkspace.shared.frontmostApplication
    emit(["pid": foreground?.processIdentifier ?? -1,
          "bundle": foreground?.bundleIdentifier ?? ""]); exit(0)
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
// File dialogs can be attached sheets absent from AXWindows. Never search
// the editor/preview tree: only native dialog roots and their controls qualify.
struct DialogMatch {
    var elements: [AXUIElement] = []
    var complete = true
    var visited = 0
}
func dialogMatches(_ expected: String, description: Bool = false) -> DialogMatch {
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 0.15)
    let deadline = Date().addingTimeInterval(1)
    var result = DialogMatch()
    func read(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        guard Date() < deadline else { result.complete = false; return nil }
        return attribute(element, name)
    }
    func uniqueAppend(_ element: AXUIElement, to elements: inout [AXUIElement]) {
        if !elements.contains(where: { CFEqual($0, element) }) { elements.append(element) }
    }
    var windows = read(application, kAXWindowsAttribute) as? [AXUIElement] ?? []
    if let focused = read(application, kAXFocusedWindowAttribute),
       CFGetTypeID(focused) == AXUIElementGetTypeID() {
        uniqueAppend(focused as! AXUIElement, to: &windows)
    }
    var pending: [(AXUIElement, Int)] = []
    for window in windows.prefix(32) {
        let sheets = read(window, "AXSheets") as? [AXUIElement] ?? []
        pending.append(contentsOf: sheets.map { ($0, 0) })
        let role = read(window, kAXRoleAttribute) as? String
        let subrole = read(window, kAXSubroleAttribute) as? String
        if role == "AXSheet" || role == "AXDialog" || subrole == "AXDialog" || subrole == "AXSystemDialog" {
            pending.append((window, 0))
        }
    }
    var roots: [AXUIElement] = []
    var seen: [AXUIElement] = []
    while !pending.isEmpty && result.complete {
        let (element, depth) = pending.removeFirst()
        if seen.contains(where: { CFEqual($0, element) }) { continue }
        uniqueAppend(element, to: &seen)
        guard seen.count <= 128 && depth <= 8 else { result.complete = false; break }
        let sheets = read(element, "AXSheets") as? [AXUIElement] ?? []
        if sheets.isEmpty { uniqueAppend(element, to: &roots) }
        else { pending.append(contentsOf: sheets.map { ($0, depth + 1) }) }
    }
    // An exact native default button is authoritative, and avoids scanning the
    // file browser. Multiple dialog defaults are ambiguous and never pressed.
    if !description {
        for root in roots {
            if let value = read(root, kAXDefaultButtonAttribute),
               CFGetTypeID(value) == AXUIElementGetTypeID() {
                let button = value as! AXUIElement
                if read(button, kAXRoleAttribute) as? String == kAXButtonRole
                    && read(button, kAXTitleAttribute) as? String == expected {
                    uniqueAppend(button, to: &result.elements)
                }
            }
        }
        if !result.elements.isEmpty { return result }
    }
    pending = roots.map { ($0, 0) }
    seen.removeAll(keepingCapacity: true)
    while !pending.isEmpty && result.complete {
        let (element, depth) = pending.removeFirst()
        if seen.contains(where: { CFEqual($0, element) }) { continue }
        uniqueAppend(element, to: &seen)
        result.visited += 1
        guard result.visited <= 128 && depth <= 8 else { result.complete = false; break }
        let role = read(element, kAXRoleAttribute) as? String
        if ["AXWebArea", "AXOutline", "AXTable", "AXBrowser", "AXScrollArea"].contains(role ?? "") { continue }
        let name = description ? kAXDescriptionAttribute : kAXTitleAttribute
        if (description || role == kAXButtonRole) && read(element, name) as? String == expected {
            uniqueAppend(element, to: &result.elements)
        }
        let children = read(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
        pending.append(contentsOf: children.map { ($0, depth + 1) })
    }
    return result
}
if command == "ax-dialog-state" {
    // Read-only phase evidence. Never emit the focused field's value: it may
    // contain source text or a human-entered path in a different application.
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 0.15)
    let deadline = Date().addingTimeInterval(1)
    var complete = true
    func read(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        guard Date() < deadline else { complete = false; return nil }
        return attribute(element, name)
    }
    func row(_ element: AXUIElement) -> [String: Any] {
        var result: [String: Any] = [:]
        for (key, attributeName) in [("role", kAXRoleAttribute), ("subrole", kAXSubroleAttribute),
                                     ("identifier", "AXIdentifier"), ("description", kAXDescriptionAttribute),
                                     ("title", kAXTitleAttribute)] {
            result[key] = read(element, attributeName) as? String ?? ""
        }
        result["enabled"] = read(element, kAXEnabledAttribute) as? Bool ?? false
        result["focused"] = read(element, kAXFocusedAttribute) as? Bool ?? false
        return result
    }
    var ancestry: [[String: Any]] = []
    var current = read(application, kAXFocusedUIElementAttribute)
    var seen: [AXUIElement] = []
    while let value = current, CFGetTypeID(value) == AXUIElementGetTypeID(), complete {
        let element = value as! AXUIElement
        if seen.contains(where: { CFEqual($0, element) }) { break }
        guard seen.count < 12 else { complete = false; break }
        seen.append(element)
        ancestry.append(row(element))
        if ancestry.last?["role"] as? String == kAXApplicationRole { break }
        current = read(element, kAXParentAttribute)
    }
    var focusedWindow: [String: Any] = [:]
    if let value = read(application, kAXFocusedWindowAttribute),
       CFGetTypeID(value) == AXUIElementGetTypeID() {
        focusedWindow = row(value as! AXUIElement)
    }
    emit(["complete": complete, "focused_ancestry": ancestry,
          "focused_window": focusedWindow]); exit(0)
}
if command == "ax-button-title" || command == "ax-press-title" || command == "ax-press-description" {
    guard args.count == 3 else { fail("native dialog lookup requires exact text") }
    let result = dialogMatches(args[2], description: command == "ax-press-description")
    let element = result.elements.count == 1 ? result.elements[0] : nil
    let role = element.flatMap { attribute($0, kAXRoleAttribute) as? String } ?? ""
    let title = element.flatMap { attribute($0, kAXTitleAttribute) as? String } ?? ""
    let enabled = element.flatMap { attribute($0, kAXEnabledAttribute) as? Bool } ?? false
    if command == "ax-button-title" {
        emit(["requested_title": args[2], "count": result.elements.count,
              "complete": result.complete, "role": role, "title": title,
              "enabled": enabled, "visited": result.visited]); exit(0)
    }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        fail("refusing native dialog action: target app is not foreground")
    }
    guard result.complete, let element = element, enabled else {
        fail("native dialog control is missing, disabled, ambiguous, or exceeded its lookup budget")
    }
    guard AXUIElementPerformAction(element, kAXPressAction as CFString) == .success else {
        fail("native dialog action failed")
    }
    emit(["pressed": args[2]]); exit(0)
}
guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fail("refusing input: target app is not foreground (actual: \(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "unknown") pid \(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1), target \(pid))") }
let source = CGEventSource(stateID: .hidSystemState)
if command == "watch-copy" {
    guard args.count == 3 else { fail("watch-copy requires expected fixture text") }
    let board = NSPasteboard.general
    let initialCount = board.changeCount
    let previous: [NSPasteboardItem] = (board.pasteboardItems ?? []).map { item in
        let copy = NSPasteboardItem()
        for type in item.types { if let data = item.data(forType: type) { copy.setData(data, forType: type) } }
        return copy
    }
    var matchedCount: Int?
    defer {
        // Restore only an observed fixture Copy; an external/newer clipboard
        // change remains untouched. Never send clipboard contents to stdout.
        if let written = matchedCount, board.changeCount == written {
            board.clearContents(); board.writeObjects(previous)
        }
    }
    emit(["ready": "copy"])
    fflush(stdout)
    while let request = readLine(), request == "check" {
        let observed = board.changeCount
        let matches = observed != initialCount && board.string(forType: .string) == args[2]
            && board.changeCount == observed
        if matches { matchedCount = observed }
        emit(["matches": matches])
        fflush(stdout)
    }
} else if command == "paste" {
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
} else if command == "key" || command == "key-burst" {
    guard args.count == (command == "key" ? 4 : 6), let code = UInt16(args[2]) else { fail("key requires keycode and flags; key-burst also requires count and interval milliseconds") }
    let count = command == "key" ? 1 : Int(args[4]) ?? 0
    let interval = command == "key" ? 0.1 : (Double(args[5]) ?? 0) / 1000
    guard (1...60).contains(count), interval.isFinite, (0.005...0.5).contains(interval), Double(count) * interval <= 3 else { fail("invalid bounded key burst") }
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
    for _ in 0..<count {
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else { fail("cannot create keyboard event") }
            event.flags = flags
            event.post(tap: .cghidEventTap)
            Thread.sleep(forTimeInterval: interval / 2)
        }
    }
} else if command == "wheel" || command == "zoom-wheel" || command == "horizontal-wheel" {
    guard args.count == 5,
          let x = Double(args[2]), let y = Double(args[3]),
          let lines = Int32(args[4]), x.isFinite, y.isFinite, lines != 0 else {
        fail("wheel requires finite screen coordinates and a nonzero line delta")
    }
    let horizontal = command == "horizontal-wheel"
    guard let event = CGEvent(scrollWheelEvent2Source: source, units: .line,
                              wheelCount: horizontal ? 2 : 1,
                              wheel1: horizontal ? 0 : lines,
                              wheel2: horizontal ? lines : 0, wheel3: 0) else {
        fail("cannot create wheel event")
    }
    event.location = CGPoint(x: x, y: y)
    if command == "zoom-wheel" { event.flags = .maskControl }
    event.post(tap: .cghidEventTap)
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
} else if command == "right-click" {
    guard args.count == 4, let x = Double(args[2]), let y = Double(args[3]), x.isFinite, y.isFinite else { fail("right-click requires finite screen coordinates") }
    let point = CGPoint(x: x, y: y)
    for type in [CGEventType.rightMouseDown, CGEventType.rightMouseUp] {
      guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .right) else { fail("cannot create mouse event") }
      event.flags = []
      event.setIntegerValueField(.mouseEventClickState, value: 1)
      event.post(tap: .cghidEventTap)
      // Preserve distinct down/up delivery across native event-loop turns.
      Thread.sleep(forTimeInterval: 0.05)
    }
} else if ["drag-start", "drag-move", "drag-end"].contains(command) {
    guard args.count == 4, let x = Double(args[2]), let y = Double(args[3]),
          x.isFinite, y.isFinite else { fail("drag phase requires finite coordinates") }
    let point = CGPoint(x: x, y: y)
    if command == "drag-start" {
        guard let approach = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                                    mouseCursorPosition: point, mouseButton: .left) else { fail("cannot approach drag start") }
        approach.flags = []; approach.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    let type: CGEventType = command == "drag-start" ? .leftMouseDown
        : command == "drag-move" ? .leftMouseDragged : .leftMouseUp
    guard let event = CGEvent(mouseEventSource: source, mouseType: type,
                             mouseCursorPosition: point, mouseButton: .left) else { fail("cannot create drag phase") }
    event.flags = []; event.setIntegerValueField(.mouseEventClickState, value: 1)
    event.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.05)
} else if command == "drag" {
    guard args.count == 6,
          let x1 = Double(args[2]), let y1 = Double(args[3]),
          let x2 = Double(args[4]), let y2 = Double(args[5]),
          [x1, y1, x2, y2].allSatisfy(\.isFinite) else { fail("drag requires four finite coordinates") }
    let start = CGPoint(x: x1, y: y1)
    let end = CGPoint(x: x2, y: y2)
    // A real drag first moves onto its handle. Posting down at a new location
    // alone can leave the previous egui hit-test owner active for that event.
    guard let approach = CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: start, mouseButton: .left) else { fail("cannot approach drag start") }
    approach.flags = []
    approach.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.05)
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
