// istesso-lock-check: can a Mac host go on working while its screen is locked?
//
// A Mac host is used with a person logged in and the screen locked (docs/m4-plan.md, 4.4).
// This program answers, on the Mac it runs on, with yes or no for each item:
//
//   - is the picture of a window still captured while the screen is locked?
//   - does it matter whether the display is asleep?
//   - is it captured right after the Mac wakes from sleep?
//   - do a key and the pointer reach an application while the screen is locked?
//
// It opens one window of its own, captures that window and nothing else (ScreenCaptureKit),
// and sends a key that types nothing (F13), a pointer movement and a click to that window
// (CGEvent). The reasons for the two permissions are in docs/adr/0017-mac-host-permissions.md.
// It never asks for a permission unless started with --ask; without them it says which one
// is missing and does nothing.
//
// Usage (docs/m4-plan.md, "手順 0b"):
//   open "Istesso Lock Check.app" --args [--ask] [--report <file>] [--oversize]
// Leave it running, lock the screen, let the display go to sleep, put the Mac to sleep and
// wake it, unlock. Each state is tried once, by itself; the answers are in the report file.
//
// THIS PROGRAM HAS BEEN COMPILED BUT NOT RUN: running it needs the permissions.

import AppKit
import CoreGraphics
import ScreenCaptureKit

// MARK: - What the window shows, and what it is told

/// A view that fills itself with one colour of a fixed series and notes the input it gets.
final class ProbeView: NSView {
    static let colours: [(UInt8, UInt8, UInt8)] = [
        (220, 40, 40), (40, 200, 60), (50, 80, 230), (230, 200, 40), (200, 60, 210), (40, 200, 210),
    ]
    private(set) var step = 0
    var keys = 0
    var moves = 0
    var clicks = 0
    var status = "starting" { didSet { needsDisplay = true } }

    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }

    func advance() {
        step += 1
        needsDisplay = true
    }

    var colour: (UInt8, UInt8, UInt8) { ProbeView.colours[step % ProbeView.colours.count] }

    override func draw(_ dirtyRect: NSRect) {
        let (r, g, b) = colour
        NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1).setFill()
        bounds.fill()
        // The text stays clear of the middle, where the captured colour is read.
        let text = "Istesso lock check\n\(status)" as NSString
        text.draw(
            in: NSRect(x: 12, y: 8, width: bounds.width - 24, height: bounds.height / 3),
            withAttributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.white])
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == Probe.keyF13 { keys += 1 }
    }
    override func mouseMoved(with event: NSEvent) { moves += 1 }
    override func mouseDown(with event: NSEvent) { clicks += 1 }
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(
            NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self))
    }
}

/// A window that may be larger than the screen (a window with a title bar is kept on it).
final class FreeWindow: NSWindow {
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
    override var canBecomeKey: Bool { true }
}

// MARK: - Capture of the one window

final class Capture: NSObject, SCStreamOutput, SCStreamDelegate {
    private let lock = NSLock()
    private var stream: SCStream?
    private var frames = 0
    private var seen: [(UInt8, UInt8, UInt8)] = []
    private(set) var size = (0, 0)
    private(set) var stopped: String?
    private let queue = DispatchQueue(label: "dev.istesso.lock-check.capture")

    /// Starts capturing the window with this number. Calls back with an error text, or nil.
    func start(windowNumber: Int, pixelWidth: Int, pixelHeight: Int, done: @escaping (String?) -> Void) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: false) { content, error in
            guard let window = content?.windows.first(where: { Int($0.windowID) == windowNumber }) else {
                done("the window is not among what may be captured: \(error?.localizedDescription ?? "not listed")")
                return
            }
            let configuration = SCStreamConfiguration()
            configuration.width = pixelWidth
            configuration.height = pixelHeight
            configuration.showsCursor = false
            configuration.pixelFormat = kCVPixelFormatType_32BGRA
            configuration.minimumFrameInterval = CMTime(value: 1, timescale: 30)
            configuration.queueDepth = 3
            let stream = SCStream(
                filter: SCContentFilter(desktopIndependentWindow: window), configuration: configuration, delegate: self)
            do {
                try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: self.queue)
            } catch {
                done("the capture would not take an output: \(error.localizedDescription)")
                return
            }
            self.stream = stream
            stream.startCapture { error in done(error?.localizedDescription) }
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let image = sampleBuffer.imageBuffer else { return }
        // Frames that say "nothing changed" carry no picture; only complete ones are counted.
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
            as? [[SCStreamFrameInfo: Any]],
            let raw = attachments.first?[.status] as? Int, let status = SCFrameStatus(rawValue: raw),
            status != .complete
        {
            return
        }
        CVPixelBufferLockBaseAddress(image, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(image, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(image) else { return }
        let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image))
        let pixel = base.advanced(by: (height / 2) * CVPixelBufferGetBytesPerRow(image) + (width / 2) * 4)
            .assumingMemoryBound(to: UInt8.self)
        let colour = (pixel[2], pixel[1], pixel[0])
        lock.lock()
        frames += 1
        size = (width, height)
        if seen.last.map({ $0 != colour }) ?? true { seen.append(colour) }
        lock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        lock.lock()
        stopped = error.localizedDescription
        lock.unlock()
    }

    /// Frames and the colours they showed, in order, since the last call.
    func take() -> (frames: Int, colours: [(UInt8, UInt8, UInt8)], stopped: String?) {
        lock.lock()
        defer {
            frames = 0
            seen = []
            lock.unlock()
        }
        return (frames, seen, stopped)
    }
}

// MARK: - The trials

struct Answer {
    var state: String
    var frames = 0
    var live = false
    var stopped: String?
    var key = false
    var move = false
    var click = false
    var inputTried = false
    var at = Date()
}

final class Probe: NSObject, NSApplicationDelegate {
    static let keyF13: UInt16 = 0x69

    private var window: NSWindow!
    private var view: ProbeView!
    private let capture = Capture()
    private var answers: [Answer] = []
    private var tried: Set<String> = []
    private var running = false
    private var wokeAt: Date?
    private var captureProblem: String?
    private let report: URL
    private let ask: Bool
    private let oversize: Bool
    private var mayCapture = false
    private var mayPost = false

    init(arguments: [String]) {
        ask = arguments.contains("--ask")
        oversize = arguments.contains("--oversize")
        if let index = arguments.firstIndex(of: "--report"), index + 1 < arguments.count {
            report = URL(fileURLWithPath: arguments[index + 1])
        } else {
            report = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".cache/istesso-m4-wip/lock-check-report.txt")
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let screen = NSScreen.main?.frame ?? NSRect(x: 0, y: 0, width: 1280, height: 720)
        var frame = NSRect(x: screen.midX - 320, y: screen.midY - 200, width: 640, height: 400)
        if oversize {
            // Half as wide again as the screen: is a window captured whole when it does not fit?
            frame = NSRect(x: screen.minX, y: screen.midY - 200, width: screen.width * 1.5, height: 400)
        }
        window = FreeWindow(
            contentRect: frame, styleMask: oversize ? [.borderless] : [.titled, .closable], backing: .buffered,
            defer: false)
        window.title = "Istesso lock check"
        view = ProbeView(frame: NSRect(origin: .zero, size: frame.size))
        window.contentView = view
        window.acceptsMouseMovedEvents = true
        window.setFrame(frame, display: true)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)

        // Only read whether the permissions are there. Asking is the person's choice (--ask).
        mayCapture = CGPreflightScreenCaptureAccess()
        mayPost = CGPreflightPostEventAccess()
        if ask {
            if !mayCapture { mayCapture = CGRequestScreenCaptureAccess() }
            if !mayPost { mayPost = CGRequestPostEventAccess() }
        }
        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(woke), name: NSWorkspace.didWakeNotification, object: nil)
        write()
        guard mayCapture else {
            view.status = "screen recording is not allowed for this program: nothing is captured (see the report)"
            return
        }
        let scale = window.backingScaleFactor
        capture.start(
            windowNumber: window.windowNumber, pixelWidth: Int(frame.width * scale),
            pixelHeight: Int(frame.height * scale)
        ) { problem in
            DispatchQueue.main.async {
                self.captureProblem = problem
                self.view.status = problem ?? "capturing; lock the screen, let the display sleep, sleep and wake the Mac"
                self.write()
                Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { _ in self.tick() }
            }
        }
    }

    @objc private func woke() { wokeAt = Date() }

    private var locked: Bool {
        (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool ?? false
    }

    private var state: String {
        if let wokeAt, Date().timeIntervalSince(wokeAt) < 20 {
            return locked ? "right after wake, locked" : "right after wake, unlocked"
        }
        let asleep = CGDisplayIsAsleep(CGMainDisplayID()) != 0
        switch (locked, asleep) {
        case (false, false): return "unlocked, display awake"
        case (false, true): return "unlocked, display asleep"
        case (true, false): return "locked, display awake"
        case (true, true): return "locked, display asleep"
        }
    }

    /// Twice a second: the window changes colour, and a state not tried yet is tried.
    private func tick() {
        view.advance()
        let now = state
        guard !running, !tried.contains(now) else { return }
        running = true
        tried.insert(now)
        _ = capture.take()
        var answer = Answer(state: now)
        // Four seconds of capture: eight colours go by.
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            let taken = self.capture.take()
            answer.frames = taken.frames
            answer.stopped = taken.stopped
            // Alive means the picture followed the window: at least three of its colours, each
            // one of the series, were seen in turn.
            let known = taken.colours.filter { colour in
                ProbeView.colours.contains { abs(Int($0.0) - Int(colour.0)) + abs(Int($0.1) - Int(colour.1)) + abs(Int($0.2) - Int(colour.2)) < 40 }
            }
            answer.live = known.count >= 3
            self.input(into: answer)
        }
    }

    /// Sends a key that types nothing, a movement and a click to the middle of the window,
    /// and sees whether the window got them.
    private func input(into answer: Answer) {
        var answer = answer
        guard mayPost else {
            finish(answer)
            return
        }
        answer.inputTried = true
        let (keys, moves, clicks) = (view.keys, view.moves, view.clicks)
        let frame = window.frame
        let height = NSScreen.screens.first?.frame.height ?? 0
        // Core Graphics counts from the top left of the main screen.
        let middle = CGPoint(x: frame.midX, y: height - frame.midY)
        let post: (CGEvent?) -> Void = { $0?.post(tap: .cgSessionEventTap) }
        post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: middle, mouseButton: .left))
        post(CGEvent(keyboardEventSource: nil, virtualKey: Probe.keyF13, keyDown: true))
        post(CGEvent(keyboardEventSource: nil, virtualKey: Probe.keyF13, keyDown: false))
        post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: middle, mouseButton: .left))
        post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: middle, mouseButton: .left))
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            answer.key = self.view.keys > keys
            answer.move = self.view.moves > moves
            answer.click = self.view.clicks > clicks
            self.finish(answer)
        }
    }

    private func finish(_ answer: Answer) {
        answers.append(answer)
        running = false
        view.status = "tried: " + answers.map { $0.state }.joined(separator: "; ")
        write()
    }

    private func yes(_ value: Bool) -> String { value ? "YES" : "NO" }

    private func write() {
        var lines = [
            "istesso lock check, \(ISO8601DateFormatter().string(from: Date()))",
            "macOS \(ProcessInfo.processInfo.operatingSystemVersionString)",
            "screen recording allowed: \(yes(mayCapture))",
            "sending input allowed (Accessibility): \(yes(mayPost))",
        ]
        if !mayCapture {
            lines.append("NOT RUN: allow this program under System Settings > Privacy & Security > Screen & System Audio Recording, then start it again")
        }
        if !mayPost {
            lines.append("INPUT NOT TRIED: allow this program under System Settings > Privacy & Security > Accessibility, then start it again")
        }
        if let captureProblem { lines.append("capture did not start: \(captureProblem)") }
        if oversize {
            let asked = window.frame.size
            let scale = window.backingScaleFactor
            lines.append(
                "window larger than the screen: asked for \(Int(asked.width * scale))x\(Int(asked.height * scale)) pixels, the window is \(Int(window.frame.width * scale))x\(Int(window.frame.height * scale)), frames come as \(capture.size.0)x\(capture.size.1)")
        }
        for answer in answers {
            lines.append("")
            lines.append("state: \(answer.state)")
            lines.append("  picture captured: \(yes(answer.frames > 0)) (\(answer.frames) frames in 4 s)")
            lines.append("  picture follows the window: \(yes(answer.live))")
            if let stopped = answer.stopped { lines.append("  the capture stopped: \(stopped)") }
            if answer.inputTried {
                lines.append("  key reaches the window: \(yes(answer.key))")
                lines.append("  pointer movement reaches the window: \(yes(answer.move))")
                lines.append("  click reaches the window: \(yes(answer.click))")
            }
        }
        let wanted = ["locked, display awake", "locked, display asleep", "right after wake, locked"]
        let missing = wanted.filter { state in !answers.contains { $0.state == state } }
        if mayCapture, !missing.isEmpty {
            lines.append("")
            lines.append("not tried yet: " + missing.joined(separator: "; "))
        }
        try? FileManager.default.createDirectory(at: report.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? (lines.joined(separator: "\n") + "\n").write(to: report, atomically: true, encoding: .utf8)
    }
}

let application = NSApplication.shared
application.setActivationPolicy(.regular)
let probe = Probe(arguments: CommandLine.arguments)
application.delegate = probe
application.run()
