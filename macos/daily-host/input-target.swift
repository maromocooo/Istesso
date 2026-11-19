// Istesso Dev Input Target (com.maromoco.istesso-dev.input-target): a window that only counts what
// reaches it, for checking that the Mac host's input arrives in ANOTHER process's window
// (docs/m4-plan.md 5c, "ほかのプロセスの窓への直送"). Development only, never shipped.
//
// One window with a text area. Every half second it writes one line of JSON to the report file
// given as its argument: how many key presses, modifier changes, clicks (and their click counts),
// drags, scrolls and inserted characters came, and where the last click landed in the window, in
// its own points from the top left. What was typed is not written: only how many characters.
// It plays no sound. It ends after the number of seconds given (120 if not), or when its window
// is closed.
//
// Usage: istesso-input-target [REPORT [SECONDS]]   (REPORT: ~/.cache/istesso-daily-mac/input-target.jsonl)

import AppKit
import Carbon

final class Counting: NSTextView {
    var counts: [String: Int] = [:]
    var lastClick: NSPoint?
    private var downAt: [UInt16: TimeInterval] = [:]
    override func keyDown(with event: NSEvent) {
        counts["key", default: 0] += 1
        if event.isARepeat { counts["repeat", default: 0] += 1 } else { downAt[event.keyCode] = event.timestamp }
        super.keyDown(with: event)
    }
    override func keyUp(with event: NSEvent) {
        counts["key_up", default: 0] += 1
        if let at = downAt.removeValue(forKey: event.keyCode) {
            // How long the key was down, in milliseconds (the longest so far); never which key.
            counts["held_ms_max"] = max(counts["held_ms_max"] ?? 0, Int((event.timestamp - at) * 1000))
        }
        super.keyUp(with: event)
    }
    override func flagsChanged(with event: NSEvent) {
        counts["flags", default: 0] += 1
        super.flagsChanged(with: event)
    }
    override func mouseDown(with event: NSEvent) {
        counts["click", default: 0] += 1
        counts["click_count_max"] = max(counts["click_count_max"] ?? 0, event.clickCount)
        let inView = convert(event.locationInWindow, from: nil)
        lastClick = NSPoint(x: inView.x, y: bounds.height - inView.y)
        super.mouseDown(with: event)
    }
    override func rightMouseDown(with event: NSEvent) {
        counts["right_click", default: 0] += 1
    }
    override func mouseDragged(with event: NSEvent) {
        counts["drag", default: 0] += 1
        super.mouseDragged(with: event)
    }
    override func mouseMoved(with event: NSEvent) {
        counts["move", default: 0] += 1
    }
    override func scrollWheel(with event: NSEvent) {
        counts["scroll", default: 0] += 1
    }
    override func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        counts["inserted_characters", default: 0] += text.count
        super.insertText(string, replacementRange: replacementRange)
    }
    // No sound, ever: a command nobody handles is dropped instead of reaching the beep.
    override func doCommand(by selector: Selector) {
        if responds(to: selector) { super.doCommand(by: selector) }
    }
}

final class Quiet: NSWindow {
    override func noResponder(for eventSelector: Selector) {}
}

final class Target: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let report: String
    let seconds: Double
    var window: NSWindow!
    var view: Counting!
    var secure: NSSecureTextField!
    var secureInputSeen = false
    init(report: String, seconds: Double) {
        self.report = report
        self.seconds = seconds
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = Quiet(contentRect: NSRect(x: 200, y: 200, width: 640, height: 400),
                          styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Istesso Dev Input Target"
        window.delegate = self
        // A password field along the bottom (40 points), the text area above it. The field counts
        // how many characters it holds, never which.
        let bounds = window.contentView!.bounds
        secure = NSSecureTextField(frame: NSRect(x: 10, y: 6, width: bounds.width - 20, height: 28))
        secure.autoresizingMask = [.width]
        window.contentView!.addSubview(secure)
        view = Counting(frame: NSRect(x: 0, y: 40, width: bounds.width, height: bounds.height - 40))
        view.autoresizingMask = [.width, .height]
        view.font = NSFont.systemFont(ofSize: 24)
        window.contentView!.addSubview(view)
        window.acceptsMouseMovedEvents = true
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)
        Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in self?.write() }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { NSApp.terminate(nil) }
    }
    func write() {
        var line = "{\"t\":\(Date().timeIntervalSince1970),\"window\":\(window.windowNumber),\"active\":\(NSApp.isActive)"
        for (key, value) in view.counts.sorted(by: { $0.key < $1.key }) { line += ",\"\(key)\":\(value)" }
        // The password field: how many characters it holds, and whether macOS's secure input was on.
        if IsSecureEventInputEnabled() { secureInputSeen = true }
        line += ",\"secure_characters\":\(secure.stringValue.count),\"secure_input_seen\":\(secureInputSeen)"
        if let click = view.lastClick { line += ",\"last_click\":[\(Int(click.x)),\(Int(click.y))]" }
        line += "}\n"
        if let handle = FileHandle(forWritingAtPath: report) {
            handle.seekToEndOfFile()
            handle.write(Data(line.utf8))
            handle.closeFile()
        } else {
            FileManager.default.createFile(atPath: report, contents: Data(line.utf8))
        }
    }
    func windowWillClose(_ notification: Notification) {
        write()
        NSApp.terminate(nil)
    }
}

let arguments = CommandLine.arguments.filter { !$0.hasPrefix("-psn_") }
let report = arguments.count > 1 ? arguments[1] : NSHomeDirectory() + "/.cache/istesso-daily-mac/input-target.jsonl"
let target = Target(report: report, seconds: arguments.count > 2 ? Double(arguments[2]) ?? 120 : 120)
let app = NSApplication.shared
app.setActivationPolicy(.regular)
app.delegate = target
app.run()
