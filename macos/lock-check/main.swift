// Istesso Lock Check Dev: can a Mac host go on working while its screen is locked, with input
// confined to its own application? A development helper; never part of the product.
//
// Bundle id com.maromoco.istesso-dev.lock-check, visible name "Istesso Lock Check Dev". It needs
// the Screen Recording and Accessibility permissions, granted by the person (ADR 0017). It never
// asks for a permission; without one it says which is missing and does nothing that needs it.
//
// Three checks, each in its own run (docs/m4-plan.md, 5c):
//
//   open "Istesso Lock Check Dev.app" [--args --report <file> --minutes <n> --no-hold]
//     Step 0b. In each state (unlocked, locked, display asleep, right after wake) it captures its
//     own window for 4 s, then sends input ONLY to itself, as listed below, and counts what its
//     own window received. Quits once the three locked states are tried, or after <n> minutes.
//
//   open "Istesso Lock Check Dev.app" --args --rate [capture settings]
//   open "Istesso Lock Check Dev.app" --args --virtual-display 3440x1440@85 [capture settings]
//     How fast frames come, on the real screen or on a virtual display (private CGVirtualDisplay,
//     virtual-display.h). Capture settings, one run per setting:
//       --min-fps <n>   shortest frame interval 1/n s (default 240)
//       --queue <n>     queue depth, 1 to 8 (default 3)
//       --format bgra|420v   pixel format (default bgra)
//       --half          a window half the size of the screen instead of all of it
//
// WHAT IT POSTS, AND TO WHOM (nothing else):
//   1. F13 down and up (a key that types nothing and has no sound) to ITS OWN PROCESS ONLY
//      (CGEventPostToPid with its own pid). Never to the session tap.
//   2. A pointer move and a left click at the middle of ITS OWN WINDOW, to ITS OWN PROCESS ONLY
//      (CGEventPostToPid), the window under the pointer named as its own window.
//   3. The same pointer move and click at the SESSION TAP, only if the rule of
//      crates/istesso-host-macos/src/confine.rs allows it: the screen is NOT locked, this program
//      is frontmost, and the topmost window under the point is its own. Otherwise not posted, and
//      the reason is reported. While locked, nothing ever goes to the session tap.
// No key reaches anything but this process, so nothing can type into or beep at the lock screen;
// its own window swallows F13 and has no responder that beeps.
//
// While a check runs it keeps the display awake (a user-activity declaration, then a
// display-sleep assertion), released when it ends; --no-hold leaves the display alone.

import AppKit
import CoreGraphics
import IOKit.pwr_mgt
import QuartzCore
import ScreenCaptureKit

// MARK: - Keeping the display awake while checking

final class DisplayHold {
    private var assertion: IOPMAssertionID = 0
    private var held = false

    func start() {
        guard !held else { return }
        var activity: IOPMAssertionID = 0
        _ = IOPMAssertionDeclareUserActivity("Istesso Lock Check Dev" as CFString, kIOPMUserActiveLocal, &activity)
        held = IOPMAssertionCreateWithName(
            kIOPMAssertionTypePreventUserIdleDisplaySleep as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
            "Istesso Lock Check Dev: a check is running" as CFString, &assertion) == kIOReturnSuccess
    }

    func stop() {
        if held { IOPMAssertionRelease(assertion) }
        held = false
    }
}

// MARK: - Where input may go: a copy of the rule of crates/istesso-host-macos/src/confine.rs
// (copied from commit 2ab17c2; keep the two the same by hand)

/// Whether the screen is locked, as far as the session's state can be read.
enum LockState: String {
    case unlocked
    case locked
    /// The session's state could not be read at all; refused like locked.
    case unknown = "unknown (the session's state could not be read)"
}

/// Reads the lock state. An absent dictionary is "unknown", not "unlocked"; an absent key is unlocked.
func lockState() -> LockState {
    guard let session = CGSessionCopyCurrentDictionary() as? [String: Any] else { return .unknown }
    return (session["CGSSessionScreenIsLocked"] as? Bool ?? false) ? .locked : .unlocked
}

/// The session tap is allowed only unlocked, frontmost, and with this program's window topmost under
/// the point. Returns nil when allowed, else the reason it is not. Direct delivery to its own process
/// needs no check: it can reach nothing else.
func sessionTapRefusal(lock: LockState, frontmost: Bool, topIsOwn: Bool?) -> String? {
    switch lock {
    case .unlocked: break
    case .locked: return "locked (the session tap is never used while locked)"
    case .unknown: return "the session's state could not be read (refused as if locked)"
    }
    if !frontmost { return "not frontmost" }
    switch topIsOwn {
    case .some(true): return nil
    case .some(false): return "another process's window is on top at the point"
    case .none: return "no window under the point"
    }
}

/// Whether the topmost non-transparent on-screen window containing `point` (top-left origin) is one of
/// this process's. Reads only owner, bounds and alpha, never names.
func topWindowIsOwn(at point: CGPoint) -> Bool? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    let me = Int32(getpid())
    for info in list {
        let alpha = (info[kCGWindowAlpha as String] as? Double) ?? 1
        guard alpha > 0.05,
            let boundsInfo = info[kCGWindowBounds as String] as? NSDictionary,
            let bounds = CGRect(dictionaryRepresentation: boundsInfo),
            bounds.contains(point)
        else { continue }
        return (info[kCGWindowOwnerPID as String] as? Int32) == me
    }
    return nil
}

// MARK: - The window, and what it receives

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

    override func draw(_ dirtyRect: NSRect) {
        let (r, g, b) = ProbeView.colours[step % ProbeView.colours.count]
        NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1).setFill()
        bounds.fill()
        let text = "Istesso lock check (Dev)\n\(status)" as NSString
        text.draw(
            in: NSRect(x: 12, y: 8, width: bounds.width - 24, height: bounds.height / 3),
            withAttributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.white])
    }

    // F13 is counted and swallowed; nothing is passed on, so nothing beeps.
    override func keyDown(with event: NSEvent) {
        if event.keyCode == Probe.keyF13 { keys += 1 }
    }
    override func keyUp(with event: NSEvent) {}
    override func mouseMoved(with event: NSEvent) { moves += 1 }
    override func mouseDown(with event: NSEvent) { clicks += 1 }
    override func mouseUp(with event: NSEvent) {}
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(
            NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self))
    }
}

/// A window that may be larger than the screen and never beeps for an unhandled key.
final class FreeWindow: NSWindow {
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
    override var canBecomeKey: Bool { true }
    override func noResponder(for eventSelector: Selector) {}
}

// MARK: - Capture, with its settings and per-frame timing

struct CaptureSettings {
    var minFps: Int32 = 240
    var queueDepth = 3
    var format: OSType = kCVPixelFormatType_32BGRA
    var formatName = "bgra"
}

final class Capture: NSObject, SCStreamOutput, SCStreamDelegate {
    private let lock = NSLock()
    private var stream: SCStream?
    private var complete = 0
    private var statuses: [String: Int] = [:]
    private var arrivals: [CFTimeInterval] = []
    private var seen: [(UInt8, UInt8, UInt8)] = []
    private(set) var size = (0, 0)
    private(set) var stopped: String?
    private let queue = DispatchQueue(label: "dev.istesso.lock-check.capture")

    func start(windowNumber: Int, pixelWidth: Int, pixelHeight: Int, settings: CaptureSettings, done: @escaping (String?) -> Void) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: false) { content, error in
            guard let window = content?.windows.first(where: { Int($0.windowID) == windowNumber }) else {
                done("the window is not among what may be captured: \(error?.localizedDescription ?? "not listed")")
                return
            }
            self.start(filter: SCContentFilter(desktopIndependentWindow: window), pixelWidth: pixelWidth,
                       pixelHeight: pixelHeight, settings: settings, done: done)
        }
    }

    func start(displayID: CGDirectDisplayID, pixelWidth: Int, pixelHeight: Int, settings: CaptureSettings, done: @escaping (String?) -> Void) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
            guard let display = content?.displays.first(where: { $0.displayID == displayID }) else {
                done("the display is not among what may be captured: \(error?.localizedDescription ?? "not listed")")
                return
            }
            self.start(filter: SCContentFilter(display: display, excludingWindows: []), pixelWidth: pixelWidth,
                       pixelHeight: pixelHeight, settings: settings, done: done)
        }
    }

    private func start(filter: SCContentFilter, pixelWidth: Int, pixelHeight: Int, settings: CaptureSettings, done: @escaping (String?) -> Void) {
        let configuration = SCStreamConfiguration()
        configuration.width = pixelWidth
        configuration.height = pixelHeight
        configuration.showsCursor = false
        configuration.pixelFormat = settings.format
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: settings.minFps)
        configuration.queueDepth = settings.queueDepth
        let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
        do {
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        } catch {
            done("the capture would not take an output: \(error.localizedDescription)")
            return
        }
        lock.lock()
        self.stream = stream
        stopped = nil
        lock.unlock()
        stream.startCapture { error in done(error?.localizedDescription) }
    }

    func stop(done: @escaping () -> Void) {
        lock.lock()
        let stream = self.stream
        self.stream = nil
        lock.unlock()
        guard let stream else { return done() }
        stream.stopCapture { _ in done() }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen else { return }
        let now = CACurrentMediaTime()
        var statusName = "unknown"
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
            as? [[SCStreamFrameInfo: Any]],
            let raw = attachments.first?[.status] as? Int, let status = SCFrameStatus(rawValue: raw)
        {
            switch status {
            case .complete: statusName = "complete"
            case .idle: statusName = "idle"
            case .blank: statusName = "blank"
            case .suspended: statusName = "suspended"
            case .started: statusName = "started"
            case .stopped: statusName = "stopped"
            @unknown default: statusName = "other"
            }
        }
        var colour: (UInt8, UInt8, UInt8)?
        if statusName == "complete", let image = sampleBuffer.imageBuffer,
            CVPixelBufferGetPixelFormatType(image) == kCVPixelFormatType_32BGRA
        {
            CVPixelBufferLockBaseAddress(image, .readOnly)
            if let base = CVPixelBufferGetBaseAddress(image) {
                let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image))
                let pixel = base.advanced(by: (height / 2) * CVPixelBufferGetBytesPerRow(image) + (width / 2) * 4)
                    .assumingMemoryBound(to: UInt8.self)
                colour = (pixel[2], pixel[1], pixel[0])
            }
            CVPixelBufferUnlockBaseAddress(image, .readOnly)
        }
        let dims = sampleBuffer.imageBuffer.map { (CVPixelBufferGetWidth($0), CVPixelBufferGetHeight($0)) }
        lock.lock()
        statuses[statusName, default: 0] += 1
        if statusName == "complete" {
            complete += 1
            arrivals.append(now)
            if let dims { size = dims }
            if let colour, seen.last.map({ $0 != colour }) ?? true { seen.append(colour) }
        }
        lock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        lock.lock()
        stopped = error.localizedDescription
        lock.unlock()
    }

    /// Everything since the last call.
    func take() -> (complete: Int, statuses: [String: Int], gaps: [Double], colours: [(UInt8, UInt8, UInt8)], stopped: String?) {
        lock.lock()
        defer {
            complete = 0
            statuses = [:]
            arrivals = []
            seen = []
            lock.unlock()
        }
        let gaps = zip(arrivals.dropFirst(), arrivals).map { ($0 - $1) * 1000 }
        return (complete, statuses, gaps, seen, stopped)
    }
}

func percentile(_ values: [Double], _ q: Double) -> Double {
    guard !values.isEmpty else { return 0 }
    let sorted = values.sorted()
    let position = q * Double(sorted.count - 1)
    let below = Int(position.rounded(.down))
    let above = Int(position.rounded(.up))
    return sorted[below] + (sorted[above] - sorted[below]) * (position - Double(below))
}

// MARK: - Step 0b: the states, and confined input

struct Answer {
    var state: String
    var frames = 0
    var live = false
    var stopped: String?
    var frontmost = false
    var lock = LockState.unknown
    var direct = (key: false, move: false, click: false)
    var tap: (posted: Bool, refusal: String?, move: Bool, click: Bool) = (false, nil, false, false)
    var sinceKeyBefore = 0.0
    var sinceKeyAfter = 0.0
    var inputTried = false
}

final class Probe: NSObject, NSApplicationDelegate {
    static let keyF13: UInt16 = 0x69
    static let wanted = ["locked, display awake", "locked, display asleep", "right after wake, locked"]

    private var window: NSWindow!
    private var view: ProbeView!
    private let capture = Capture()
    private let hold = DisplayHold()
    private var answers: [Answer] = []
    private var tried: Set<String> = []
    private var running = false
    private var wokeAt: Date?
    private var captureProblem: String?
    private let report: URL
    private let minutes: Double
    private let holdDisplay: Bool
    private var quitting = false
    private var quitReason: String?
    private var mayCapture = false
    private var mayPost = false

    init(arguments: [String]) {
        minutes = argument(arguments, "--minutes").flatMap(Double.init).flatMap { $0 > 0 ? $0 : nil } ?? 60
        holdDisplay = !arguments.contains("--no-hold")
        report = reportURL(arguments, default: "lock-check-report.txt")
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let screen = NSScreen.main?.frame ?? NSRect(x: 0, y: 0, width: 1280, height: 720)
        let frame = NSRect(x: screen.midX - 320, y: screen.midY - 200, width: 640, height: 400)
        window = FreeWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Istesso lock check (Dev)"
        view = ProbeView(frame: NSRect(origin: .zero, size: frame.size))
        window.contentView = view
        window.acceptsMouseMovedEvents = true
        window.setFrame(frame, display: true)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)

        // Only read whether the permissions are there; never ask.
        mayCapture = CGPreflightScreenCaptureAccess()
        mayPost = CGPreflightPostEventAccess()
        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(woke), name: NSWorkspace.didWakeNotification, object: nil)
        write()
        guard mayCapture else {
            view.status = "screen recording is not allowed for this program (see the report)"
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) { NSApp.terminate(nil) }
            return
        }
        if holdDisplay { hold.start() }
        Timer.scheduledTimer(withTimeInterval: minutes * 60, repeats: false) { _ in self.quit("time is up") }
        let scale = window.backingScaleFactor
        capture.start(windowNumber: window.windowNumber, pixelWidth: Int(frame.width * scale),
                      pixelHeight: Int(frame.height * scale), settings: CaptureSettings(minFps: 30)) { problem in
            DispatchQueue.main.async {
                self.captureProblem = problem
                self.view.status = problem ?? "capturing; try each state"
                self.write()
                Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { _ in self.tick() }
            }
        }
    }

    @objc private func woke() { wokeAt = Date() }

    /// For naming the state only; the rule reads `lockState()` itself, where unknown is refused.
    private var locked: Bool { lockState() != .unlocked }

    private var frontmost: Bool {
        NSWorkspace.shared.frontmostApplication?.processIdentifier == getpid()
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

    private func tick() {
        view.advance()
        let now = state
        guard !running, !tried.contains(now) else { return }
        running = true
        tried.insert(now)
        _ = capture.take()
        var answer = Answer(state: now)
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            let taken = self.capture.take()
            answer.frames = taken.complete
            answer.stopped = taken.stopped
            let known = taken.colours.filter { colour in
                ProbeView.colours.contains {
                    abs(Int($0.0) - Int(colour.0)) + abs(Int($0.1) - Int(colour.1)) + abs(Int($0.2) - Int(colour.2)) < 40
                }
            }
            answer.live = known.count >= 3
            self.input(into: answer)
        }
    }

    private var middle: CGPoint {
        // Core Graphics counts from the top left of the main screen.
        let frame = window.frame
        let height = NSScreen.screens.first?.frame.height ?? 0
        return CGPoint(x: frame.midX, y: height - frame.midY)
    }

    /// A pointer event aimed at this program's own window.
    private func pointer(_ type: CGEventType) -> CGEvent? {
        let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: middle, mouseButton: .left)
        // Name the window under the pointer as this program's own, so its own process routes it there.
        event?.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.windowNumber))
        event?.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.windowNumber))
        return event
    }

    private func input(into answer: Answer) {
        var answer = answer
        guard mayPost else { return finish(answer) }
        answer.inputTried = true
        answer.frontmost = frontmost
        answer.lock = lockState()
        let me = getpid()
        answer.sinceKeyBefore = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: .keyDown)

        // 1 and 2: straight to this program's own process. Nothing else can receive these.
        let (keys0, moves0, clicks0) = (view.keys, view.moves, view.clicks)
        CGEvent(keyboardEventSource: nil, virtualKey: Probe.keyF13, keyDown: true)?.postToPid(me)
        CGEvent(keyboardEventSource: nil, virtualKey: Probe.keyF13, keyDown: false)?.postToPid(me)
        pointer(.mouseMoved)?.postToPid(me)
        pointer(.leftMouseDown)?.postToPid(me)
        pointer(.leftMouseUp)?.postToPid(me)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.7) {
            answer.direct = (self.view.keys > keys0, self.view.moves > moves0, self.view.clicks > clicks0)
            answer.sinceKeyAfter = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: .keyDown)

            // 3: the session tap, only if the rule allows it. Never while locked.
            let refusal = sessionTapRefusal(lock: lockState(), frontmost: self.frontmost,
                                            topIsOwn: topWindowIsOwn(at: self.middle))
            answer.tap.refusal = refusal
            guard refusal == nil else { return self.finish(answer) }
            let (moves1, clicks1) = (self.view.moves, self.view.clicks)
            answer.tap.posted = true
            self.pointer(.mouseMoved)?.post(tap: .cgSessionEventTap)
            self.pointer(.leftMouseDown)?.post(tap: .cgSessionEventTap)
            self.pointer(.leftMouseUp)?.post(tap: .cgSessionEventTap)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.7) {
                answer.tap.move = self.view.moves > moves1
                answer.tap.click = self.view.clicks > clicks1
                self.finish(answer)
            }
        }
    }

    private func finish(_ answer: Answer) {
        answers.append(answer)
        running = false
        view.status = "tried: " + answers.map { $0.state }.joined(separator: "; ")
        write()
        if Probe.wanted.allSatisfy({ state in answers.contains { $0.state == state } }) {
            quit("all three locked states tried")
        }
    }

    private func quit(_ why: String) {
        guard !quitting else { return }
        quitting = true
        quitReason = why
        capture.stop {
            DispatchQueue.main.async {
                self.hold.stop()
                self.write()
                NSApp.terminate(nil)
            }
        }
    }

    private func yes(_ value: Bool) -> String { value ? "YES" : "NO" }

    private func write() {
        var lines = [
            "istesso lock check (Dev), \(ISO8601DateFormatter().string(from: Date()))",
            "macOS \(ProcessInfo.processInfo.operatingSystemVersionString)",
            "screen recording allowed: \(yes(mayCapture))",
            "sending input allowed (Accessibility): \(yes(mayPost))",
            "display held awake while checking: \(yes(holdDisplay))",
        ]
        if !mayCapture {
            lines.append("NOT RUN: allow this program under System Settings > Privacy & Security > Screen & System Audio Recording, then start it again")
        }
        if !mayPost {
            lines.append("INPUT NOT TRIED: allow this program under System Settings > Privacy & Security > Accessibility, then start it again")
        }
        if let captureProblem { lines.append("capture did not start: \(captureProblem)") }
        for answer in answers {
            lines.append("")
            lines.append("state: \(answer.state)")
            lines.append("  picture captured: \(yes(answer.frames > 0)) (\(answer.frames) frames in 4 s)")
            lines.append("  picture follows the window: \(yes(answer.live))")
            if let stopped = answer.stopped { lines.append("  the capture stopped: \(stopped)") }
            guard answer.inputTried else { continue }
            lines.append("  lock state read: \(answer.lock.rawValue)")
            lines.append("  this program frontmost: \(yes(answer.frontmost))")
            lines.append("  direct to own process: key \(yes(answer.direct.key)), move \(yes(answer.direct.move)), click \(yes(answer.direct.click))")
            lines.append(String(format: "  session's last key event, seconds ago: %.2f before, %.2f after the direct key (a fall to near 0 would mean it entered the session stream)",
                                answer.sinceKeyBefore, answer.sinceKeyAfter))
            if let refusal = answer.tap.refusal {
                lines.append("  session tap: not posted, \(refusal)")
            } else {
                lines.append("  session tap (allowed by the rule): move \(yes(answer.tap.move)), click \(yes(answer.tap.click))")
            }
        }
        let missing = Probe.wanted.filter { state in !answers.contains { $0.state == state } }
        if mayCapture, !missing.isEmpty {
            lines.append("")
            lines.append("not tried yet: " + missing.joined(separator: "; "))
        }
        if let quitReason {
            lines.append("")
            lines.append("stopped capturing and quit: \(quitReason)")
        }
        writeReport(lines, to: report)
    }
}

func argument(_ arguments: [String], _ name: String) -> String? {
    guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
    return arguments[index + 1]
}

func reportURL(_ arguments: [String], default name: String) -> URL {
    if let path = argument(arguments, "--report") { return URL(fileURLWithPath: path) }
    return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".cache/istesso-m4-wip/\(name)")
}

func writeReport(_ lines: [String], to report: URL) {
    try? FileManager.default.createDirectory(at: report.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? (lines.joined(separator: "\n") + "\n").write(to: report, atomically: true, encoding: .utf8)
}

// MARK: - How fast frames come

final class RateView: NSView {
    private(set) var draws = 0
    private var step = 0
    private var link: CADisplayLink?

    func run() {
        link?.invalidate()
        let link = displayLink(target: self, selector: #selector(frame(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 1, maximum: 240, preferred: 240)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func frame(_ link: CADisplayLink) {
        step += 1
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        draws += 1
        let (r, g, b) = ProbeView.colours[step % ProbeView.colours.count]
        NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1).setFill()
        bounds.fill()
    }
}

final class RateCheck: NSObject, NSApplicationDelegate {
    private let report: URL
    private let virtualSpec: (width: Int, height: Int, hertz: Double)?
    private var settings = CaptureSettings()
    private let half: Bool
    private let holdDisplay: Bool
    private var lines: [String] = []
    private var window: NSWindow!
    private var view: RateView!
    private let capture = Capture()
    private let hold = DisplayHold()
    private var virtualDisplay: CGVirtualDisplay?
    private var displayID = CGMainDisplayID()

    init(arguments: [String]) {
        if let spec = argument(arguments, "--virtual-display") {
            let parts = spec.split(whereSeparator: { $0 == "x" || $0 == "@" }).map(String.init)
            if parts.count == 3, let w = Int(parts[0]), let h = Int(parts[1]), let hz = Double(parts[2]) {
                virtualSpec = (w, h, hz)
            } else {
                virtualSpec = (3440, 1440, 85)
            }
            report = reportURL(arguments, default: "virtual-display-report.txt")
        } else {
            virtualSpec = nil
            report = reportURL(arguments, default: "rate-check-report.txt")
        }
        if let n = argument(arguments, "--min-fps").flatMap(Int32.init), n > 0 { settings.minFps = n }
        if let n = argument(arguments, "--queue").flatMap(Int.init) { settings.queueDepth = min(max(n, 1), 8) }
        if argument(arguments, "--format") == "420v" {
            settings.format = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            settings.formatName = "420v"
        }
        half = arguments.contains("--half")
        holdDisplay = !arguments.contains("--no-hold")
    }

    private func note(_ line: String) {
        lines.append(line)
        writeReport(lines, to: report)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        note("istesso rate check (Dev), \(ISO8601DateFormatter().string(from: Date()))")
        note("macOS \(ProcessInfo.processInfo.operatingSystemVersionString)")
        note("settings: min frame interval 1/\(settings.minFps) s, queue depth \(settings.queueDepth), format \(settings.formatName), window \(half ? "half the screen" : "640x400 or the whole virtual display"), display held awake \(holdDisplay ? "YES" : "NO")")
        Timer.scheduledTimer(withTimeInterval: 90, repeats: false) { _ in self.end("time is up") }
        guard CGPreflightScreenCaptureAccess() else {
            note("NOT RUN: allow this program under System Settings > Privacy & Security > Screen & System Audio Recording, then start it again")
            NSApp.terminate(nil)
            return
        }
        if holdDisplay { hold.start() }
        note("displays before: \(activeDisplays().map { describe($0) }.joined(separator: "; "))")
        if let spec = virtualSpec {
            makeVirtualDisplay(spec)
        } else {
            openWindow(on: NSScreen.main, fill: false)
        }
    }

    private func describe(_ id: CGDirectDisplayID) -> String {
        guard let mode = CGDisplayCopyDisplayMode(id) else { return "no mode" }
        return "\(mode.width)x\(mode.height) points, \(mode.pixelWidth)x\(mode.pixelHeight) pixels, \(mode.refreshRate) Hz"
    }

    private func activeDisplays() -> [CGDirectDisplayID] {
        var count: UInt32 = 0
        CGGetActiveDisplayList(0, nil, &count)
        var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
        CGGetActiveDisplayList(count, &ids, &count)
        return Array(ids.prefix(Int(count)))
    }

    private func makeVirtualDisplay(_ spec: (width: Int, height: Int, hertz: Double)) {
        let asked = Date()
        let descriptor = CGVirtualDisplayDescriptor()
        descriptor.name = "Istesso trial (Dev)"
        descriptor.maxPixelsWide = UInt32(spec.width)
        descriptor.maxPixelsHigh = UInt32(spec.height)
        descriptor.sizeInMillimeters = CGSize(width: Double(spec.width) / 110 * 25.4, height: Double(spec.height) / 110 * 25.4)
        descriptor.vendorID = 0x1234
        descriptor.productID = 0x0001
        descriptor.serialNum = 0x0001
        descriptor.redPrimary = CGPoint(x: 0.64, y: 0.33)
        descriptor.greenPrimary = CGPoint(x: 0.30, y: 0.60)
        descriptor.bluePrimary = CGPoint(x: 0.15, y: 0.06)
        descriptor.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
        descriptor.queue = DispatchQueue.main
        guard let display = CGVirtualDisplay(descriptor: descriptor) else {
            return end("the virtual display was not made")
        }
        let modes = CGVirtualDisplaySettings()
        modes.hiDPI = 0
        modes.modes = [CGVirtualDisplayMode(width: UInt32(spec.width), height: UInt32(spec.height), refreshRate: spec.hertz)]
        let applied = display.apply(modes)
        virtualDisplay = display
        displayID = display.displayID
        note("asked for \(spec.width)x\(spec.height) at \(spec.hertz) Hz; settings applied: \(applied ? "YES" : "NO")")
        Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { timer in
            let screen = NSScreen.screens.first {
                ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == self.displayID
            }
            if let screen {
                timer.invalidate()
                self.note(String(format: "it appeared after %.2f s: %@", Date().timeIntervalSince(asked), self.describe(self.displayID)))
                self.note("displays with it: \(self.activeDisplays().map { self.describe($0) }.joined(separator: "; "))")
                self.openWindow(on: screen, fill: !self.half)
            } else if Date().timeIntervalSince(asked) > 10 {
                timer.invalidate()
                self.end("the virtual display did not appear among the screens within 10 s")
            }
        }
    }

    private func openWindow(on screen: NSScreen?, fill: Bool) {
        let area = screen?.frame ?? NSRect(x: 0, y: 0, width: 1280, height: 720)
        let frame: NSRect
        if fill {
            frame = area
        } else if half {
            frame = NSRect(x: area.minX, y: area.minY, width: area.width / 2, height: area.height / 2)
        } else {
            frame = NSRect(x: area.midX - 320, y: area.midY - 200, width: 640, height: 400)
        }
        window = FreeWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        view = RateView(frame: NSRect(origin: .zero, size: frame.size))
        window.contentView = view
        window.setFrame(frame, display: true)
        window.orderFrontRegardless()
        view.run()
        note("screen: \(describe(displayID)); window \(Int(frame.width))x\(Int(frame.height)) points, scale \(window.backingScaleFactor)")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.measureWindow() }
    }

    private func count(_ what: String, then next: @escaping () -> Void) -> (String?) -> Void {
        return { problem in
            DispatchQueue.main.async {
                if let problem {
                    self.note("\(what): capture did not start: \(problem)")
                    return self.capture.stop { DispatchQueue.main.async { next() } }
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                    _ = self.capture.take()
                    let drawsBefore = self.view.draws
                    DispatchQueue.main.asyncAfter(deadline: .now() + 8) {
                        let taken = self.capture.take()
                        let draws = Double(self.view.draws - drawsBefore) / 8
                        let perSecond = taken.statuses.sorted { $0.key < $1.key }
                            .map { String(format: "%@ %.1f", $0.key, Double($0.value) / 8) }.joined(separator: ", ")
                        self.note(String(
                            format: "%@: complete %.1f a second (statuses a second: %@), the window drew %.1f a second, frames %dx%d; arrival gaps median %.2f ms, 95%% %.2f ms, max %.2f ms%@",
                            what, Double(taken.complete) / 8, perSecond, draws, self.capture.size.0, self.capture.size.1,
                            percentile(taken.gaps, 0.5), percentile(taken.gaps, 0.95), taken.gaps.max() ?? 0,
                            taken.stopped.map { "; the capture stopped: \($0)" } ?? ""))
                        self.capture.stop { DispatchQueue.main.async { next() } }
                    }
                }
            }
        }
    }

    private func measureWindow() {
        let scale = window.backingScaleFactor
        capture.start(windowNumber: window.windowNumber, pixelWidth: Int(window.frame.width * scale),
                      pixelHeight: Int(window.frame.height * scale), settings: settings,
                      done: count("window alone (form A)") { self.measureDisplay() })
    }

    private func measureDisplay() {
        let mode = CGDisplayCopyDisplayMode(displayID)
        capture.start(displayID: displayID, pixelWidth: mode?.pixelWidth ?? CGDisplayPixelsWide(displayID),
                      pixelHeight: mode?.pixelHeight ?? CGDisplayPixelsHigh(displayID), settings: settings,
                      done: count("whole display (form C)") { self.end("done") })
    }

    private var ended = false

    private func end(_ why: String) {
        guard !ended else { return }
        ended = true
        capture.stop {
            DispatchQueue.main.async {
                self.window?.orderOut(nil)
                let finish = {
                    self.hold.stop()
                    self.note("stopped capturing and quit: \(why)")
                    NSApp.terminate(nil)
                }
                guard self.virtualDisplay != nil else { return finish() }
                let id = self.displayID
                self.virtualDisplay = nil
                DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                    self.note("virtual display removed: \(self.activeDisplays().contains(id) ? "NO" : "YES")")
                    self.note("displays after: \(self.activeDisplays().map { self.describe($0) }.joined(separator: "; "))")
                    finish()
                }
            }
        }
    }
}

let application = NSApplication.shared
application.setActivationPolicy(.regular)
let arguments = CommandLine.arguments
let delegate: NSApplicationDelegate =
    arguments.contains("--rate") || arguments.contains("--virtual-display")
    ? RateCheck(arguments: arguments) : Probe(arguments: arguments)
application.delegate = delegate
application.run()
