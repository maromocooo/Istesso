// The application's other windows, each as a stream of its own (ADR 0027, a proposal). PROTOTYPE,
// trial only, and OFF unless the host is started with --windows: without it nothing here runs, the
// application's windows are captured as before (the first window's place, with whatever of the
// application lies over it), and no list is ever sent.
//
// With --windows, and only after the viewer said that it shows windows one by one (the input
// event {"kind":"windows","version":1}, which reaches this program only through an agent that
// passes window numbers on):
//   - the application's other windows are noticed: by the window list every 200 ms (lookAtApp) and
//     at once when Accessibility says a window was made. Counted are windows of its process on
//     the screen, at layer 0, larger than 40x40 points, that Accessibility calls a window (a sheet
//     stays in its parent's picture), and never one the application had when its first window
//     was first shown or that came within 1.5 s after (another document of the owner's, a
//     window restored at launch: WindowBaseline);
//   - each gets a stream of its own (at most three besides the first: four to an application,
//     ADR 0027 2.2): a listener on a port of its own (any free one), an encoder of its own, and a
//     capture of the display it is on, of the application's windows except the first window and
//     the other streamed ones, read from its place. The first window's capture leaves the
//     streamed windows out the same way; its encoder and connection are not touched;
//   - the list of them goes to the viewer as packet kind 6 on the first window's connection
//     (number, port, size, title, whether it is a dialog, whether the host's keyboard is in it). A
//     window whose stream nobody watches within one second goes back into the first window's
//     picture, as today, and is not streamed again (ADR 0027, 3, step 5); so does a fifth one;
//   - pointer positions with a window number are in that window's picture; a focus with a number
//     makes that window the one keys go to (it is raised, as the first one is); a close asks the
//     application to close that window only (its close button, by Accessibility: a question in a
//     sheet stays and is shown in that window's picture);
//   - everything here ends, and the list is emptied, when the picture stops (pause, detach, the
//     lock, the measuring signal, a new size of the first window) or the viewer has been away
//     for three seconds. A viewer that comes back says it again.
// Titles are only put into the list: never written to a log or to standard output.
//
// With --generated --windows --generated-windows N (1-3): N still windows of the stand-in, for
// checks without any permission; their close removes them as an application would.

import AppKit
import ApplicationServices
import ScreenCaptureKit

// MARK: - Which windows, and which get a stream (pure; --self-test)

/// Another window of the application that may get a stream of its own.
struct ExtraCandidate: Equatable {
    let id: CGWindowID
    let bounds: CGRect
    let title: String
    let modal: Bool
}

func samePlace(_ a: CGRect, _ b: CGRect) -> Bool {
    abs(a.minX - b.minX) < 2 && abs(a.minY - b.minY) < 2 && abs(a.width - b.width) < 2 && abs(a.height - b.height) < 2
}

/// The windows that are never another window to stream: every window the application has when its
/// first window is first shown, and every one that comes within the next 1.5 s (windows restored
/// at launch, an application's own windows of its start). Recorded with --windows only, in every
/// form (a document, a new window, the application as it is). Until it is recorded, nothing is
/// another window.
struct WindowBaseline {
    private(set) var known: Set<CGWindowID>?
    private var until: TimeInterval = 0
    static let grace: TimeInterval = 1.5
    /// A look at the application: `all` its windows now, `firstShown` whether its first window is
    /// chosen, `now` uptime.
    mutating func look(all: [CGWindowID], firstShown: Bool, now: TimeInterval) {
        guard firstShown else { return }
        if known == nil {
            known = Set(all)
            until = now + Self.grace
        } else if now < until {
            known?.formUnion(all)
        }
    }
}

/// The application's other windows, front to back: see the top of this file. `all` is the
/// application's windows in the window list, `first` the window that is shown, `before` the
/// windows that are never another window (WindowBaseline; nil: not recorded yet, so none is),
/// `ax` what Accessibility listed last.
func extraCandidates(all: [WindowInfo], first: CGWindowID?, before: Set<CGWindowID>?, ax: [AXWindowInfo]) -> [ExtraCandidate] {
    guard let first, let before else { return [] }
    let dialogs = [kAXDialogSubrole, kAXSystemDialogSubrole, kAXFloatingWindowSubrole].map { $0 as String }
    return all.compactMap { w in
        guard w.id != first, w.layer == 0, w.onScreen, w.alpha > 0.05, w.bounds.width > 40, w.bounds.height > 40,
              !before.contains(w.id),
              let match = ax.first(where: { samePlace($0.bounds, w.bounds) }), match.role == kAXWindowRole as String else { return nil }
        return ExtraCandidate(id: w.id, bounds: w.bounds, title: match.title, modal: dialogs.contains(match.subrole))
    }
}

/// Small numbers for the windows, 2 and up, in the order they were first seen; a number is never
/// given to another window while the host runs (a late request for a closed window reaches nothing).
final class WindowNumbers {
    private var next: UInt32 = 2
    private var given: [CGWindowID: UInt32] = [:]
    func number(for id: CGWindowID) -> UInt32 {
        if let n = given[id] { return n }
        let n = next
        next += 1
        given[id] = n
        return n
    }
    func forget(_ id: CGWindowID) { given[id] = nil }
}

/// What changes: streams to start (new windows, in order, while fewer than `most` have one), to
/// stop (their window is gone), and windows that stay in the first window's picture because
/// `most` streams run already.
func streamPlan(candidates: [CGWindowID], streaming: Set<CGWindowID>, fallback: Set<CGWindowID>, most: Int)
    -> (start: [CGWindowID], stop: [CGWindowID], over: [CGWindowID]) {
    let present = Set(candidates)
    let stop = streaming.filter { !present.contains($0) }.sorted()
    var running = streaming.count - stop.count
    var start: [CGWindowID] = [], over: [CGWindowID] = []
    for id in candidates where !streaming.contains(id) && !fallback.contains(id) {
        if running < most {
            start.append(id)
            running += 1
        } else {
            over.append(id)
        }
    }
    return (start, stop, over)
}

/// One entry of the list the viewer is sent.
struct ListedWindow: Equatable {
    let number: UInt32
    let port: Int
    let width: Int
    let height: Int
    let title: String
    let modal: Bool
    let key: Bool
}

/// The list as packet kind 6 carries it (JSON, keys in a fixed order); titles cut to 256 bytes
/// at a character's end.
func windowListJSON(serial: UInt32, _ windows: [ListedWindow]) -> String {
    let entries: [[String: Any]] = windows.map { w in
        var title = w.title
        while title.utf8.count > 256 { title.removeLast() }
        return ["id": w.number, "port": w.port, "width": w.width, "height": w.height, "title": title,
                "modal": w.modal, "key": w.key, "attention": false]
    }
    let root: [String: Any] = ["serial": serial, "windows": entries]
    guard let data = try? JSONSerialization.data(withJSONObject: root, options: [.sortedKeys]),
          let text = String(data: data, encoding: .utf8) else { return "{\"serial\":\(serial),\"windows\":[]}" }
    return text
}

/// Where a pointer position in a picture goes: a place on the screen and the window it is in (1:
/// the first), or nowhere (a window number that is not listed).
enum PointerTarget: Equatable {
    case at(CGPoint, window: UInt32)
    case nowhere(UInt32)
}

/// `window` nil, 0 or 1: the first window's picture (`first`), as without --windows; 2 and up:
/// the listed window's (`place`), or nowhere when it is not listed.
func pointerTarget(x: Double, y: Double, window: UInt32?, first: (origin: CGPoint, scale: Double),
                   place: (UInt32) -> (origin: CGPoint, scale: Double)?) -> PointerTarget {
    guard let window, window > 1 else { return .at(CGPoint(x: first.origin.x + x * first.scale, y: first.origin.y + y * first.scale), window: 1) }
    guard let p = place(window) else { return .nowhere(window) }
    return .at(CGPoint(x: p.origin.x + x * p.scale, y: p.origin.y + y * p.scale), window: window)
}

/// While the pointer is nowhere, a button event goes only when it releases a button that is down.
func passesNowhere(pressed: Bool, held: Bool) -> Bool { !pressed && held }

/// A window's picture size: its points at the density of its display, even, 64 to 3840 x 2160.
func extraPictureSize(_ bounds: CGRect, density: Double) -> (Int, Int) {
    (max(64, min(3840, Int(bounds.width * density)) & ~1), max(64, min(2160, Int(bounds.height * density)) & ~1))
}

// MARK: - One other window's stream

final class ExtraStream {
    let number: UInt32
    let windowId: CGWindowID
    private(set) var bounds: CGRect
    var title: String
    var modal: Bool
    let width: Int, height: Int
    let transport: Transport
    let pump: Pump
    private var capture: Capture?
    private var generated: Generated?
    private var timers: [DispatchSourceTimer] = []
    private(set) var started = false
    /// When it was listed (uptime), and whether a viewer has connected since.
    let listedAt = ProcessInfo.processInfo.systemUptime
    var watched = false

    init?(number: UInt32, candidate: ExtraCandidate, density: Double, admitting: AdmitState) {
        self.number = number
        windowId = candidate.id
        bounds = candidate.bounds
        title = candidate.title
        modal = candidate.modal
        let size = extraPictureSize(candidate.bounds, density: density)
        width = size.0
        height = size.1
        guard let made = Transport(settings: settings, width: width, height: height, admitting: admitting, port: 0) else { return nil }
        transport = made
        made.ownCursor()
        pump = Pump(transport: made, width: width, height: height)
        pump.window = Int(number)
    }

    /// Points of the screen to one pixel of its picture, and where the picture's top left is.
    var place: (origin: CGPoint, scale: Double) { (bounds.origin, bounds.width / Double(width)) }

    /// Opens the encoder, then the capture (`filter` and `sourceRect`), or a still picture of the
    /// stand-in when there is no filter. `done` on the main queue, with why it did not start.
    func start(filter: SCContentFilter?, sourceRect: CGRect?, done: @escaping (String?) -> Void) {
        started = true
        pump.queue.async {
            if let why = self.pump.open() { return DispatchQueue.main.async { done("no encoder: \(why)") } }
            DispatchQueue.main.async {
                self.startTimers()
                guard let filter else {
                    let source = Generated(width: self.width, height: self.height)
                    self.generated = source
                    source.start(on: self.pump.queue, fps: settings.fps, still: true) { [weak self] buffer, at in self?.pump.picture(buffer, displayNs: at) }
                    return done(nil)
                }
                let capture = Capture(queue: self.pump.queue) { [weak self] buffer, at in self?.pump.picture(buffer, displayNs: at) }
                capture.onStop = { why in note("the capture of another window stopped: \(why)") }
                self.capture = capture
                capture.start(filter: filter, configuration: Capture.configuration(width: self.width, height: self.height, sourceRect: sourceRect)) { why in
                    DispatchQueue.main.async { done(why.map { "the capture did not start: \($0)" }) }
                }
            }
        }
    }

    /// The window moved (same size), or the windows left out changed.
    func update(bounds: CGRect, filter: SCContentFilter?, sourceRect: CGRect?) {
        self.bounds = bounds
        if let filter { capture?.update(filter) }
        if let sourceRect { capture?.update(Capture.configuration(width: width, height: height, sourceRect: sourceRect)) }
    }

    func moved(to bounds: CGRect) { self.bounds = bounds }

    private func startTimers() {
        let tick = DispatchSource.makeTimerSource(flags: .strict, queue: pump.queue)
        tick.schedule(deadline: .now(), repeating: .milliseconds(4), leeway: .microseconds(500))
        tick.setEventHandler { [weak self] in self?.pump.tick() }
        tick.resume()
        let second = DispatchSource.makeTimerSource(queue: pump.queue)
        second.schedule(deadline: .now() + 1, repeating: .seconds(1))
        second.setEventHandler { [weak self] in self?.pump.second() }
        second.resume()
        timers = [tick, second]
    }

    /// Stops capturing, gives the encoder back and stops listening (the viewer's connection ends).
    func stop() {
        for timer in timers { timer.cancel() }
        timers = []
        generated?.stop()
        generated = nil
        let capture = self.capture
        self.capture = nil
        let finish = { self.pump.queue.async { self.pump.close(); self.transport.close() } }
        if let capture { capture.stop(done: finish) } else { finish() }
    }
}

/// What the host keeps about the other windows (one per host; on the main thread).
final class ExtraWindows {
    /// The viewer said it shows windows one by one, and has not gone away since.
    var declared = false
    var streams: [UInt32: ExtraStream] = [:]
    /// Windows that stay in the first window's picture (no viewer came, or too many).
    var fallback: Set<CGWindowID> = []
    let numbers = WindowNumbers()
    var serial: UInt32 = 0
    /// The list told last, without its serial.
    var told: String?
    /// The window keys go to, when it is not the first (a focus with its number).
    var focus: UInt32?
    var viewerAwaySince: TimeInterval?
    /// The windows that are never another window (--windows; recorded by lookAtApp).
    var baseline = WindowBaseline()
    /// The stand-in's windows (--generated), there from `pretendFrom` (uptime) on: three seconds
    /// after the viewer said it, so that they open while the first window's stream runs steadily.
    var pretend: [ExtraCandidate] = []
    var pretendFrom: TimeInterval = 0
    /// What the first window's capture leaves out (filled in by refreshFilters).
    var leftOut: [SCWindow] = []
    var refreshing = false
    /// Something changed while the filters were being made: made again once they are.
    var refreshAgain = false
    func stream(_ id: CGWindowID) -> ExtraStream? { streams.values.first { $0.windowId == id } }
}

// MARK: - The host's part

extension DailyHost {
    /// The window keys go to: the one the viewer last focused, else the first.
    var keyWindowId: CGWindowID? { extras.focus.flatMap { extras.streams[$0]?.windowId } ?? windowId }
    var keyWindowBounds: CGRect { extras.focus.flatMap { extras.streams[$0]?.bounds } ?? windowBounds }

    /// {"kind":"windows"}: the viewer shows windows one by one.
    func windowsDeclared() {
        guard settings.windows, !extras.declared else { return }
        extras.declared = true
        extras.viewerAwaySince = nil
        say("windows", [("declared", true)])
        if settings.generated && extras.pretend.isEmpty {
            extras.pretend = (0..<settings.generatedWindows).map { k in
                ExtraCandidate(id: CGWindowID(900_001 + k), bounds: CGRect(x: 200 + 40 * k, y: 200 + 40 * k, width: 640, height: 400),
                               title: "Stand-in window \(k + 2)", modal: false)
            }
            extras.pretendFrom = ProcessInfo.processInfo.systemUptime + 3
        }
        lookAtWindows()
    }

    /// Looked at with every look at the application (and at once when a window was made).
    func lookAtWindows() {
        guard settings.windows, extras.declared, transport != nil, !restarting else { return }
        let now = ProcessInfo.processInfo.systemUptime
        // The viewer away for three seconds: back to the form of today; it says it again.
        if transport.viewers == 0 {
            if let since = extras.viewerAwaySince {
                if now - since > 3 { return windowsReset("the viewer has gone") }
            } else {
                extras.viewerAwaySince = now
            }
        } else {
            extras.viewerAwaySince = nil
        }
        var candidates: [ExtraCandidate]
        if settings.generated {
            candidates = now >= extras.pretendFrom ? extras.pretend : []
        } else {
            guard let pid = app?.processIdentifier, windowId != nil, !screenLocked() else { return }
            lookout.refresh(pid)
            let all = windowList(onScreenOnly: false).filter { $0.pid == pid }
            let before = extras.baseline.known.map { $0.union(windowsAtOpen ?? []) }
            candidates = extraCandidates(all: all, first: windowId, before: before, ax: lookout.windows)
        }
        candidates.sort { extras.numbers.number(for: $0.id) < extras.numbers.number(for: $1.id) }
        let streaming = Set(extras.streams.values.map(\.windowId))
        // A window of another size is a new stream (the viewer opens it again; no resize here).
        var resized: Set<CGWindowID> = []
        for c in candidates {
            guard let s = extras.stream(c.id) else { continue }
            let density = settings.generated ? 2 : densityAt(c.bounds)
            if extraPictureSize(c.bounds, density: density) != (s.width, s.height) { resized.insert(c.id) }
            s.title = c.title
        }
        let plan = streamPlan(candidates: candidates.map(\.id), streaming: streaming.subtracting(resized), fallback: extras.fallback, most: 3)
        var changed = false
        for id in plan.stop + Array(resized) {
            guard let s = extras.stream(id) else { continue }
            s.stop()
            extras.streams[s.number] = nil
            if extras.focus == s.number { extras.focus = nil }
            logWindow(s.number, "gone")
            if !resized.contains(id) { extras.numbers.forget(id) }
            changed = true
        }
        for id in plan.over where !extras.fallback.contains(id) {
            extras.fallback.insert(id)
            say("window-fallback", [("window", Int(extras.numbers.number(for: id))), ("reason", "four streams to an application at most")])
        }
        for id in plan.start {
            guard let c = candidates.first(where: { $0.id == id }) else { continue }
            let number = extras.numbers.number(for: id)
            let density = settings.generated ? 2 : densityAt(c.bounds)
            guard let s = ExtraStream(number: number, candidate: c, density: density, admitting: admitState) else {
                extras.fallback.insert(id)
                say("window-fallback", [("window", Int(number)), ("reason", "no listener could be opened")])
                continue
            }
            extras.streams[number] = s
            logWindow(number, "noticed")
            changed = true
            if settings.generated {
                s.start(filter: nil, sourceRect: nil) { why in self.streamStarted(s, why) }
            }
        }
        // Moved (same size): read from its new place.
        for c in candidates {
            guard let s = extras.stream(c.id), !samePlace(s.bounds, c.bounds) else { continue }
            s.moved(to: c.bounds)
            changed = true
        }
        // Nobody watches it within a second: back into the first window's picture, as today.
        for s in extras.streams.values {
            if s.transport.viewers > 0 {
                if !s.watched { s.watched = true; logWindow(s.number, "watched") }
            } else if !s.watched && now - s.listedAt > 1.0 {
                s.stop()
                extras.streams[s.number] = nil
                extras.fallback.insert(s.windowId)
                if extras.focus == s.number { extras.focus = nil }
                logWindow(s.number, "fallback")
                say("window-fallback", [("window", Int(s.number)), ("reason", "nobody watched its stream within one second")])
                changed = true
            }
        }
        // Windows gone are forgotten as fallbacks too.
        extras.fallback = extras.fallback.filter { id in candidates.contains { $0.id == id } }
        if changed && !settings.generated { refreshFilters() }
        publishWindows()
    }

    private func streamStarted(_ s: ExtraStream, _ why: String?) {
        if let why {
            say("window-fallback", [("window", Int(s.number)), ("reason", why)])
            s.stop()
            extras.streams[s.number] = nil
            extras.fallback.insert(s.windowId)
            if !settings.generated { refreshFilters() }
        } else {
            logWindow(s.number, "streaming")
        }
        publishWindows()
    }

    /// The list, to the viewer, when it changed (the latest only; also to a viewer that connects
    /// later). Only streams that run are listed.
    func publishWindows() {
        guard settings.windows, let transport else { return }
        // `key`: the host's keyboard is in that window (the application's focused window, by
        // Accessibility; the stand-in: the one the viewer focused). The viewer follows it only
        // while it is the application in front there (ADR 0027, question 窓7, (a)).
        let focused = lookout.focused
        let entries = extras.streams.values.sorted { $0.number < $1.number }.compactMap { s -> ListedWindow? in
            let port = s.transport.port
            guard port > 0 else { return nil }
            let key = settings.generated ? extras.focus == s.number : focused.map { samePlace($0, s.bounds) } ?? false
            return ListedWindow(number: s.number, port: port, width: s.width, height: s.height, title: s.title, modal: s.modal, key: key)
        }
        let body = windowListJSON(serial: 0, entries)
        guard body != extras.told else { return }
        extras.serial &+= 1
        if transport.setWindows(windowListJSON(serial: extras.serial, entries)) {
            extras.told = body
            say("windows-listed", [("count", entries.count)])
        }
    }

    /// Back to the form of today: every other window's stream ends and the list is emptied.
    func windowsReset(_ why: String) {
        guard settings.windows, extras.declared || !extras.streams.isEmpty else { return }
        for s in extras.streams.values {
            s.stop()
            logWindow(s.number, "gone")
        }
        let had = !extras.streams.isEmpty
        extras.streams.removeAll()
        extras.fallback.removeAll()
        extras.focus = nil
        extras.declared = false
        extras.pretend = []
        extras.viewerAwaySince = nil
        say("windows", [("declared", false), ("reason", why)])
        // The first window's capture takes every window of the application again (now, when the
        // picture runs; else when it is opened again).
        extras.leftOut = []
        if had && !settings.generated { placeFirstAgain() }
        publishWindows()
    }

    /// {"kind":"close","window":N}: the application is asked to close that window only.
    func closeWindowAsked(_ number: UInt32) {
        guard settings.windows, let s = extras.streams[number] else { return say("window-close", [("window", Int(number)), ("asked", false)]) }
        say("window-close", [("window", Int(number)), ("asked", true)])
        if settings.generated {
            // As an application would: the window goes, and with it its stream.
            extras.pretend.removeAll { $0.id == s.windowId }
            return lookAtWindows()
        }
        guard !settings.inputDry, injector.mayPost, !screenLocked(), let pid = app?.processIdentifier else { return }
        let bounds = s.bounds
        lookout.act {
            let element = AXUIElementCreateApplication(pid)
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value) == .success,
                  let windows = value as? [AXUIElement] else { return }
            for window in windows {
                var position: CFTypeRef?, extent: CFTypeRef?
                AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position)
                AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent)
                var p = CGPoint.zero, z = CGSize.zero
                if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
                if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &z) }
                guard samePlace(CGRect(origin: p, size: z), bounds) else { continue }
                var button: CFTypeRef?
                if AXUIElementCopyAttributeValue(window, kAXCloseButtonAttribute as CFString, &button) == .success, let button {
                    _ = AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString)
                }
                return
            }
        }
    }

    /// {"kind":"focus"} with a window's number (none or 1: the first): keys go to that window.
    func focusWindowAsked(_ number: UInt32) {
        guard settings.windows else { return }
        extras.focus = number > 1 && extras.streams[number] != nil ? number : nil
        publishWindows()
    }

    /// Where a pointer position in a window's picture lies on the screen.
    func windowPlace(_ number: UInt32) -> (origin: CGPoint, scale: Double)? {
        guard settings.windows else { return nil }
        return extras.streams[number]?.place
    }

    /// Windows that pointer events in window `number` (1: the first) must pass over: the first
    /// window and the other streamed ones, which may lie over it on the same display.
    func windowsPassedOver(pointerIn number: UInt32) -> Set<CGWindowID> {
        guard settings.windows, !extras.streams.isEmpty else { return [] }
        var over = Set(extras.streams.values.filter { $0.number != number }.map(\.windowId))
        if number > 1, let first = windowId { over.insert(first) }
        return over
    }

    /// The captures' filters again: the first window's leaves the streamed windows out; each
    /// streamed one reads the application's windows on its display from its place, except the
    /// first and the other streamed ones. Streams not started yet start here.
    func refreshFilters() {
        guard !extras.refreshing else { extras.refreshAgain = true; return }
        extras.refreshing = true
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, _ in
            DispatchQueue.main.async {
                self.extras.refreshing = false
                defer {
                    if self.extras.refreshAgain {
                        self.extras.refreshAgain = false
                        self.refreshFilters()
                    }
                }
                guard let content, let app = self.app,
                      let scApp = content.applications.first(where: { $0.processID == app.processIdentifier }) else { return }
                let ids = Set(self.extras.streams.values.map(\.windowId))
                self.extras.leftOut = content.windows.filter { ids.contains($0.windowID) }
                self.placeFirstAgain()
                for s in self.extras.streams.values {
                    let center = CGPoint(x: s.bounds.midX, y: s.bounds.midY)
                    guard let display = content.displays.first(where: { $0.frame.contains(center) }) ?? content.displays.first else { continue }
                    let others = content.windows.filter { ($0.windowID == self.windowId || ids.contains($0.windowID)) && $0.windowID != s.windowId }
                    let filter = SCContentFilter(display: display, including: [scApp], exceptingWindows: others)
                    var rect = s.bounds.offsetBy(dx: -display.frame.minX, dy: -display.frame.minY)
                    rect = rect.intersection(CGRect(origin: .zero, size: display.frame.size))
                    if s.started {
                        s.update(bounds: s.bounds, filter: filter, sourceRect: rect)
                    } else {
                        s.start(filter: filter, sourceRect: rect) { why in self.streamStarted(s, why) }
                    }
                }
            }
        }
    }

    /// The first window's capture with what it leaves out now (place() says it to the capture only
    /// when that changed). Only while its picture runs: a stop or a new size places it anew anyway.
    func placeFirstAgain() {
        guard picture == "running", capture != nil, !restarting, let id = windowId,
              let window = windowList(onScreenOnly: true).first(where: { $0.id == id }) else { return }
        place(window)
    }

    func logWindow(_ number: UInt32, _ event: String) {
        if frameLog.on { frameLog.write("{\"kind\":\"window\",\"id\":\(number),\"event\":\"\(event)\",\"t_ns\":\(nowNs())}") }
    }
}

// MARK: - Self-test

func windowsSelfTest() -> Int {
    var failed = 0
    func check(_ ok: Bool, _ what: String) {
        if ok { print("ok   windows: \(what)") } else { failed += 1; print("FAIL windows: \(what)") }
    }
    let first = WindowInfo(id: 10, pid: 100, layer: 0, bounds: CGRect(x: 0, y: 0, width: 800, height: 600), alpha: 1, onScreen: true)
    let second = WindowInfo(id: 11, pid: 100, layer: 0, bounds: CGRect(x: 40, y: 40, width: 600, height: 400), alpha: 1, onScreen: true)
    let sheet = WindowInfo(id: 12, pid: 100, layer: 0, bounds: CGRect(x: 100, y: 30, width: 400, height: 200), alpha: 1, onScreen: true)
    let menu = WindowInfo(id: 13, pid: 100, layer: 101, bounds: CGRect(x: 50, y: 50, width: 200, height: 300), alpha: 1, onScreen: true)
    let owners = WindowInfo(id: 14, pid: 100, layer: 0, bounds: CGRect(x: 900, y: 0, width: 500, height: 500), alpha: 1, onScreen: true)
    let hidden = WindowInfo(id: 15, pid: 100, layer: 0, bounds: CGRect(x: 0, y: 0, width: 500, height: 500), alpha: 1, onScreen: false)
    let dialog = WindowInfo(id: 16, pid: 100, layer: 0, bounds: CGRect(x: 300, y: 300, width: 420, height: 240), alpha: 1, onScreen: true)
    let ax = [AXWindowInfo(bounds: first.bounds, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String, title: "a"),
              AXWindowInfo(bounds: second.bounds, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String, title: "Untitled 2"),
              AXWindowInfo(bounds: sheet.bounds, role: kAXSheetRole as String, subrole: "", title: ""),
              AXWindowInfo(bounds: owners.bounds, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String, title: "owner"),
              AXWindowInfo(bounds: dialog.bounds, role: kAXWindowRole as String, subrole: kAXDialogSubrole as String, title: "d")]
    let found = extraCandidates(all: [first, second, sheet, menu, owners, hidden, dialog], first: 10, before: [14], ax: ax)
    check(found.map(\.id) == [11, 16], "a second window and a dialog; not the first, a sheet, a menu, a hidden one, nor one from before the start")
    check(found.first?.title == "Untitled 2" && found.first?.modal == false && found.last?.modal == true, "titles and dialogs from Accessibility")
    check(extraCandidates(all: [first, second], first: nil, before: [], ax: ax).isEmpty, "nothing before a first window is shown")
    check(extraCandidates(all: [first, second], first: 10, before: [], ax: []).isEmpty, "nothing that Accessibility has not listed yet")
    // Review 1: no baseline recorded, no other window (the owner's open documents of an application
    // opened as it is); windows there at the first look or within 1.5 s are never other windows.
    check(extraCandidates(all: [first, second, owners], first: 10, before: nil, ax: ax).isEmpty, "without a baseline, nothing is another window")
    var baseline = WindowBaseline()
    baseline.look(all: [10, 14], firstShown: false, now: 100)
    check(baseline.known == nil, "the baseline waits for the first window")
    baseline.look(all: [10, 14], firstShown: true, now: 101)
    baseline.look(all: [10, 14, 17], firstShown: true, now: 102)  // restored at launch, 1 s later
    baseline.look(all: [10, 14, 17, 11], firstShown: true, now: 103)  // Command-N, 2 s later
    check(baseline.known == [10, 14, 17], "windows of the first 1.5 s are the baseline; a later one is not")
    let restored = WindowInfo(id: 17, pid: 100, layer: 0, bounds: CGRect(x: 500, y: 500, width: 300, height: 300), alpha: 1, onScreen: true)
    let restoredAX = AXWindowInfo(bounds: restored.bounds, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String)
    check(extraCandidates(all: [first, second, owners, restored], first: 10, before: baseline.known, ax: ax + [restoredAX]).map(\.id) == [11],
          "only the window made after the baseline is another window")

    let numbers = WindowNumbers()
    check(numbers.number(for: 11) == 2 && numbers.number(for: 16) == 3 && numbers.number(for: 11) == 2, "numbers from 2, kept")
    numbers.forget(11)
    check(numbers.number(for: 11) == 4, "a number is not given again")

    var plan = streamPlan(candidates: [1, 2, 3, 4, 5], streaming: [], fallback: [], most: 3)
    check(plan.start == [1, 2, 3] && plan.over == [4, 5] && plan.stop.isEmpty, "three streams besides the first; the rest stay in the first picture")
    plan = streamPlan(candidates: [2, 3, 6], streaming: [1, 2, 3], fallback: [4], most: 3)
    check(plan.stop == [1] && plan.start == [6] && plan.over.isEmpty, "a window gone stops; its place goes to the next one")
    plan = streamPlan(candidates: [4], streaming: [], fallback: [4], most: 3)
    check(plan.start.isEmpty && plan.over.isEmpty, "a window back in the first picture is not streamed again")

    let json = windowListJSON(serial: 7, [ListedWindow(number: 2, port: 50001, width: 1200, height: 800, title: "Untitled 2", modal: false, key: true)])
    check(json == "{\"serial\":7,\"windows\":[{\"attention\":false,\"height\":800,\"id\":2,\"key\":true,\"modal\":false,\"port\":50001,\"title\":\"Untitled 2\",\"width\":1200}]}",
          "the list's JSON")
    let long = windowListJSON(serial: 1, [ListedWindow(number: 2, port: 1, width: 64, height: 64, title: String(repeating: "あ", count: 100), modal: false, key: false)])
    if let data = long.data(using: .utf8), let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
       let title = (root["windows"] as? [[String: Any]])?.first?["title"] as? String {
        check(title.utf8.count <= 256 && title.count == 85, "a long title is cut at a character's end")
    } else {
        check(false, "a long title is cut at a character's end")
    }
    check(extraPictureSize(CGRect(x: 0, y: 0, width: 600, height: 401), density: 2) == (1200, 802), "a window's picture at its display's density")
    check(extraPictureSize(CGRect(x: 0, y: 0, width: 20, height: 3000), density: 2) == (64, 2160), "within 64 and 3840 x 2160")
    // Review 2: a position in a window that is not listed goes nowhere, and the next press with it.
    let firstPlace = (origin: CGPoint(x: 100, y: 50), scale: 0.5)
    let listed: (UInt32) -> (origin: CGPoint, scale: Double)? = { $0 == 2 ? (CGPoint(x: 400, y: 300), 0.5) : nil }
    check(pointerTarget(x: 10, y: 20, window: nil, first: firstPlace, place: listed) == .at(CGPoint(x: 105, y: 60), window: 1),
          "without a number: the first window, as before")
    check(pointerTarget(x: 10, y: 20, window: 2, first: firstPlace, place: listed) == .at(CGPoint(x: 405, y: 310), window: 2),
          "a listed window's picture")
    check(pointerTarget(x: 10, y: 20, window: 9, first: firstPlace, place: listed) == .nowhere(9), "a number not listed is nowhere")
    check(!passesNowhere(pressed: true, held: false) && passesNowhere(pressed: false, held: true) && !passesNowhere(pressed: false, held: false),
          "nowhere: no press; only the release of a button that is down")
    // Review 9: without --windows Accessibility is asked nothing more than before.
    check(!axAsksTitle(windows: false) && axAsksTitle(windows: true), "a window's title is read only with --windows")
    return failed
}

extension DailyHost {
    /// The injector's part of the other windows (windows.swift), for an application and the stand-in.
    func wireWindows() {
        injector.onWindows = { [weak self] in self?.windowsDeclared() }
        injector.onClose = { [weak self] number in self?.closeWindowAsked(number) }
        injector.onFocusWindow = { [weak self] number in self?.focusWindowAsked(number) }
        injector.windowPlace = { [weak self] number in self?.windowPlace(number) }
    }
}
