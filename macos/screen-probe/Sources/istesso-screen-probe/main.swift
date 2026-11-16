// Watches two windows on this Mac's screen and records when each reacted to the same input.
//
// Two copies of the test application (istesso-flash) take the same synthetic input at the same
// moment. One runs on this Mac. The other runs on the host and is shown here by a streaming
// client. This program sends the inputs, looks at one point in each window through
// ScreenCaptureKit, and writes down when each window first showed the new shade. The two logs
// are compared with `istesso measure compare`, and each can be paired with the log of its test
// application by `istesso measure report`.
//
// Of the screen it reads the brightness of those two points and nothing else. No picture is
// stored or sent anywhere. Why a measurement tool records the screen at all is written down in
// docs/adr/0008-screen-recording-for-measurement.md; how to use it, in
// docs/measurements/README.md.

import CoreGraphics
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import ScreenCaptureKit

// MARK: - Errors

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func complain(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}

// MARK: - Clock

/// Nanoseconds on the clock ScreenCaptureKit stamps frames with (mach absolute time).
/// Inputs are stamped on the same clock, so the two can be subtracted.
struct MachClock {
    private let numer: UInt64
    private let denom: UInt64

    init() {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        numer = UInt64(info.numer)
        denom = UInt64(max(info.denom, 1))
    }

    func nanoseconds(_ ticks: UInt64) -> UInt64 {
        let (whole, rest) = ticks.quotientAndRemainder(dividingBy: denom)
        return whole * numer + rest * numer / denom
    }

    func now() -> UInt64 { nanoseconds(mach_absolute_time()) }
}

// MARK: - What the test application shows

enum Shade {
    case dark, light

    var toggled: Shade { self == .dark ? .light : .dark }
}

/// The rule of `istesso_measure::classify`: a picture counts as a shade when its brightness is
/// within a quarter of the way from that shade (0x20 or 0xE0) to the other.
func classify(luma: Double) -> Shade? {
    let dark = 32.0, light = 224.0
    let quarter = (light - dark) / 4
    if luma < 0 { return nil }
    if luma <= dark + quarter { return .dark }
    if luma >= light - quarter { return .light }
    return nil
}

// MARK: - Spacing of inputs

/// The intervals of `istesso_measure::Jitter` (SplitMix64): the same seed gives the same run in
/// both tools. Not for anything related to security.
struct Jitter {
    private var state: UInt64
    private let minNs: UInt64
    private let spanNs: UInt64

    init(seed: UInt64, minMs: Int, maxMs: Int) {
        let low = UInt64(max(0, min(minMs, maxMs))) * 1_000_000
        let high = UInt64(max(0, max(minMs, maxMs))) * 1_000_000
        state = seed
        minNs = low
        spanNs = high - low
    }

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        z ^= z >> 31
        return minNs + z.multipliedFullWidth(by: spanNs + 1).high
    }
}

// MARK: - Command line

/// Says which window to watch.
enum WindowSelector {
    case title(String)
    case owner(String)
    case point(CGPoint)

    init(_ text: String, option: String) throws {
        if text.hasPrefix("title:") {
            self = .title(String(text.dropFirst("title:".count)))
        } else if text.hasPrefix("owner:") {
            self = .owner(String(text.dropFirst("owner:".count)))
        } else if text.hasPrefix("point:") {
            let parts = text.dropFirst("point:".count).split(separator: ",")
            guard parts.count == 2, let x = Double(parts[0]), let y = Double(parts[1]) else {
                throw Failure("\(option): a point is written point:X,Y")
            }
            self = .point(CGPoint(x: x, y: y))
        } else {
            throw Failure("\(option) takes title:TEXT, owner:APPLICATION or point:X,Y")
        }
    }
}

struct Options {
    var local = ""
    var relayed = ""
    var localWindow = WindowSelector.title("Istesso Flash")
    var relayedWindow = WindowSelector.owner("Moonlight")
    var localLog = ""
    var relayedLog = ""
    var count = 500
    var minMs = 200
    var maxMs = 400
    var timeoutMs = 1500
    var relayedInterface: String? = nil
    var seed = UInt64(Date().timeIntervalSince1970 * 1000)

    static let usage = """
    Usage: istesso-screen-probe --local ADDRESS:PORT --relayed ADDRESS:PORT \\
               --local-log FILE --relayed-log FILE [options]

    Sends the same synthetic input to two copies of istesso-flash, one on this Mac and one
    on the host, and records when the window of each first showed it on this Mac's screen.

      --local ADDRESS:PORT       where the test application on this Mac takes inputs
      --relayed ADDRESS:PORT     where the test application on the host takes inputs
      --local-log FILE           log for the local window (must not exist)
      --relayed-log FILE         log for the relayed window (must not exist)
      --local-window SELECTOR    window of the local test application
                                 (default: title:Istesso Flash)
      --relayed-window SELECTOR  window that shows the relayed one (default: owner:Moonlight)
      --count N                  number of inputs (default: 500)
      --min-ms N, --max-ms N     interval between inputs (default: 200 to 400)
      --timeout-ms N             how long to wait for a window to react (default: 1500)
      --relayed-interface NAME   send the host's inputs out of this network interface (en8, say),
                                 whichever one the system would choose: for measuring on a cable
                                 while Wi-Fi is still on
      --seed N                   seed of the intervals; the same seed repeats the same run

    A SELECTOR is title:TEXT (the window's title), owner:APPLICATION (the largest window of
    that application) or point:X,Y (points from the top left of the main display). The
    middle of the window is watched, so the test application has to cover it.

    Needs the permission to record the screen. Unencrypted inputs: trusted networks only.
    """

    static func parse(_ arguments: [String]) throws -> Options {
        var options = Options()
        var index = 0
        func value(_ name: String) throws -> String {
            index += 1
            guard index < arguments.count else { throw Failure("\(name) needs a value") }
            return arguments[index]
        }
        func number(_ name: String) throws -> Int {
            guard let n = Int(try value(name)), n >= 0 else { throw Failure("\(name) needs a number") }
            return n
        }
        while index < arguments.count {
            let name = arguments[index]
            switch name {
            case "--local": options.local = try value(name)
            case "--relayed": options.relayed = try value(name)
            case "--local-log": options.localLog = try value(name)
            case "--relayed-log": options.relayedLog = try value(name)
            case "--local-window": options.localWindow = try WindowSelector(try value(name), option: name)
            case "--relayed-window": options.relayedWindow = try WindowSelector(try value(name), option: name)
            case "--count": options.count = try number(name)
            case "--min-ms": options.minMs = try number(name)
            case "--max-ms": options.maxMs = try number(name)
            case "--timeout-ms": options.timeoutMs = try number(name)
            case "--relayed-interface": options.relayedInterface = try value(name)
            case "--seed":
                guard let seed = UInt64(try value(name)) else { throw Failure("--seed needs a number") }
                options.seed = seed
            case "-h", "--help":
                print(usage)
                exit(0)
            default:
                throw Failure("unknown option \(name)\n\n\(usage)")
            }
            index += 1
        }
        for (name, given) in [("--local", options.local), ("--relayed", options.relayed),
                              ("--local-log", options.localLog), ("--relayed-log", options.relayedLog)]
        where given.isEmpty {
            throw Failure("\(name) is required\n\n\(usage)")
        }
        return options
    }
}

// MARK: - Inputs

/// Opens a UDP socket to a test application. Errors name the option, never the address:
/// what this program prints ends up in measurement records.
func openSocket(to target: String, option: String, interface: String? = nil) throws -> Int32 {
    guard let colon = target.lastIndex(of: ":") else {
        throw Failure("\(option) needs ADDRESS:PORT")
    }
    let host = String(target[..<colon]).trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
    let port = String(target[target.index(after: colon)...])
    var hints = addrinfo()
    hints.ai_socktype = SOCK_DGRAM
    var list: UnsafeMutablePointer<addrinfo>?
    guard getaddrinfo(host, port, &hints, &list) == 0, let first = list else {
        throw Failure("cannot resolve the address given to \(option)")
    }
    defer { freeaddrinfo(list) }
    let descriptor = socket(first.pointee.ai_family, first.pointee.ai_socktype, first.pointee.ai_protocol)
    guard descriptor >= 0 else { throw Failure("cannot open a socket for \(option)") }
    if let interface {
        // Bound to the interface before it is connected: the datagrams then leave by it even
        // where another interface comes first in the system's order.
        var index = if_nametoindex(interface)
        let level = first.pointee.ai_family == AF_INET6 ? IPPROTO_IPV6 : IPPROTO_IP
        let name = first.pointee.ai_family == AF_INET6 ? IPV6_BOUND_IF : IP_BOUND_IF
        guard index != 0,
              setsockopt(descriptor, level, name, &index, socklen_t(MemoryLayout<UInt32>.size)) == 0
        else {
            close(descriptor)
            throw Failure("cannot use the network interface \(interface) for \(option)")
        }
    }
    guard connect(descriptor, first.pointee.ai_addr, first.pointee.ai_addrlen) == 0 else {
        close(descriptor)
        throw Failure("cannot reach the address given to \(option)")
    }
    return descriptor
}

/// Sends the datagram of `istesso_measure::FlashInput`: "IFI1" and the input's number.
///
/// `nil` when it went out, otherwise what the system said (`errno`). A wireless interface
/// that is busy for a moment refuses a datagram, so it is offered up to four times, a
/// millisecond apart.
func sendInput(_ descriptor: Int32, id: UInt64) -> Int32? {
    var bytes = [UInt8]("IFI1".utf8)
    withUnsafeBytes(of: id.bigEndian) { bytes.append(contentsOf: $0) }
    var refusal: Int32 = 0
    for attempt in 0..<4 {
        if attempt > 0 { usleep(1_000) }
        if bytes.withUnsafeBytes({ send(descriptor, $0.baseAddress, $0.count, 0) }) == bytes.count {
            return nil
        }
        refusal = errno
    }
    return refusal
}

/// Counts the inputs that could not be sent to one of the test applications. One that does
/// not go out is one input lost; five in a row mean that nothing takes them.
struct Unsent {
    let option: String
    var total = 0
    var inARow = 0
    var last: Int32 = 0

    mutating func note(_ refusal: Int32?) throws {
        guard let refusal else {
            inARow = 0
            return
        }
        total += 1
        inARow += 1
        last = refusal
        if inARow >= 5 {
            throw Failure("nothing takes inputs at the address given to \(option) (\(String(cString: strerror(refusal))))")
        }
    }

    var summary: String? {
        total == 0 ? nil : "\(total) inputs could not be sent to the address given to \(option) (last: \(String(cString: strerror(last))))"
    }
}

// MARK: - Windows

struct WindowInfo {
    let number: Int
    let owner: String
    let title: String
    let frame: CGRect
    let layer: Int
}

/// The windows on screen, front to back.
func windowsOnScreen() -> [WindowInfo] {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let entries = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return []
    }
    return entries.compactMap { entry in
        guard let number = entry[kCGWindowNumber as String] as? Int,
              let bounds = entry[kCGWindowBounds as String] as? NSDictionary,
              let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary)
        else { return nil }
        return WindowInfo(
            number: number,
            owner: entry[kCGWindowOwnerName as String] as? String ?? "",
            title: entry[kCGWindowName as String] as? String ?? "",
            frame: frame,
            layer: entry[kCGWindowLayer as String] as? Int ?? 0
        )
    }
}

/// The point to watch for one side: the middle of the selected window. Fails when another
/// window lies over that point, because then the wrong picture would be measured.
///
/// The selected window may be an ordinary one or one that is kept above the ordinary ones (a
/// measurement window is kept on top so that nothing comes to lie over it while the person goes
/// on working). What can lie over it is a window of its own level or of a level below, further
/// to the front: the menu bar, the Dock and what the system draws over everything are left out.
func watchPoint(_ selector: WindowSelector, among windows: [WindowInfo], side: String) throws -> CGPoint {
    let floating = Int(CGWindowLevelForKey(.floatingWindow))
    let selectable = windows.filter { (0...floating).contains($0.layer) }
    let candidates: [WindowInfo]
    switch selector {
    case .point(let point):
        return point
    case .title(let title):
        candidates = selectable.filter { $0.title == title }
    case .owner(let owner):
        candidates = selectable.filter { $0.owner == owner }
    }
    guard let target = candidates.max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height })
    else {
        throw Failure("no window on screen for the \(side) side")
    }
    let middle = CGPoint(x: target.frame.midX, y: target.frame.midY)
    // The list runs front to back, so the first window that holds the point is the one seen there.
    let seen = windows.first { (0...target.layer).contains($0.layer) && $0.frame.contains(middle) }
    if let front = seen, front.number != target.number {
        throw Failure("the middle of the \(side) window is covered by a window of \(front.owner)")
    }
    return middle
}

// MARK: - The screen

/// Looks at every frame of the display and notes the shade at each watched point.
final class Watcher: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let clock: MachClock
    /// Watched points, in pixels of the captured picture.
    private let points: [(x: Int, y: Int)]
    private let condition = NSCondition()
    private var shades: [Shade?]
    private var expected: [Shade]?
    private var armedAtNs: UInt64 = 0
    private var shownNs: [UInt64?]
    private var stopped: String?
    private(set) var frames = 0

    init(clock: MachClock, points: [(x: Int, y: Int)]) {
        self.clock = clock
        self.points = points
        shades = Array(repeating: nil, count: points.count)
        shownNs = Array(repeating: nil, count: points.count)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, CMSampleBufferIsValid(sampleBuffer),
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]],
              let info = attachments.first,
              let rawStatus = info[.status] as? Int,
              let status = SCFrameStatus(rawValue: rawStatus),
              status == .complete || status == .started,
              let displayTicks = info[.displayTime] as? UInt64,
              let picture = CMSampleBufferGetImageBuffer(sampleBuffer)
        else { return }

        // When the system put this frame on the display.
        let displayNs = clock.nanoseconds(displayTicks)
        CVPixelBufferLockBaseAddress(picture, .readOnly)
        let seen = points.map { classify(luma: Watcher.luma(of: picture, around: $0)) }
        CVPixelBufferUnlockBaseAddress(picture, .readOnly)

        condition.lock()
        frames += 1
        shades = seen
        if let expected {
            for index in expected.indices
            where shownNs[index] == nil && seen[index] == expected[index] && displayNs >= armedAtNs {
                shownNs[index] = displayNs
            }
        }
        condition.broadcast()
        condition.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        condition.lock()
        stopped = error.localizedDescription
        condition.broadcast()
        condition.unlock()
    }

    /// Average brightness (BT.709) of the 5x5 pixels around a point of a BGRA picture.
    private static func luma(of picture: CVPixelBuffer, around point: (x: Int, y: Int)) -> Double {
        guard let base = CVPixelBufferGetBaseAddress(picture) else { return -1 }
        let width = CVPixelBufferGetWidth(picture)
        let height = CVPixelBufferGetHeight(picture)
        let rowBytes = CVPixelBufferGetBytesPerRow(picture)
        guard width > 0, height > 0 else { return -1 }
        let columns = max(0, min(point.x, width - 1) - 2)...min(width - 1, max(point.x, 0) + 2)
        let rows = max(0, min(point.y, height - 1) - 2)...min(height - 1, max(point.y, 0) + 2)
        var sum = 0.0
        for y in rows {
            for x in columns {
                let pixel = base.advanced(by: y * rowBytes + x * 4).assumingMemoryBound(to: UInt8.self)
                sum += 0.0722 * Double(pixel[0]) + 0.7152 * Double(pixel[1]) + 0.2126 * Double(pixel[2])
            }
        }
        return sum / Double(columns.count * rows.count)
    }

    /// The shade at every point, once each point shows one of the two shades.
    func settledShades(timeout: TimeInterval) throws -> [Shade]? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let stopped { throw Failure("the screen recording stopped: \(stopped)") }
            let settled = shades.compactMap { $0 }
            if settled.count == shades.count { return settled }
            if !condition.wait(until: deadline) { return nil }
        }
    }

    /// From now on, note the first frame in which each point shows its expected shade.
    func arm(expecting expected: [Shade], atNs: UInt64) {
        condition.lock()
        self.expected = expected
        armedAtNs = atNs
        shownNs = Array(repeating: nil, count: expected.count)
        condition.unlock()
    }

    /// When each point first showed its expected shade; nil for a point that did not in time.
    func collect(timeout: TimeInterval) -> [UInt64?] {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer {
            expected = nil
            condition.unlock()
        }
        while shownNs.contains(where: { $0 == nil }), stopped == nil {
            if !condition.wait(until: deadline) { break }
        }
        return shownNs
    }
}

final class Box<Value>: @unchecked Sendable {
    var value: Value
    init(_ value: Value) { self.value = value }
}

func shareableDisplays() throws -> [SCDisplay] {
    let done = DispatchSemaphore(value: 0)
    let result = Box<Result<[SCDisplay], Error>>(.failure(Failure("no answer from the system")))
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
        if let content {
            result.value = .success(content.displays)
        } else {
            result.value = .failure(error ?? Failure("the system gave no list of displays"))
        }
        done.signal()
    }
    done.wait()
    return try result.value.get()
}

func waitUntilDone(_ what: String, _ start: (@escaping @Sendable (Error?) -> Void) -> Void) throws {
    let done = DispatchSemaphore(value: 0)
    let failure = Box<Error?>(nil)
    start { error in
        failure.value = error
        done.signal()
    }
    done.wait()
    if let error = failure.value {
        throw Failure("\(what): \(error.localizedDescription)")
    }
}

// MARK: - Logs

/// One log in the format of `istesso_measure::record`. Refuses to overwrite a file.
final class Log {
    private let handle: FileHandle

    init(path: String) throws {
        let descriptor = open(path, O_WRONLY | O_CREAT | O_EXCL, 0o644)
        guard descriptor >= 0 else {
            throw Failure("cannot create \(path): \(String(cString: strerror(errno)))")
        }
        handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        let startedMs = UInt64(Date().timeIntervalSince1970 * 1000)
        write("{\"kind\":\"header\",\"side\":\"client\",\"tool\":\"istesso-screen-probe\",\"version\":\"0.0.1\",\"started_unix_ms\":\(startedMs)}")
    }

    func shown(id: UInt64, sentNs: UInt64, shownNs: UInt64) {
        write("{\"kind\":\"client\",\"id\":\(id),\"sent_ns\":\(sentNs),\"shown_ns\":\(shownNs)}")
    }

    private func write(_ line: String) {
        handle.write(Data((line + "\n").utf8))
    }
}

// MARK: - The measurement

func run() throws {
    let options = try Options.parse(Array(CommandLine.arguments.dropFirst()))

    guard CGPreflightScreenCaptureAccess() else {
        CGRequestScreenCaptureAccess()
        throw Failure("""
        this program may not record the screen yet. Allow it under System Settings > \
        Privacy & Security > Screen & System Audio Recording, then run it again
        """)
    }

    let windows = windowsOnScreen()
    let localPoint = try watchPoint(options.localWindow, among: windows, side: "local")
    let relayedPoint = try watchPoint(options.relayedWindow, among: windows, side: "relayed")
    guard let display = try shareableDisplays().first(where: {
        $0.frame.contains(localPoint) && $0.frame.contains(relayedPoint)
    }) else {
        throw Failure("""
        the two windows are not on one display. Put them on the same display: otherwise the \
        difference between the displays is measured, not the relay
        """)
    }
    let refreshHz = CGDisplayCopyDisplayMode(display.displayID)?.refreshRate ?? 0

    // A quarter-size picture of the display is plenty to tell two greys apart.
    let width = max(64, Int(display.frame.width / 4))
    let height = max(64, Int(display.frame.height / 4))
    func pixel(_ point: CGPoint) -> (x: Int, y: Int) {
        let x = (point.x - display.frame.minX) / display.frame.width * CGFloat(width)
        let y = (point.y - display.frame.minY) / display.frame.height * CGFloat(height)
        return (x: min(width - 1, max(0, Int(x))), y: min(height - 1, max(0, Int(y))))
    }

    let configuration = SCStreamConfiguration()
    configuration.width = width
    configuration.height = height
    configuration.minimumFrameInterval = CMTime(value: 1, timescale: 240)
    configuration.pixelFormat = kCVPixelFormatType_32BGRA
    configuration.showsCursor = false
    configuration.queueDepth = 8

    let clock = MachClock()
    let watcher = Watcher(clock: clock, points: [pixel(localPoint), pixel(relayedPoint)])
    let stream = SCStream(
        filter: SCContentFilter(display: display, excludingWindows: []),
        configuration: configuration,
        delegate: watcher
    )
    try stream.addStreamOutput(
        watcher,
        type: .screen,
        sampleHandlerQueue: DispatchQueue(label: "istesso.screen-probe.frames", qos: .userInteractive)
    )
    try waitUntilDone("cannot start recording the screen") { stream.startCapture(completionHandler: $0) }
    defer { try? waitUntilDone("stop") { stream.stopCapture(completionHandler: $0) } }

    let localSocket = try openSocket(to: options.local, option: "--local")
    let relayedSocket = try openSocket(to: options.relayed, option: "--relayed", interface: options.relayedInterface)
    defer {
        close(localSocket)
        close(relayedSocket)
    }
    let localLog = try Log(path: options.localLog)
    let relayedLog = try Log(path: options.relayedLog)

    var jitter = Jitter(seed: options.seed, minMs: options.minMs, maxMs: options.maxMs)
    let timeout = Double(options.timeoutMs) / 1000
    var shownLocal = 0
    var shownRelayed = 0
    var unsentLocal = Unsent(option: "--local")
    var unsentRelayed = Unsent(option: "--relayed")
    var missedInARow = 0

    for id in 0..<UInt64(options.count) {
        guard let before = try watcher.settledShades(timeout: 3) else {
            throw Failure("""
            cannot tell which shade the windows show. Both test applications have to be \
            visible and cover the middle of their windows
            """)
        }
        let sentNs = clock.now()
        watcher.arm(expecting: before.map(\.toggled), atNs: sentNs)
        try unsentLocal.note(sendInput(localSocket, id: id))
        try unsentRelayed.note(sendInput(relayedSocket, id: id))

        let shown = watcher.collect(timeout: timeout)
        if let at = shown[0] {
            localLog.shown(id: id, sentNs: sentNs, shownNs: at)
            shownLocal += 1
        }
        if let at = shown[1] {
            relayedLog.shown(id: id, sentNs: sentNs, shownNs: at)
            shownRelayed += 1
        }
        if shown.contains(where: { $0 == nil }) {
            // A window that stops reacting has usually come to lie under another one: say so
            // and stop, instead of waiting out the rest of the inputs one timeout at a time.
            missedInARow += 1
            if missedInARow == 5 {
                let windows = windowsOnScreen()
                _ = try watchPoint(options.localWindow, among: windows, side: "local")
                _ = try watchPoint(options.relayedWindow, among: windows, side: "relayed")
            }
            // Let a reaction that is merely late pass, so it is not taken for the next one.
            Thread.sleep(forTimeInterval: 1.0)
        } else {
            missedInARow = 0
        }

        let next = sentNs + jitter.next()
        let now = clock.now()
        if next > now {
            Thread.sleep(forTimeInterval: Double(next - now) / 1e9)
        }
    }

    print("Display: \(Int(display.frame.width))x\(Int(display.frame.height)) points at \(String(format: "%.2f", refreshHz)) Hz")
    print("Inputs: \(options.count) sent, \(shownLocal) shown by the local window, \(shownRelayed) by the relayed window (seed \(options.seed))")
    for unsent in [unsentLocal, unsentRelayed] {
        if let summary = unsent.summary { print(summary) }
    }
    print("Frames looked at: \(watcher.frames)")
    print("Compare the logs with: istesso measure compare --local \(options.localLog) --relayed \(options.relayedLog) --refresh-hz \(String(format: "%.2f", refreshHz))")
}

do {
    try run()
} catch let failure as Failure {
    complain("error: \(failure)")
    exit(1)
} catch {
    complain("error: \(error.localizedDescription)")
    exit(1)
}
