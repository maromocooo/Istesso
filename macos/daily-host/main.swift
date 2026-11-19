// Istesso Dev Daily Host (com.maromoco.istesso-dev.daily-host): the Mac as a host for the everyday
// trial (docs/m4-daily-host.md). TRIAL ONLY: nothing of it is the product's Mac host.
//
// It does on the Mac what scripts/m1-step3-host.py does on the Linux host, so that the same viewer
// (macos/gst-viewer, the trial's window on the other Mac) and AURA receive it unchanged:
//
//   picture  ScreenCaptureKit -> VideoToolbox H.265 -> Annex B -> istesso-media's host
//            (istesso_media_host_send: the encrypted, authenticated transport of ADR 0015, only to
//            paired devices that may look)
//   input    one JSON event per datagram on a socket, as `istesso agent` hands them over
//            (istesso_proto::InputEvent: key, pointer_at, button, scroll, and the trial's text),
//            keys and committed text posted to the application's own process (CGEventPostToPid);
//            pointer events either to the process (forms 1-3: other processes ignore such clicks)
//            or, with --click-form 4 (the everyday trial's default for an application's windows),
//            through the session onto the range only: the application's own windows, or the
//            whole display in the display form. Never anything but releases while the screen is
//            locked (ADR 0017, "守ること" 3, and the proposal added to it).
//   words    pause, detach, resume, admit ID KEY | admit nobody, close: one a line, on a pipe
//            (--control); the answers are the same JSON lines, each with the key "istesso", on
//            standard output.
//
// What it captures: the whole main display (--display; for the first check only, ADR 0017 keeps
// the product to the application's windows), or the windows of one application (--app NAME), which
// it opens. Input then goes to that application's process and nowhere else.
//
// It never asks for a permission. Without Screen Recording it captures nothing and says so; without
// Accessibility it passes no input and says so. Nothing typed is written anywhere: the logs hold
// times and counts only.
//
// --generated draws a moving picture of its own instead of capturing (no permission needed), and
// --input-dry counts the input instead of posting it: for checks that need no permission.

import AppKit
import ApplicationServices
import CoreMedia
import CoreVideo
import IOKit.pwr_mgt
import ScreenCaptureKit
import VideoToolbox

// MARK: - Settings

enum Target {
    case display
    case app(String)
}

// The system's confirmations, as observed on the Mac Studio on 2026-11-19 (macOS 26 line,
// --prompt-windows while the owner held each open): the administrator password dialog is a
// window of com.apple.SecurityAgent at layer 1000; a Files-and-Folders privacy prompt is a
// window of com.apple.UserNotificationCenter at layer 8 (the modal panel level); the
// Accessibility prompt is one of com.apple.accessibility.universalAccessAuthWarn at layer 0
// (461x181; that process shows nothing else, so layer 0 is safe for it). Others are added only
// once seen.
let systemPromptMinLayers: [String: Int] = [
    "com.apple.SecurityAgent": 8,
    "com.apple.UserNotificationCenter": 8,
    "com.apple.accessibility.universalAccessAuthWarn": 0,
]
let systemPromptOwners: Set<String> = Set(systemPromptMinLayers.keys)
/// The lowest layer for an owner that is listed without one of its own (the owner's file).
let systemPromptMinLayer = Int(CGWindowLevelForKey(.modalPanelWindow)) // 8

func validSystemPromptOwner(_ owner: String) -> Bool {
    guard owner.utf8.count <= 128 else { return false }
    let label = "[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
    return owner.range(of: "\\A\(label)(?:\\.\(label))+\\z", options: .regularExpression) != nil
}

struct Settings {
    var promptOwners = systemPromptOwners
    var port = 47911
    var bind = "[::]"
    var configDir: String?
    var admitNobody = false
    var inputSocket: String?
    var control: String?
    var log: String?
    var target = Target.display
    var size: (Int, Int)?
    var fps = 60
    var chroma: UInt32 = 444
    var bitrateKbit = 30000
    var keepAwake = 20
    var minQP = 0
    var generated = false
    var inputDry = false
    var fitWindow = true
    var clickForm = 1
    var pidFile: String?
    var document: String?
    /// The application is launched without its windows of last time (an argument for this launch
    /// only; nothing of its settings is changed).
    var noRestore = false
    var signal = NSHomeDirectory() + "/.local/state/istesso/measuring"
    /// The application form on a virtual display of this size (points, at 2 pixels a point) and
    /// rate: the receiver's screen. Made while a device is shown the picture (trial only).
    var virtualDisplay: (width: Int, height: Int, hertz: Double)?
    /// How long a virtual display stays after the last viewer left (it may come back).
    var virtualKeep: Double = 30
    var virtualNow = false
    /// Which of several virtual displays this program's is (1-3): they are laid in a row from the
    /// main display's bottom right corner, so that each touches the main one at most at a corner.
    var virtualSlot = 1
    /// Start with a new window of the application's own (Command-N to its process once it is in
    /// front), never with what it opens by itself; the new window is the one shown.
    var newWindow = false
    /// Copy and paste of text with the trial agent (ADR 0022); off unless asked for.
    var clipboard = false
    /// Composing text inline in the application through Istesso Dev Input (ADR 0023, inline.swift);
    /// off unless asked for (--inline, or ISTESSO_DAILY_INLINE=1): the viewer keeps its strip.
    var inline = ProcessInfo.processInfo.environment["ISTESSO_DAILY_INLINE"] == "1"
    /// How long this Mac's own keyboard must be idle before Istesso's source is selected again.
    var inlineIdle = 3.0
}

func usage() -> Never {
    FileHandle.standardError.write(Data("""
    usage: istesso-daily-host [--port N] [--bind ADDR] [--config-dir DIR] [--admit nobody]
           [--display | --app NAME|BUNDLE-ID|PATH] [--size WxH] [--fps N] [--chroma 444|420]
           [--bitrate KBIT] [--keep-awake FPS] [--min-qp N] [--input-socket PATH] [--input-dry]
           [--control FIFO] [--log FILE|-] [--no-fit-window] [--click-form 1|2|3] [--generated]
           [--pid-file FILE] [--signal FILE] [--system-prompt-owner BUNDLE_ID]...
           [--inline [--inline-local-idle SECONDS]]

    """.utf8))
    exit(2)
}

func readSettings() -> Settings {
    var s = Settings()
    var promptOwnerCount = 0
    var args = Array(CommandLine.arguments.dropFirst())
    func value() -> String {
        guard !args.isEmpty else { usage() }
        return args.removeFirst()
    }
    func number() -> Int {
        guard let n = Int(value()) else { usage() }
        return n
    }
    while !args.isEmpty {
        let word = args.removeFirst()
        switch word {
        case "--port": s.port = number()
        case "--bind": s.bind = value()
        case "--config-dir": s.configDir = value()
        case "--admit": s.admitNobody = value() == "nobody"
        case "--input-socket": s.inputSocket = value()
        case "--input-dry": s.inputDry = true
        case "--control": s.control = value()
        case "--log": let v = value(); s.log = v == "-" ? nil : v
        case "--display": s.target = .display
        case "--app": s.target = .app(value())
        case "--system-prompt-owner":
            let owner = value()
            guard promptOwnerCount < 16, validSystemPromptOwner(owner) else { usage() }
            promptOwnerCount += 1
            s.promptOwners.insert(owner)
        case "--document": s.document = value()
        case "--no-restore": s.noRestore = true
        case "--size":
            let parts = value().split(separator: "x").compactMap { Int($0) }
            guard parts.count == 2, parts[0] >= 64, parts[1] >= 64, parts[0] <= 8192, parts[1] <= 8192 else { usage() }
            // Even sizes: the encoder's 4:2:0 cannot take odd ones.
            s.size = (parts[0] & ~1, parts[1] & ~1)
        case "--fps": s.fps = max(1, min(240, number()))
        case "--chroma":
            let c = number()
            guard c == 444 || c == 420 else { usage() }
            s.chroma = UInt32(c)
        case "--bitrate": s.bitrateKbit = max(500, number())
        case "--keep-awake": s.keepAwake = max(0, min(60, number()))
        case "--min-qp": s.minQP = max(0, min(51, number()))
        case "--generated": s.generated = true
        case "--no-fit-window": s.fitWindow = false
        case "--click-form": s.clickForm = number()
        case "--pid-file": s.pidFile = value()
        case "--signal": s.signal = value()
        case "--virtual-display":
            let parts = value().split(whereSeparator: { $0 == "x" || $0 == "@" }).compactMap { Double($0) }
            guard parts.count == 3, (320...4096).contains(parts[0]), (240...4096).contains(parts[1]), (30...240).contains(parts[2]) else { usage() }
            s.virtualDisplay = (Int(parts[0]), Int(parts[1]), parts[2])
        case "--virtual-keep": s.virtualKeep = Double(number())
        case "--virtual-now": s.virtualNow = true
        case "--virtual-slot": s.virtualSlot = max(1, min(3, number()))
        case "--new-window": s.newWindow = true
        case "--clipboard": s.clipboard = true
        case "--inline": s.inline = true
        case "--inline-local-idle": s.inlineIdle = Double(max(1, min(60, number())))
        default: usage()
        }
    }
    return s
}

if CommandLine.arguments.dropFirst().first == "--self-test" { selfTest() }
if CommandLine.arguments.dropFirst().first == "--close-document" { closeDocument(Array(CommandLine.arguments.dropFirst(2))) }
if CommandLine.arguments.dropFirst().first == "--close-window" { closeWindowAt(Array(CommandLine.arguments.dropFirst(2))) }
if CommandLine.arguments.dropFirst().first == "--displays" {
    // Read only: every active display, its place in the arrangement, and its mode.
    var count: UInt32 = 0
    var ids = [CGDirectDisplayID](repeating: 0, count: 16)
    CGGetActiveDisplayList(16, &ids, &count)
    for id in ids.prefix(Int(count)) {
        let b = CGDisplayBounds(id)
        let m = CGDisplayCopyDisplayMode(id)
        print("display main=\(CGDisplayIsMain(id) != 0) origin=\(Int(b.minX)),\(Int(b.minY)) size=\(Int(b.width))x\(Int(b.height)) mode=\(m?.width ?? 0)x\(m?.height ?? 0)pt/\(m?.pixelWidth ?? 0)x\(m?.pixelHeight ?? 0)px@\(Int(m?.refreshRate ?? 0))Hz builtin=\(CGDisplayIsBuiltin(id) != 0)")
    }
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "--windows", let pid = Int(CommandLine.arguments.dropFirst(2).first ?? "") {
    // Read only: where the windows of one process are (no titles).
    for window in readWindowList(onScreenOnly: false, desktop: false) where window.pid == pid_t(pid) && window.layer == 0 {
        print("window \(window.id) at \(Int(window.bounds.minX)),\(Int(window.bounds.minY)) size \(Int(window.bounds.width))x\(Int(window.bounds.height)) on_screen=\(window.onScreen)")
    }
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "--access-behavior" {
    // Only how this program may read the general pasteboard (never what is on it).
    let names = [0: "default (asks on the first programmatic read)", 1: "ask", 2: "always allow", 3: "always deny"]
    if #available(macOS 15.4, *) {
        let value = NSPasteboard.general.accessBehavior.rawValue
        print("pasteboard access behavior: \(value) \(names[value] ?? "unknown")")
    } else {
        print("pasteboard access behavior: not on this macOS")
    }
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "--prompt-windows" {
    // Read only, for collecting what the system's confirmations look like. Every on-screen window
    // above the normal layer, and the normal-layer windows of Apple's own processes: layer, size,
    // whether on screen, and the owner - "an application" for any application the person uses
    // (with a Dock tile), named or not; Apple's bundle id for the system's own agents; or, with
    // no bundle id, the
    // executable's name when it lives below /System/ or /usr/ (the system's agents); else
    // "another application" (only counted). Never a title.
    func owner(_ pid: pid_t) -> (String, apple: Bool) {
        if let running = NSRunningApplication(processIdentifier: pid) {
            // An application the person uses (it has a Dock tile): which one is their activity,
            // never named, Apple's or not.
            if running.activationPolicy == .regular { return ("an application", true) }
            if let id = running.bundleIdentifier, !id.isEmpty {
                return id.hasPrefix("com.apple.") ? (id, true) : ("another application", false)
            }
        }
        var buffer = [CChar](repeating: 0, count: 4096)
        if proc_pidpath(pid, &buffer, UInt32(buffer.count)) > 0 {
            let path = String(cString: buffer)
            if path.hasPrefix("/System/") || path.hasPrefix("/usr/") {
                return ("no bundle id: " + (path as NSString).lastPathComponent, true)
            }
        }
        return ("another application", false)
    }
    var others = 0, listed = 0
    for window in readWindowList(onScreenOnly: false, desktop: false) {
        let (name, apple) = owner(window.pid)
        guard window.onScreen, window.layer >= 0 else { continue }
        if !apple { others += 1; continue }
        listed += 1
        print("layer \(window.layer) \(name) \(Int(window.bounds.width))x\(Int(window.bounds.height)) on_screen=\(window.onScreen)")
    }
    print("listed: \(listed); windows of other applications (only counted): \(others)")
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "--ask-desktop" {
    // A harmless Files-and-Folders confirmation for the owner's samples: asks to look into the
    // Desktop folder, and says only whether it was allowed (no names, no counts).
    // The folder is only opened, never read: the same confirmation, and nothing is listed.
    let fd = open(NSHomeDirectory() + "/Desktop", O_RDONLY | O_DIRECTORY)
    if fd >= 0 {
        close(fd)
        print("desktop: allowed")
    } else {
        print(errno == ENOENT ? "desktop: missing" : "desktop: not allowed")
    }
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "--vd-check" { runVirtualDisplayCheck(Array(CommandLine.arguments.dropFirst(2))) }
let settings = readSettings()

// MARK: - Saying things

/// The clock istesso-media reads its times from (crates/istesso-media/src/clock.rs).
@inline(__always) func nowNs() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }

let sayLock = NSLock()

/// One line of JSON on standard output, with the key "istesso", as m1-step3-host.py says things.
func say(_ word: String, _ fields: [(String, Any?)] = []) {
    var line = "{\"istesso\":\"\(word)\""
    for (key, value) in fields {
        line += ",\"\(key)\":" + jsonValue(value)
    }
    line += "}\n"
    sayLock.lock()
    FileHandle.standardOutput.write(Data(line.utf8))
    sayLock.unlock()
}

func jsonValue(_ value: Any?) -> String {
    switch value {
    case nil: return "null"
    case let b as Bool: return b ? "true" : "false"
    case let i as Int: return String(i)
    case let i as UInt64: return String(i)
    case let i as UInt32: return String(i)
    case let d as Double: return String(format: "%.3f", d)
    case let s as String:
        let data = try? JSONSerialization.data(withJSONObject: [s], options: [.fragmentsAllowed])
        let text = data.flatMap { String(data: $0, encoding: .utf8) } ?? "[\"\"]"
        return String(text.dropFirst().dropLast())
    default: return "null"
    }
}

func note(_ text: String) {
    FileHandle.standardError.write(Data("daily-host: \(text)\n".utf8))
}

/// The frame log: one line of JSON for each thing that happens to a frame, with times on nowNs().
/// Written from several threads; each line is one write.
final class FrameLog {
    private let handle: FileHandle?
    private let lock = NSLock()
    init(path: String?) {
        guard let path else { handle = nil; return }
        if FileManager.default.fileExists(atPath: path) {
            note("the frame log \(path) exists already; nothing is written to it")
            handle = nil
            return
        }
        FileManager.default.createFile(atPath: path, contents: nil, attributes: [.posixPermissions: 0o600])
        handle = FileHandle(forWritingAtPath: path)
    }
    var on: Bool { handle != nil }
    func write(_ line: String) {
        guard let handle else { return }
        lock.lock()
        handle.write(Data((line + "\n").utf8))
        lock.unlock()
    }
}

let frameLog = FrameLog(path: settings.log)

// MARK: - The transport

/// Who is shown the picture: every paired device that may look, one device, or nobody.
enum AdmitState: Equatable {
    case paired
    case only(id: String, key: String)
    case nobody
}

/// The order in which a stream is started, fail closed: listen admitting nobody first (nothing is
/// called before), then, when it is not offered, stop offering, and only then say who may look.
/// The native calls are passed in, so that --self-test can check the order.
func startStream(admitting: AdmitState, offered: Bool, listenClosed: () -> Bool, offerNot: () -> Void,
                 admitPaired: () -> Void, admitOnly: (String, String) -> Void) {
    guard listenClosed() else { return }
    if !offered { offerNot() }
    switch admitting {
    case .nobody: break
    case .paired: admitPaired()
    case let .only(id, key): admitOnly(id, key)
    }
}

/// The admit state and the offer after the words that came during a restart, in order.
func stateAfter(words: [String], admit: AdmitState, offered: Bool, detached: Bool, held: Bool) -> (AdmitState, Bool, Bool) {
    var admit = admit, offered = offered, detached = detached
    for line in words {
        let w = line.split(separator: " ").map(String.init)
        switch w.first {
        case "admit": admit = w.count == 3 ? .only(id: w[1], key: w[2]) : .nobody
        case "detach": detached = true; offered = false
        case "resume" where !held: detached = false; offered = true
        default: break
        }
    }
    return (admit, offered, detached)
}

/// The native calls a stream may still take: none once it is closed. A small lock-held flag, so
/// that a frame on the encoder's thread and the close on the main thread never overlap.
final class Guarded {
    private let lock = NSLock()
    private var closed = false
    /// Runs `work` while not closed; nil once closed.
    func use<T>(_ work: () -> T) -> T? {
        lock.lock(); defer { lock.unlock() }
        return closed ? nil : work()
    }
    /// Closes; `last` runs once, under the lock, after every `use` in progress has finished.
    func close(_ last: () -> Void) {
        lock.lock(); defer { lock.unlock() }
        guard !closed else { return }
        closed = true
        last()
    }
    var isClosed: Bool { lock.lock(); defer { lock.unlock() }; return closed }
}

final class Transport {
    private let native: OpaquePointer
    private let guarded = Guarded()
    /// Fail closed: the host always starts admitting nobody, and is then told who is shown the
    /// picture (the remembered state), before anything else is done with it.
    init?(settings: Settings, width: Int, height: Int, admitting: AdmitState, offered: Bool = true) {
        var info = IstessoMediaStreamInfo(codec: 1, width: UInt32(width), height: UInt32(height),
                                          fps: UInt32(settings.fps), chroma: settings.chroma, colour: 1)
        let bind = "\(settings.bind):\(settings.port)"
        var made: OpaquePointer?
        startStream(admitting: admitting, offered: offered,
                    listenClosed: { made = istesso_media_host_listen_closed(settings.configDir, bind, &info); return made != nil },
                    offerNot: { istesso_media_host_offer(made, 0) },
                    admitPaired: { _ = istesso_media_host_admit_paired(made) },
                    admitOnly: { id, key in _ = istesso_media_host_admit(made, id, key) })
        guard let made else {
            let why = istesso_media_last_error().map { String(cString: $0) } ?? "unknown"
            note("cannot listen for paired devices: \(why)")
            return nil
        }
        native = made
    }
    var closed: Bool { guarded.isClosed }
    var viewers: Int { guarded.use { Int(istesso_media_host_viewers(native)) } ?? 0 }
    /// Ends every stream and stops listening; nothing reaches the native host afterwards.
    func close() { guarded.close { istesso_media_host_close(native) } }
    func keyFrameWanted() -> Bool { guarded.use { istesso_media_host_key_frame_wanted(native) != 0 } ?? false }
    func offer(_ on: Bool) { _ = guarded.use { istesso_media_host_offer(native, on ? 1 : 0) } }
    func admitPaired() { _ = guarded.use { istesso_media_host_admit_paired(native) } }
    func admit(_ id: String?, _ key: String?) -> Bool { guarded.use { istesso_media_host_admit(native, id, key) == 0 } ?? false }
    func send(_ data: [UInt8], pts: UInt64, encoded: UInt64, key: Bool) -> IstessoMediaSent? {
        guarded.use { () -> IstessoMediaSent? in
            var sent = IstessoMediaSent()
            let result = data.withUnsafeBufferPointer {
                istesso_media_host_send(native, $0.baseAddress, $0.count, pts, encoded, key ? 1 : 0, &sent)
            }
            return result == 0 ? sent : nil
        } ?? nil
    }
    /// Kind 5, the text input field (trial only; only with --inline).
    func setTextInput(_ field: FieldReport) {
        var made = IstessoMediaTextInput(state: field.state, x: field.x, y: field.y, width: field.width, height: field.height, serial: field.serial, dropped: 0)
        _ = guarded.use { istesso_media_host_set_text_input(native, &made) }
    }
    /// The viewers draw the Mac's own cursor where their pointer is: the picture has none, and the
    /// host's pointer does not move (input goes to the application's process, not to the screen).
    func ownCursor() {
        "default".withCString { name in
            var cursor = IstessoMediaCursor(shape: 1, width: 0, height: 0, hotspot_x: 0, hotspot_y: 0, scale: 1,
                                            pixels: nil, pixels_len: 0, name: name)
            _ = guarded.use { istesso_media_host_set_cursor(native, &cursor) }
        }
    }

    /// The cursor as an image the size it has in the picture (`pixelsPerPoint`: picture pixels for
    /// one point of the host's screen), so that the viewer, which scales it as it scales the
    /// picture, shows it as large as the host's screen looks inside its window.
    /// Says nothing when the image is the one told last (`told`: a hash of what was told).
    func setCursor(_ cursor: NSCursor, pixelsPerPoint k: Double, told: inout Int?) -> Bool {
        let image = cursor.image
        let width = max(1, Int((image.size.width * k).rounded())), height = max(1, Int((image.size.height * k).rounded()))
        guard width <= 256, height <= 256,
              let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
        image.draw(in: NSRect(x: 0, y: 0, width: width, height: height))
        NSGraphicsContext.restoreGraphicsState()
        guard let data = context.data else { return false }
        let pixels = UnsafeRawPointer(data).assumingMemoryBound(to: UInt8.self)
        let hot = cursor.hotSpot
        var hasher = Hasher()
        hasher.combine(Data(bytes: pixels, count: width * height * 4))
        hasher.combine(width); hasher.combine(Int(hot.x * k)); hasher.combine(Int(hot.y * k))
        let hash = hasher.finalize()
        if told == hash { return true }
        told = hash
        var made = IstessoMediaCursor(shape: 2, width: UInt32(width), height: UInt32(height),
                                      hotspot_x: UInt32(min(Double(width - 1), max(0, hot.x * k))),
                                      hotspot_y: UInt32(min(Double(height - 1), max(0, hot.y * k))), scale: 1,
                                      pixels: pixels, pixels_len: width * height * 4, name: nil)
        return guarded.use { istesso_media_host_set_cursor(native, &made) == 0 } ?? false
    }
}

// MARK: - The encoder

enum EncoderError: Error, CustomStringConvertible {
    case status(String, OSStatus)
    var description: String {
        switch self { case let .status(what, status): return "\(what) failed (\(status))" }
    }
}

/// H.265 from the Mac's own encoder, with the settings measured in docs/m4-plan.md, step 2: no
/// RealTime (three times slower on the Mac mini), no reordering, key frames only when asked,
/// BGRA handed in as it is.
final class Encoder {
    let session: VTCompressionSession
    let width: Int
    let height: Int
    private(set) var refused: [String] = []

    init(width: Int, height: Int, settings: Settings) throws {
        self.width = width
        self.height = height
        let spec = [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: kCFBooleanTrue] as CFDictionary
        var made: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_HEVC,
            encoderSpecification: spec, imageBufferAttributes: nil, compressedDataAllocator: nil,
            outputCallback: nil, refcon: nil, compressionSessionOut: &made)
        guard status == 0, let made else { throw EncoderError.status("VTCompressionSessionCreate", status) }
        session = made
        // No constant for 4:4:4 in the public headers; the hardware encoder lists it under this name.
        let profile: CFString = settings.chroma == 444 ? "HEVC_Main444_AutoLevel" as CFString
            : kVTProfileLevel_HEVC_Main_AutoLevel
        let profileStatus = VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ProfileLevel, value: profile)
        guard profileStatus == 0 else {
            VTCompressionSessionInvalidate(session)
            throw EncoderError.status("setting the profile", profileStatus)
        }
        let bps = settings.bitrateKbit * 1000
        var wanted: [(String, CFString, CFTypeRef)] = [
            ("AllowFrameReordering", kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse),
            ("MaxKeyFrameInterval", kVTCompressionPropertyKey_MaxKeyFrameInterval, Int32.max as CFNumber),
            ("ExpectedFrameRate", kVTCompressionPropertyKey_ExpectedFrameRate, settings.fps as CFNumber),
            ("AverageBitRate", kVTCompressionPropertyKey_AverageBitRate, bps as CFNumber),
            ("DataRateLimits", kVTCompressionPropertyKey_DataRateLimits, [bps / 8, 1.0] as CFArray),
            ("PrioritizeEncodingSpeedOverQuality", kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanTrue),
            ("ColorPrimaries", kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2),
            ("TransferFunction", kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_sRGB),
            ("YCbCrMatrix", kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2),
        ]
        if settings.minQP > 0 {
            wanted.append(("MinAllowedFrameQP", kVTCompressionPropertyKey_MinAllowedFrameQP, settings.minQP as CFNumber))
        }
        for (name, key, value) in wanted where VTSessionSetProperty(session, key: key, value: value) != 0 {
            refused.append(name)
        }
        VTCompressionSessionPrepareToEncodeFrames(session)
    }

    deinit { VTCompressionSessionInvalidate(session) }

    /// Hands one picture over. `done` is called on the encoder's thread with the frame as the
    /// transport carries it (Annex B, parameter sets before a key frame), or nil.
    func encode(_ buffer: CVPixelBuffer, ptsNs: UInt64, key: Bool,
                done: @escaping (_ frame: [UInt8]?, _ key: Bool, _ status: OSStatus) -> Void) -> OSStatus {
        let pts = CMTime(value: CMTimeValue(ptsNs), timescale: 1_000_000_000)
        let options = key ? [kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanTrue] as CFDictionary : nil
        return VTCompressionSessionEncodeFrame(session, imageBuffer: buffer, presentationTimeStamp: pts,
                                               duration: .invalid, frameProperties: options, infoFlagsOut: nil) { status, _, sample in
            guard status == 0, let sample, let (frame, isKey) = annexB(sample) else {
                done(nil, false, status)
                return
            }
            done(frame, isKey, 0)
        }
    }
}

/// From the encoder's sample (NAL units behind their lengths) to an access unit behind start codes,
/// with the parameter sets (VPS, SPS, PPS) before a key frame.
func annexB(_ sample: CMSampleBuffer) -> ([UInt8], Bool)? {
    guard let block = CMSampleBufferGetDataBuffer(sample) else { return nil }
    var key = true
    if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]],
       let notSync = attachments.first?[kCMSampleAttachmentKey_NotSync] as? Bool, notSync {
        key = false
    }
    let start: [UInt8] = [0, 0, 0, 1]
    var out: [UInt8] = []
    var lengthSize: Int32 = 4
    if let format = CMSampleBufferGetFormatDescription(sample) {
        var count = 0
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                           parameterSetSizeOut: nil, parameterSetCountOut: &count,
                                                           nalUnitHeaderLengthOut: &lengthSize)
        if key {
            for index in 0..<count {
                var pointer: UnsafePointer<UInt8>?
                var size = 0
                if CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: index, parameterSetPointerOut: &pointer,
                                                                      parameterSetSizeOut: &size, parameterSetCountOut: nil,
                                                                      nalUnitHeaderLengthOut: nil) == 0, let pointer {
                    out += start
                    out.append(contentsOf: UnsafeBufferPointer(start: pointer, count: size))
                }
            }
        }
    }
    let total = CMBlockBufferGetDataLength(block)
    var bytes = [UInt8](repeating: 0, count: total)
    guard CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: total, destination: &bytes) == 0 else { return nil }
    let size = Int(lengthSize)
    guard size >= 1 && size <= 4 else { return nil }
    out.reserveCapacity(out.count + total + 16)
    var at = 0
    while at + size <= total {
        var length = 0
        for i in 0..<size { length = length << 8 | Int(bytes[at + i]) }
        at += size
        guard length > 0, at + length <= total else { return nil }
        out += start
        out.append(contentsOf: bytes[at..<at + length])
        at += length
    }
    return (out, key)
}

// MARK: - The pump: pictures in, frames out

/// Everything between a picture and the transport, on one serial queue. The newest picture wins:
/// while the encoder still has a frame, a new picture is kept, not queued, and goes in as soon as
/// the encoder is free (no picture waits behind another).
final class Pump {
    let queue = DispatchQueue(label: "istesso.daily-host.pump", qos: .userInteractive)
    let transport: Transport
    let width: Int
    let height: Int
    private var encoder: Encoder?
    private var last: CVPixelBuffer?
    private var lastPts: UInt64 = 0
    private var fresh = false
    private var keyWanted = true
    private var inFlight = false
    private var lastEncodeNs: UInt64 = 0
    /// One more for every encoder opened: a frame of an encoder that was given back is not taken.
    private var generation = 0
    /// The generation that may send now (0: none), for the encoder's own thread.
    private let liveLock = NSLock()
    private var live = 0
    private func setLive(_ value: Int) { liveLock.lock(); live = value; liveLock.unlock() }
    private func isLive(_ value: Int) -> Bool { liveLock.lock(); defer { liveLock.unlock() }; return live == value }
    private(set) var running = false
    // Counts for the line a second.
    private var captured = 0, encoded = 0, dropped = 0, sentFrames = 0, fillers = 0, failed = 0
    private var encodeMs: [Double] = []
    // The answer to `resume`, once the first frame is out.
    private var resumeAsked: UInt64?
    private var resumeOpened: UInt64 = 0

    init(transport: Transport, width: Int, height: Int) {
        self.transport = transport
        self.width = width
        self.height = height
    }

    /// Opens the encoder (on the pump's queue).
    func open() -> String? {
        do {
            let made = try Encoder(width: width, height: height, settings: settings)
            if !made.refused.isEmpty { note("the encoder refused: \(made.refused.joined(separator: ", "))") }
            generation += 1
            encoder = made
            running = true
            keyWanted = true
            inFlight = false
            fresh = false
            lastEncodeNs = 0
            setLive(generation)
            return nil
        } catch {
            running = false
            encoder = nil
            return "\(error)"
        }
    }

    /// Gives the encoder back (on the pump's queue).
    func close() {
        running = false
        generation += 1
        setLive(0)
        encoder = nil
        inFlight = false
        last = nil
        fresh = false
    }

    /// A picture from the capture (on the pump's queue). `displayNs` is when it was on the screen.
    func picture(_ buffer: CVPixelBuffer, displayNs: UInt64) {
        guard running else { return }
        captured += 1
        let arrived = nowNs()
        if frameLog.on {
            frameLog.write("{\"kind\":\"frame_out\",\"pts\":\(displayNs),\"t_ns\":\(arrived)}")
        }
        if fresh { dropped += 1 }  // the one kept before was never encoded
        last = buffer
        // Never backwards: a filler took the time it was made.
        lastPts = max(displayNs, lastPts + 1)
        fresh = true
        if !inFlight { encodeLast(filler: false) }
    }

    /// Looked at every few milliseconds: key frames asked for, and keeping the encoder awake
    /// (docs/m4-plan.md, step 2: 20 pictures a second while somebody watches).
    func tick() {
        if transport.keyFrameWanted() { keyWanted = true }
        guard running, !inFlight, last != nil, transport.viewers > 0 || resumeAsked != nil else { return }
        let now = nowNs()
        let awake = settings.keepAwake > 0 ? UInt64(1_000_000_000 / settings.keepAwake) : UInt64.max
        if fresh {
            encodeLast(filler: false)
        } else if keyWanted || now - lastEncodeNs >= awake {
            lastPts = max(now, lastPts + 1)
            encodeLast(filler: true)
        }
    }

    private func encodeLast(filler: Bool) {
        guard let encoder, let buffer = last else { return }
        // Nobody watches and nobody waits for a resumed picture: nothing is encoded.
        if transport.viewers == 0 && resumeAsked == nil {
            fresh = false
            return
        }
        let key = keyWanted
        keyWanted = false
        fresh = false
        inFlight = true
        if filler { fillers += 1 }
        let pts = lastPts
        let mine = generation
        let handed = nowNs()
        lastEncodeNs = handed
        let status = encoder.encode(buffer, ptsNs: pts, key: key) { [weak self] frame, isKey, status in
            let done = nowNs()
            guard let self else { return }
            var sent: IstessoMediaSent?
            // A frame of an encoder that has been given back since goes nowhere.
            if let frame, self.isLive(mine) {
                sent = self.transport.send(frame, pts: pts, encoded: done, key: isKey)
            }
            let written = nowNs()
            if frameLog.on, let frame {
                frameLog.write("{\"kind\":\"encoded\",\"pts\":\(pts),\"t_ns\":\(done),\"handed_ns\":\(handed),\"bytes\":\(frame.count),\"key\":\(isKey ? 1 : 0),\"filler\":\(filler ? 1 : 0)}")
                if let sent {
                    frameLog.write("{\"kind\":\"sent\",\"pts\":\(pts),\"t_ns\":\(done),\"rtp\":\(sent.seq),\"packets\":\(sent.messages),\"written_ns\":\(written),\"viewers\":\(sent.viewers),\"queued\":\(sent.queued),\"bytes\":\(frame.count)}")
                }
            }
            self.queue.async {
                guard self.generation == mine, self.running else { return }
                self.inFlight = false
                if frame == nil || sent == nil {
                    self.failed += 1
                    self.keyWanted = true
                    if status == kVTInvalidSessionErr { self.reopen() }
                } else {
                    self.encoded += 1
                    if sent != nil && (sent?.viewers ?? 0) > 0 { self.sentFrames += 1 }
                    self.encodeMs.append(Double(done - handed) / 1e6)
                    if let asked = self.resumeAsked {
                        self.resumeAsked = nil
                        say("resumed", [("opened_ms", Double(self.resumeOpened - asked) / 1e6),
                                        ("encoded_ms", Double(done - asked) / 1e6),
                                        ("written_ms", Double(written - asked) / 1e6), ("key", isKey ? 1 : 0)])
                    }
                }
                if self.fresh { self.encodeLast(filler: false) }
            }
        }
        if status != 0 {
            inFlight = false
            failed += 1
            keyWanted = true
            if status == kVTInvalidSessionErr { reopen() }
        }
    }

    /// The encoder went away underneath (after the Mac slept, for one): a new one.
    private func reopen() {
        guard running else { return }
        note("the encoder's session became invalid; opening a new one")
        let keep = last
        if let why = open() {
            note("no new encoder: \(why)")
            say("refused", [("word", "encode"), ("reason", "no encoder could be opened again: \(why); the picture stays stopped")])
        }
        last = keep
    }

    func resumed(asked: UInt64) {
        resumeAsked = asked
        resumeOpened = nowNs()
    }

    /// The line a second (on the pump's queue).
    func second() {
        encodeMs.sort()
        let median = encodeMs.isEmpty ? 0 : encodeMs[encodeMs.count / 2]
        let worst = encodeMs.last ?? 0
        if frameLog.on {
            frameLog.write("{\"kind\":\"second\",\"t_ns\":\(nowNs()),\"captured\":\(captured),\"encoded\":\(encoded),\"fillers\":\(fillers),\"dropped\":\(dropped),\"failed\":\(failed),\"sent\":\(sentFrames),\"viewers\":\(transport.viewers),\"encode_median_ms\":\(String(format: "%.2f", median)),\"encode_max_ms\":\(String(format: "%.2f", worst))}")
        }
        captured = 0; encoded = 0; dropped = 0; sentFrames = 0; fillers = 0; failed = 0
        encodeMs.removeAll(keepingCapacity: true)
    }
}

// MARK: - Where the picture comes from

/// A moving picture of its own, for checks without the Screen Recording permission.
final class Generated {
    private var pool: CVPixelBufferPool?
    private var timer: DispatchSourceTimer?
    private var frame = 0
    let width: Int, height: Int

    init(width: Int, height: Int) {
        self.width = width
        self.height = height
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey: width, kCVPixelBufferHeightKey: height,
            kCVPixelBufferIOSurfacePropertiesKey: [CFString: Any](),
        ]
        CVPixelBufferPoolCreate(nil, nil, attributes as CFDictionary, &pool)
    }

    func start(on queue: DispatchQueue, fps: Int, each: @escaping (CVPixelBuffer, UInt64) -> Void) {
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        timer.schedule(deadline: .now(), repeating: .nanoseconds(1_000_000_000 / fps), leeway: .microseconds(200))
        timer.setEventHandler { [weak self] in
            guard let self, let pool = self.pool else { return }
            var made: CVPixelBuffer?
            guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &made) == 0, let buffer = made else { return }
            self.draw(buffer)
            each(buffer, nowNs())
        }
        timer.resume()
        self.timer = timer
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }

    private func draw(_ buffer: CVPixelBuffer) {
        frame += 1
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        let stride = CVPixelBufferGetBytesPerRow(buffer)
        let bar = (frame * 8) % width
        for y in 0..<height {
            let row = base.advanced(by: y * stride).assumingMemoryBound(to: UInt32.self)
            let shade: UInt32 = UInt32(40 + (y * 120 / height))
            let background = 0xff00_0000 | shade << 16 | shade << 8 | shade
            for x in 0..<width { row[x] = background }
            for x in max(0, bar - 12)..<min(width, bar + 12) { row[x] = 0xffff_ffff }
        }
    }
}

/// ScreenCaptureKit: one stream, its pictures handed to the pump's queue.
final class Capture: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private let queue: DispatchQueue
    private let each: (CVPixelBuffer, UInt64) -> Void
    var onStop: ((String) -> Void)?
    private var timebase = mach_timebase_info_data_t()

    init(queue: DispatchQueue, each: @escaping (CVPixelBuffer, UInt64) -> Void) {
        self.queue = queue
        self.each = each
        mach_timebase_info(&timebase)
    }

    static func configuration(width: Int, height: Int, sourceRect: CGRect?) -> SCStreamConfiguration {
        let configuration = SCStreamConfiguration()
        configuration.width = width
        configuration.height = height
        configuration.pixelFormat = kCVPixelFormatType_32BGRA
        configuration.colorSpaceName = CGColorSpace.sRGB
        configuration.showsCursor = false
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(settings.fps))
        // Few pictures on hand: a picture that waits is a picture late.
        configuration.queueDepth = 4
        configuration.scalesToFit = true
        if let sourceRect { configuration.sourceRect = sourceRect }
        configuration.backgroundColor = .black
        return configuration
    }

    func start(filter: SCContentFilter, configuration: SCStreamConfiguration, done: @escaping (String?) -> Void) {
        let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
        do {
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        } catch {
            return done("the capture would not take an output: \(error.localizedDescription)")
        }
        self.stream = stream
        stream.startCapture { error in done(error?.localizedDescription) }
    }

    func update(_ configuration: SCStreamConfiguration) {
        stream?.updateConfiguration(configuration) { error in
            if let error { note("the capture did not take the new place: \(error.localizedDescription)") }
        }
    }

    func update(_ filter: SCContentFilter) {
        stream?.updateContentFilter(filter) { error in
            if let error { note("the capture did not take the new filter: \(error.localizedDescription)") }
        }
    }

    func stop(done: @escaping () -> Void) {
        guard let stream else { return done() }
        self.stream = nil
        stream.stopCapture { _ in done() }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = attachments.first?[.status] as? Int, let status = SCFrameStatus(rawValue: raw), status != .complete {
            return
        }
        var displayNs = nowNs()
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let ticks = attachments.first?[.displayTime] as? UInt64 {
            displayNs = ticks * UInt64(timebase.numer) / UInt64(timebase.denom)
        }
        each(buffer, displayNs)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        onStop?(error.localizedDescription)
    }
}

// MARK: - The application and its windows

/// Where a virtual display goes in the arrangement: touching the main display only at its
/// bottom right corner (diagonally), so that the pointer does not wander onto it from an edge.
func cornerOrigin(main: CGRect) -> CGPoint { CGPoint(x: main.maxX, y: main.maxY) }

/// The place of the virtual display of slot N: in a row from that corner, each the receiver's
/// width apart (slot 1 at the corner itself; the others touch only the one before them).
func rowOrigin(main: CGRect, width: Double, slot: Int) -> CGPoint {
    let corner = cornerOrigin(main: main)
    return CGPoint(x: corner.x + Double(max(0, slot - 1)) * width, y: corner.y)
}

/// A window's place made to fit inside the visible area (kept as large as fits, moved inside).
func fitInto(_ frame: CGRect, _ visible: CGRect) -> CGRect {
    let w = min(frame.width, visible.width), h = min(frame.height, visible.height)
    let x = min(max(frame.minX, visible.minX), visible.maxX - w)
    let y = min(max(frame.minY, visible.minY), visible.maxY - h)
    return CGRect(x: x, y: y, width: w, height: h)
}

/// Whether a place is on some display now (its middle).
func onSomeDisplay(_ frame: CGRect, displays: [CGRect]) -> Bool {
    displays.contains { $0.contains(CGPoint(x: frame.midX, y: frame.midY)) }
}

/// The density of the display a place lies on (its middle); the main display's otherwise.
func densityAt(_ frame: CGRect) -> Double {
    var count: UInt32 = 0
    var ids = [CGDirectDisplayID](repeating: 0, count: 16)
    CGGetActiveDisplayList(16, &ids, &count)
    let id = ids.prefix(Int(count)).first { CGDisplayBounds($0).contains(CGPoint(x: frame.midX, y: frame.midY)) } ?? CGMainDisplayID()
    return pixelsPerPoint(id)
}

/// Pixels of a display's current mode for one of its points (2 on a Retina display).
func pixelsPerPoint(_ display: CGDirectDisplayID) -> Double {
    guard let mode = CGDisplayCopyDisplayMode(display), mode.width > 0 else { return 1 }
    return Double(mode.pixelWidth) / Double(mode.width)
}

struct WindowInfo {
    let id: CGWindowID
    let pid: pid_t
    let layer: Int
    let bounds: CGRect  // global, points, from the top left of the main display
    let alpha: Double
    let onScreen: Bool
}

/// The windows on the screen, front to back.
/// The on-screen list, kept for a few milliseconds: one input event asks for it several times, and
/// each call costs about half a millisecond on the main thread.
private var listCache: (at: UInt64, list: [WindowInfo])?
func windowList(onScreenOnly: Bool = true, desktop: Bool = false) -> [WindowInfo] {
    if onScreenOnly && !desktop, let cache = listCache, nowNs() - cache.at < 8_000_000 { return cache.list }
    let list = readWindowList(onScreenOnly: onScreenOnly, desktop: desktop)
    if onScreenOnly && !desktop { listCache = (nowNs(), list) }
    return list
}

/// Right before a press or a scroll: the next look at the window list is a fresh one (one call,
/// answered by WindowServer, which waits for no application).
func freshWindowList() { listCache = nil }

func systemPromptCount(_ windows: [NSDictionary], owners: Set<String>,
                       minLayer: (String) -> Int = { systemPromptMinLayers[$0] ?? systemPromptMinLayer },
                       bundleID: (pid_t) -> String?) -> Int {
    windows.reduce(into: 0) { count, window in
        guard let layer = window[kCGWindowLayer as String] as? Int,
              let rawPID = window[kCGWindowOwnerPID as String] as? Int, let pid = pid_t(exactly: rawPID),
              let owner = bundleID(pid), owners.contains(owner), layer >= minLayer(owner) else { return }
        count += 1
    }
}

struct SystemPromptState {
    private var present = false
    private var previous: Bool?

    /// An unreadable list breaks the streak. Only a confirmed change produces a log entry.
    mutating func observe(_ sample: Bool?) -> Bool? {
        defer { previous = sample }
        guard let sample, sample == previous, sample != present else { return nil }
        present = sample
        return present
    }
}

final class SystemPromptMonitor {
    private let queue = DispatchQueue(label: "istesso.daily-host.system-prompt", qos: .utility)
    private let sample: () -> Bool?
    private var state = SystemPromptState() // Accessed only on the timer's queue.
    private var timer: DispatchSourceTimer? // Started and stopped on main.

    init(sample: @escaping () -> Bool?) { self.sample = sample }

    deinit { timer?.cancel() }

    func start() {
        guard timer == nil else { return }
        let source = DispatchSource.makeTimerSource(queue: queue)
        source.schedule(deadline: .now() + 1)
        source.setEventHandler { [weak self] in
            guard let self, !source.isCancelled else { return }
            let present = self.sample()
            // A measurement can start while WindowServer is answering.
            guard !source.isCancelled else { return }
            if let changed = self.state.observe(present) {
                say("system-prompt", [("present", changed)])
            }
            source.schedule(deadline: .now() + 1)
        }
        timer = source
        source.resume()
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }
}

func readWindowList(onScreenOnly: Bool, desktop: Bool) -> [WindowInfo] {
    var options: CGWindowListOption = onScreenOnly ? [.optionOnScreenOnly] : [.optionAll]
    if !desktop { options.insert(.excludeDesktopElements) }
    guard let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else { return [] }
    return list.compactMap { entry in
        guard let number = entry[kCGWindowNumber as String] as? Int,
              let pid = entry[kCGWindowOwnerPID as String] as? Int,
              let layer = entry[kCGWindowLayer as String] as? Int,
              let boundsDict = entry[kCGWindowBounds as String] as? NSDictionary,
              let bounds = CGRect(dictionaryRepresentation: boundsDict) else { return nil }
        let alpha = entry[kCGWindowAlpha as String] as? Double ?? 1
        let onScreen = entry[kCGWindowIsOnscreen as String] as? Bool ?? false
        return WindowInfo(id: CGWindowID(number), pid: pid_t(pid), layer: layer, bounds: bounds, alpha: alpha, onScreen: onScreen)
    }
}

/// Finds an application by what the person called it: a path to a bundle, a bundle id, or a name
/// in the usual folders.
func applicationURL(_ name: String) -> URL? {
    if name.hasSuffix(".app") || name.contains("/") {
        let url = URL(fileURLWithPath: (name as NSString).expandingTildeInPath)
        return FileManager.default.fileExists(atPath: url.path) ? url : nil
    }
    if name.contains("."), let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: name) {
        return url
    }
    for folder in ["/Applications", "/System/Applications", "/System/Applications/Utilities",
                   NSHomeDirectory() + "/Applications", "/Applications/Utilities"] {
        let url = URL(fileURLWithPath: "\(folder)/\(name).app")
        if FileManager.default.fileExists(atPath: url.path) { return url }
    }
    return nil
}

// MARK: - Input

/// Moves and sizes the application's window that is at `matching` (Accessibility; on the
/// lookout's queue, never on the input's).
func setWindowFrame(_ pid: pid_t, matching: CGRect, to target: CGRect) {
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { return }
    for window in windows {
        var position: CFTypeRef?, extent: CFTypeRef?
        AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position)
        AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent)
        var p = CGPoint.zero, s = CGSize.zero
        if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
        if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &s) }
        guard abs(p.x - matching.minX) < 2, abs(p.y - matching.minY) < 2,
              abs(s.width - matching.width) < 2, abs(s.height - matching.height) < 2 else { continue }
        var origin = target.origin, size = target.size
        if let made = AXValueCreate(.cgPoint, &origin) { AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, made) }
        if let made = AXValueCreate(.cgSize, &size) { AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, made) }
        return
    }
}

/// The places of the windows Accessibility lists for an application (global points, top left).
struct AXWindowInfo {
    let bounds: CGRect
    let role: String
    let subrole: String
}

func axWindows(_ pid: pid_t) -> [AXWindowInfo] {
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { return [] }
    return windows.compactMap { window in
        var position: CFTypeRef?, extent: CFTypeRef?
        guard AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position) == .success,
              AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent) == .success,
              let position, let extent else { return nil }
        var p = CGPoint.zero, s = CGSize.zero
        AXValueGetValue(position as! AXValue, .cgPoint, &p)
        AXValueGetValue(extent as! AXValue, .cgSize, &s)
        var role: CFTypeRef?, subrole: CFTypeRef?
        AXUIElementCopyAttributeValue(window, kAXRoleAttribute as CFString, &role)
        AXUIElementCopyAttributeValue(window, kAXSubroleAttribute as CFString, &subrole)
        return AXWindowInfo(bounds: CGRect(origin: p, size: s), role: role as? String ?? "", subrole: subrole as? String ?? "")
    }
}

/// The system's process that shows an application's open and save panel.
func isPanelService(_ pid: pid_t) -> Bool {
    NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.apple.appkit.xpc.openAndSavePanelService"
}

/// Whether a window of the system's open and save panel service is the application's own panel:
/// only while the application is in front, and only when the application itself shows a window
/// with the very same place and size (Accessibility lists the panel among the application's
/// windows). A panel the service shows for another application has no such twin.
///
/// The twin must be a sheet or a dialog (a standard window of the application with the same
/// place and size does not count). Which process the service works for cannot be asked through a
/// public interface (the "responsible process" calls are private), so the panel is tied to the
/// application through its own Accessibility tree.
func panelAllowed(serviceWindow: CGRect, appWindows: [AXWindowInfo], appInFront: Bool) -> Bool {
    guard appInFront else { return false }
    return appWindows.contains { twin in
        let kind = twin.role == (kAXSheetRole as String)
            || twin.subrole == (kAXDialogSubrole as String) || twin.subrole == (kAXSystemDialogSubrole as String)
        let b = twin.bounds
        return kind && abs(b.minX - serviceWindow.minX) < 2 && abs(b.minY - serviceWindow.minY) < 2
            && abs(b.width - serviceWindow.width) < 2 && abs(b.height - serviceWindow.height) < 2
    }
}

/// The windows the capture names while the application's panel shows: all of the application's
/// own, and of the panel service only those allowed. Any other window of the service is left out,
/// also one that came after the allowed ones were found.
func capturedWindows(appWindows: [CGWindowID], serviceWindows: [CGWindowID], allowed: Set<CGWindowID>) -> [CGWindowID] {
    appWindows + serviceWindows.filter { allowed.contains($0) }
}

/// What to do with one pointer event of the session form (form 4).
enum SessionStep: Equatable {
    enum Remember: Equatable { case press(CGRect), release, nothing }
    /// Post it through the session, at this place.
    case post(CGPoint, Remember)
    /// Post it to the application's process only (the Mac's pointer does not move).
    case toProcess
    case drop
}

func clamp(_ point: CGPoint, into rect: CGRect) -> CGPoint {
    CGPoint(x: min(max(point.x, rect.minX + 1), rect.maxX - 1), y: min(max(point.y, rect.minY + 1), rect.maxY - 1))
}

/// The rule of the session form. `onTarget`: the window on top at the point is the application's
/// own. `pressWindow`: the bounds of the application's window this button was pressed in through
/// the session, if it was. A press or a scroll goes through only onto the application; a move only
/// over it (else to the process); a drag and a release only after a posted press, kept inside the
/// window of the press.
func sessionStep(type: CGEventType, at point: CGPoint, onTarget: Bool, pressWindow: CGRect?, window: CGRect) -> SessionStep {
    switch type {
    case .leftMouseDown, .rightMouseDown, .otherMouseDown:
        return onTarget && pressWindow == nil ? .post(point, .press(window)) : .drop
    case .leftMouseUp, .rightMouseUp, .otherMouseUp:
        guard let pressWindow else { return .drop }
        return .post(clamp(point, into: pressWindow), .release)
    case .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
        guard let pressWindow else { return .toProcess }
        return .post(clamp(point, into: pressWindow), .nothing)
    case .mouseMoved:
        return onTarget ? .post(point, .nothing) : .toProcess
    case .scrollWheel:
        return onTarget ? .post(point, .nothing) : .drop
    default:
        return .toProcess
    }
}

/// `--close-document PID PATH`: closes the one window of the application PID that shows the trial's
/// own document PATH, by pressing its close button through Accessibility; no other window and no
/// other document is touched. Says what became of it (one line of JSON) and ends. Never says the
/// document's contents or any window's title.
func closeDocument(_ arguments: [String]) -> Never {
    guard arguments.count == 2, let pid = pid_t(arguments[0]) else {
        print("{\"closed\":false,\"why\":\"usage: --close-document PID PATH\"}")
        exit(2)
    }
    let wanted = URL(fileURLWithPath: (arguments[1] as NSString).expandingTildeInPath).standardizedFileURL
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXIsProcessTrusted(), AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else {
        print("{\"closed\":false,\"why\":\"the application's windows cannot be read\"}")
        exit(1)
    }
    for window in windows {
        var document: CFTypeRef?
        guard AXUIElementCopyAttributeValue(window, kAXDocumentAttribute as CFString, &document) == .success,
              let text = document as? String, let url = URL(string: text)?.standardizedFileURL,
              url.path == wanted.path else { continue }
        var button: CFTypeRef?
        guard AXUIElementCopyAttributeValue(window, kAXCloseButtonAttribute as CFString, &button) == .success,
              let button, AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString) == .success else {
            print("{\"closed\":false,\"why\":\"its close button could not be pressed\"}")
            exit(1)
        }
        Thread.sleep(forTimeInterval: 1.0)
        var sheets: CFTypeRef?
        AXUIElementCopyAttributeValue(window, kAXChildrenAttribute as CFString, &sheets)
        let asking = (sheets as? [AXUIElement] ?? []).contains { child in
            var role: CFTypeRef?
            AXUIElementCopyAttributeValue(child, kAXRoleAttribute as CFString, &role)
            return role as? String == kAXSheetRole as String
        }
        print(asking ? "{\"closed\":false,\"why\":\"the application asks in a sheet (left as it is)\"}" : "{\"closed\":true}")
        exit(asking ? 1 : 0)
    }
    print("{\"closed\":false,\"why\":\"no window shows that document\"}")
    exit(1)
}

// MARK: - The virtual display for the application form (trial only; the owner's plan A)

/// One virtual display at 2 pixels a point, of the receiver's screen's size, placed touching the
/// main display at its bottom right corner only. It lives as long as this object (and this
/// program: when the program ends, macOS takes it away).
final class VirtualScreen {
    private var display: CGVirtualDisplay?
    let size: CGSize
    let hertz: Double
    let slot: Int
    init(width: Int, height: Int, hertz: Double, slot: Int = 1) {
        size = CGSize(width: width, height: height)
        self.hertz = hertz
        self.slot = slot
    }
    var id: CGDirectDisplayID? { display?.displayID }
    var frame: CGRect? { id.map(CGDisplayBounds) }

    /// Whether this macOS has the (private, weakly imported) classes at all.
    static var available: Bool { NSClassFromString("CGVirtualDisplay") != nil && NSClassFromString("CGVirtualDisplayDescriptor") != nil }

    /// Makes it; `done` gets its frame (global points) once it is among the screens, or nil.
    func make(done: @escaping (CGRect?) -> Void) {
        guard VirtualScreen.available else { return done(nil) }
        let descriptor = CGVirtualDisplayDescriptor()
        descriptor.name = "Istesso trial (Dev)"
        descriptor.maxPixelsWide = UInt32(size.width * 2)
        descriptor.maxPixelsHigh = UInt32(size.height * 2)
        descriptor.sizeInMillimeters = CGSize(width: size.width / 110 * 25.4, height: size.height / 110 * 25.4)
        descriptor.vendorID = 0x1234
        descriptor.productID = 0x0003
        // One per slot: two virtual displays with the same serial number are not both made.
        descriptor.serialNum = UInt32(slot)
        descriptor.redPrimary = CGPoint(x: 0.64, y: 0.33)
        descriptor.greenPrimary = CGPoint(x: 0.30, y: 0.60)
        descriptor.bluePrimary = CGPoint(x: 0.15, y: 0.06)
        descriptor.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
        descriptor.queue = DispatchQueue.main
        guard let made = CGVirtualDisplay(descriptor: descriptor) else { return done(nil) }
        let settings = CGVirtualDisplaySettings()
        settings.hiDPI = 1
        settings.modes = [CGVirtualDisplayMode(width: UInt32(size.width), height: UInt32(size.height), refreshRate: hertz),
                          CGVirtualDisplayMode(width: UInt32(size.width * 2), height: UInt32(size.height * 2), refreshRate: hertz)]
        guard made.apply(settings) else { return done(nil) }
        display = made
        let id = made.displayID
        var tries = 0
        Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { timer in
            tries += 1
            let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
            let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
            let pointsWide: Int = Int(self.size.width)
            let pixelsWide: Int = pointsWide * 2
            if let hidpi = modes.first(where: { (mode: CGDisplayMode) -> Bool in mode.width == pointsWide && mode.pixelWidth == pixelsWide }) {
                timer.invalidate()
                CGDisplaySetDisplayMode(id, hidpi, nil)
                // Diagonal to the main display: for this program only (macOS undoes it when it ends).
                var config: CGDisplayConfigRef?
                if CGBeginDisplayConfiguration(&config) == .success, let config {
                    let mainBounds: CGRect = CGDisplayBounds(CGMainDisplayID())
                    let origin: CGPoint = rowOrigin(main: mainBounds, width: Double(self.size.width), slot: self.slot)
                    CGConfigureDisplayOrigin(config, id, Int32(origin.x), Int32(origin.y))
                    CGCompleteDisplayConfiguration(config, .forAppOnly)
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { done(CGDisplayBounds(id)) }
            } else if tries > 50 {
                timer.invalidate()
                done(nil)
            }
        }
    }

    func remove() { display = nil }
}

// MARK: - The virtual display check (trial only; the owner's plan A, 2026-11-19)

/// `--vd-check WxH@HZ`: makes a virtual display through the private CGVirtualDisplay classes
/// (virtual-display.h: declared from the Objective-C runtime's own description, no other
/// project's code), asks for W x H points at HZ with HiDPI (2 pixels a point), puts one window of
/// this program on it that changes every frame, and reports, as one line of JSON each:
/// the modes the display offers and the one it took (points, pixels, refresh rate); how many
/// pictures a second the window drew and ScreenCaptureKit captured, and their size in pixels;
/// where a click posted through the session at a known place landed in the window. Then it takes
/// the display away and ends (about 20 s). Nothing of anybody else's is moved; the Mac's pointer
/// goes onto the virtual display for the click and is put back where it was.
func runVirtualDisplayCheck(_ arguments: [String]) -> Never {
    let parts = (arguments.first ?? "1720x720@120").split(whereSeparator: { $0 == "x" || $0 == "@" }).compactMap { Double($0) }
    guard parts.count == 3 else { print("{\"vd\":\"usage: --vd-check WxH@HZ\"}"); exit(2) }
    guard VirtualScreen.available else { print("{\"vd\":\"this macOS has no virtual display classes\"}"); exit(1) }
    let check = VirtualDisplayCheck(width: Int(parts[0]), height: Int(parts[1]), hertz: parts[2])
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    application.delegate = check
    application.run()
    exit(0)
}

final class AnimatedView: NSView {
    var frames = 0
    var clicks: [NSPoint] = []
    var link: CADisplayLink?
    override func draw(_ dirtyRect: NSRect) {
        let shade = CGFloat(frames % 60) / 60
        NSColor(calibratedRed: shade, green: 1 - shade, blue: 0.5, alpha: 1).setFill()
        bounds.fill()
        NSColor.black.setFill()
        NSRect(x: CGFloat(frames * 7 % Int(max(1, bounds.width))), y: 0, width: 4, height: bounds.height).fill()
    }
    @objc func tick(_ link: CADisplayLink) {
        frames += 1
        needsDisplay = true
    }
    override func mouseDown(with event: NSEvent) {
        let inView = convert(event.locationInWindow, from: nil)
        clicks.append(NSPoint(x: inView.x, y: bounds.height - inView.y))
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

final class VirtualDisplayCheck: NSObject, NSApplicationDelegate, SCStreamOutput {
    let width: Int, height: Int, hertz: Double
    var display: CGVirtualDisplay?
    var window: NSWindow?
    var view: AnimatedView?
    var stream: SCStream?
    var captured = 0
    var capturedSize = (0, 0)
    let queue = DispatchQueue(label: "istesso.vd-check.capture")
    init(width: Int, height: Int, hertz: Double) { self.width = width; self.height = height; self.hertz = hertz }

    func say(_ line: String) { print(line); fflush(stdout) }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard CGPreflightScreenCaptureAccess() else { say("{\"vd\":\"no screen recording\"}"); exit(3) }
        let descriptor = CGVirtualDisplayDescriptor()
        descriptor.name = "Istesso trial (Dev)"
        descriptor.maxPixelsWide = UInt32(width * 2)
        descriptor.maxPixelsHigh = UInt32(height * 2)
        descriptor.sizeInMillimeters = CGSize(width: Double(width) / 110 * 25.4, height: Double(height) / 110 * 25.4)
        descriptor.vendorID = 0x1234
        descriptor.productID = 0x0002
        descriptor.serialNum = 0x0001
        descriptor.redPrimary = CGPoint(x: 0.64, y: 0.33)
        descriptor.greenPrimary = CGPoint(x: 0.30, y: 0.60)
        descriptor.bluePrimary = CGPoint(x: 0.15, y: 0.06)
        descriptor.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
        descriptor.queue = DispatchQueue.main
        guard let made = CGVirtualDisplay(descriptor: descriptor) else { say("{\"vd\":\"not made\"}"); exit(1) }
        display = made
        let settings = CGVirtualDisplaySettings()
        settings.hiDPI = 1
        settings.modes = [CGVirtualDisplayMode(width: UInt32(width), height: UInt32(height), refreshRate: hertz),
                          CGVirtualDisplayMode(width: UInt32(width * 2), height: UInt32(height * 2), refreshRate: hertz)]
        let applied = made.apply(settings)
        say("{\"vd\":\"made\",\"applied\":\(applied),\"display\":\(made.displayID)}")
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { self.modes() }
    }

    func modes() {
        guard let id = display?.displayID else { return }
        let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
        let all = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
        let list = all.map { "\"\($0.width)x\($0.height) pt \($0.pixelWidth)x\($0.pixelHeight) px \(Int($0.refreshRate)) Hz\"" }
        say("{\"vd\":\"modes\",\"modes\":[\(list.joined(separator: ","))]}")
        // The HiDPI one of the size asked for, at the rate asked for, if there is one.
        if let hidpi = all.first(where: { $0.width == width && $0.pixelWidth == width * 2 }) {
            let result = CGDisplaySetDisplayMode(id, hidpi, nil)
            say("{\"vd\":\"set hidpi\",\"result\":\(result.rawValue)}")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { self.current() }
    }

    func current() {
        guard let id = display?.displayID, let mode = CGDisplayCopyDisplayMode(id) else { return }
        say("{\"vd\":\"current\",\"points\":\"\(mode.width)x\(mode.height)\",\"pixels\":\"\(mode.pixelWidth)x\(mode.pixelHeight)\",\"refresh_hz\":\(mode.refreshRate)}")
        guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id }) else {
            say("{\"vd\":\"not among the screens\"}")
            return finish()
        }
        say("{\"vd\":\"screen\",\"backing_scale\":\(screen.backingScaleFactor),\"max_fps\":\(screen.maximumFramesPerSecond)}")
        let frame = NSRect(x: screen.frame.minX + 40, y: screen.frame.minY + 40, width: 600, height: 300)
        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false, screen: screen)
        let view = AnimatedView(frame: NSRect(origin: .zero, size: frame.size))
        window.contentView = view
        window.setFrame(frame, display: true)
        window.orderFrontRegardless()
        let link = view.displayLink(target: view, selector: #selector(AnimatedView.tick(_:)))
        link.add(to: .main, forMode: .common)
        view.link = link
        self.window = window
        self.view = view
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.capture(id) }
    }

    func capture(_ id: CGDirectDisplayID) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, _ in
            DispatchQueue.main.async {
                guard let scDisplay = content?.displays.first(where: { $0.displayID == id }), let mode = CGDisplayCopyDisplayMode(id) else {
                    self.say("{\"vd\":\"not capturable\"}")
                    return self.finish()
                }
                let configuration = SCStreamConfiguration()
                configuration.width = mode.pixelWidth
                configuration.height = mode.pixelHeight
                configuration.pixelFormat = kCVPixelFormatType_32BGRA
                configuration.showsCursor = false
                configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(self.hertz))
                configuration.queueDepth = 4
                let stream = SCStream(filter: SCContentFilter(display: scDisplay, excludingWindows: []), configuration: configuration, delegate: nil)
                try? stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: self.queue)
                self.stream = stream
                stream.startCapture { error in
                    if let error { self.say("{\"vd\":\"capture failed\",\"why\":\"\(error.localizedDescription)\"}") }
                }
                let drawn0 = self.view?.frames ?? 0
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                    let start = self.queue.sync { self.captured }
                    let drawnStart = self.view?.frames ?? 0
                    DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                        let got = self.queue.sync { self.captured } - start
                        let drawn = (self.view?.frames ?? 0) - drawnStart
                        let size = self.queue.sync { self.capturedSize }
                        self.say("{\"vd\":\"rates\",\"drawn_per_s\":\(Double(drawn) / 5),\"captured_per_s\":\(Double(got) / 5),\"captured_pixels\":\"\(size.0)x\(size.1)\",\"drawn_before\":\(drawn0)}")
                        self.click(id)
                    }
                }
            }
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = attachments.first?[.status] as? Int, let status = SCFrameStatus(rawValue: raw), status != .complete { return }
        captured += 1
        capturedSize = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer))
    }

    func click(_ id: CGDirectDisplayID) {
        guard let window, CGPreflightPostEventAccess() else { say("{\"vd\":\"no click (no accessibility)\"}"); return finish() }
        let before = CGEvent(source: nil)?.location ?? .zero
        // 100, 50 points into the window, in global points from the top left.
        let mainTop = NSScreen.screens[0].frame.maxY
        let target = CGPoint(x: window.frame.minX + 100, y: mainTop - window.frame.maxY + 50)
        for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
            CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: target, mouseButton: .left)?.post(tap: .cgSessionEventTap)
            usleep(30_000)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            CGWarpMouseCursorPosition(before)
            let landed = self.view?.clicks.last.map { "[\(Int($0.x)),\(Int($0.y))]" } ?? "null"
            self.say("{\"vd\":\"click\",\"asked\":[100,50],\"landed\":\(landed)}")
            self.finish()
        }
    }

    func finish() {
        stream?.stopCapture { _ in }
        window?.orderOut(nil)
        window = nil
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            self.display = nil  // the display goes away with its object
            self.say("{\"vd\":\"removed\"}")
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { exit(0) }
        }
    }
}

/// `--close-window PID X Y W H`: closes the one window of the application PID at exactly that
/// place and size (global points), by pressing its close button through Accessibility: for a
/// window of the trial's own whose document cannot be named any more. Nothing else is touched.
func closeWindowAt(_ arguments: [String]) -> Never {
    let numbers = arguments.compactMap { Double($0) }
    guard numbers.count == 5, let pid = pid_t(arguments[0]) else { print("{\"closed\":false,\"why\":\"usage\"}"); exit(2) }
    let wanted = CGRect(x: numbers[1], y: numbers[2], width: numbers[3], height: numbers[4])
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success, let windows = value as? [AXUIElement] else {
        print("{\"closed\":false,\"why\":\"the application's windows cannot be read\"}"); exit(1)
    }
    for window in windows {
        var position: CFTypeRef?, extent: CFTypeRef?
        AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position)
        AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent)
        var p = CGPoint.zero, s = CGSize.zero
        if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
        if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &s) }
        guard abs(p.x - wanted.minX) < 2, abs(p.y - wanted.minY) < 2, abs(s.width - wanted.width) < 2, abs(s.height - wanted.height) < 2 else { continue }
        var button: CFTypeRef?
        guard AXUIElementCopyAttributeValue(window, kAXCloseButtonAttribute as CFString, &button) == .success, let button,
              AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString) == .success else {
            print("{\"closed\":false,\"why\":\"its close button could not be pressed\"}"); exit(1)
        }
        Thread.sleep(forTimeInterval: 1.0)
        var children: CFTypeRef?
        AXUIElementCopyAttributeValue(window, kAXChildrenAttribute as CFString, &children)
        let asking = (children as? [AXUIElement] ?? []).contains { child in
            var role: CFTypeRef?
            AXUIElementCopyAttributeValue(child, kAXRoleAttribute as CFString, &role)
            return role as? String == kAXSheetRole as String
        }
        print(asking ? "{\"closed\":false,\"why\":\"the application asks in a sheet (left as it is)\"}" : "{\"closed\":true}")
        exit(asking ? 1 : 0)
    }
    // Accessibility lists one window whose place it cannot say, and the window list has exactly one
    // window of the application on the screen, at the place asked for: that one.
    let shown = readWindowList(onScreenOnly: true, desktop: false).filter { $0.pid == pid && $0.layer == 0 }
    if windows.count == 1, shown.count == 1, abs(shown[0].bounds.minX - wanted.minX) < 2, abs(shown[0].bounds.minY - wanted.minY) < 2,
       abs(shown[0].bounds.width - wanted.width) < 2, abs(shown[0].bounds.height - wanted.height) < 2 {
        var button: CFTypeRef?
        if AXUIElementCopyAttributeValue(windows[0], kAXCloseButtonAttribute as CFString, &button) == .success, let button,
           AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString) == .success {
            Thread.sleep(forTimeInterval: 1.0)
            let still = readWindowList(onScreenOnly: true, desktop: false).contains { $0.id == shown[0].id }
            print(still ? "{\"closed\":false,\"why\":\"pressed, but the window stays (a question in it?)\"}" : "{\"closed\":true,\"by\":\"the only window\"}")
            exit(still ? 1 : 0)
        }
    }
    let places = axWindows(pid).map { "\"\(Int($0.bounds.minX)),\(Int($0.bounds.minY)) \(Int($0.bounds.width))x\(Int($0.bounds.height)) \($0.role)\"" }
    print("{\"closed\":false,\"why\":\"no window at that place\",\"windows\":\(windows.count),\"places\":[\(places.joined(separator: ","))]}")
    exit(1)
}

/// `--self-test`: the rule of the session form, without any window or permission.
func selfTest() -> Never {
    setvbuf(stdout, nil, _IONBF, 0)
    let win = CGRect(x: 100, y: 100, width: 200, height: 100)
    let inside = CGPoint(x: 150, y: 150), outside = CGPoint(x: 500, y: 500)
    var failed = 0
    func check(_ got: SessionStep, _ want: SessionStep, _ what: String) {
        if got != want { failed += 1; print("FAIL \(what): \(got) != \(want)") } else { print("ok   \(what)") }
    }
    check(sessionStep(type: .leftMouseDown, at: inside, onTarget: true, pressWindow: nil, window: win), .post(inside, .press(win)), "press onto the app")
    check(sessionStep(type: .leftMouseDown, at: inside, onTarget: false, pressWindow: nil, window: win), .drop, "press onto another window is dropped")
    check(sessionStep(type: .leftMouseUp, at: inside, onTarget: true, pressWindow: nil, window: win), .drop, "release without a posted press is dropped")
    check(sessionStep(type: .leftMouseUp, at: outside, onTarget: false, pressWindow: win, window: win), .post(CGPoint(x: 299, y: 199), .release), "release outside is kept inside the press window")
    check(sessionStep(type: .leftMouseDragged, at: outside, onTarget: false, pressWindow: win, window: win), .post(CGPoint(x: 299, y: 199), .nothing), "drag outside is kept inside")
    check(sessionStep(type: .leftMouseDragged, at: inside, onTarget: true, pressWindow: nil, window: win), .toProcess, "drag without a posted press goes to the process")
    check(sessionStep(type: .mouseMoved, at: outside, onTarget: false, pressWindow: nil, window: win), .toProcess, "move over another window goes to the process")
    check(sessionStep(type: .mouseMoved, at: inside, onTarget: true, pressWindow: nil, window: win), .post(inside, .nothing), "move over the app")
    check(sessionStep(type: .scrollWheel, at: outside, onTarget: false, pressWindow: nil, window: win), .drop, "scroll over another window is dropped")
    check(sessionStep(type: .leftMouseDown, at: inside, onTarget: true, pressWindow: win, window: win), .drop, "a second press of a held button is dropped")
    let panel = CGRect(x: 400, y: 300, width: 600, height: 400)
    func checkPanel(_ got: Bool, _ want: Bool, _ what: String) {
        if got != want { failed += 1; print("FAIL \(what)") } else { print("ok   \(what)") }
    }
    do {
        let owner = "com.example.prompt" // check-private: ok synthetic bundle identifier
        let other = "com.example.other" // check-private: ok synthetic bundle identifier
        let pid = kCGWindowOwnerPID as String, layer = kCGWindowLayer as String
        let level = systemPromptMinLayer
        let windows: [NSDictionary] = [
            [pid: 1, layer: level], [pid: 1, layer: level + 1], [pid: 1, layer: level - 1],
            [pid: 2, layer: level], [pid: 3, layer: level], [pid: 1], [layer: level]
        ]
        let ids: [pid_t: String] = [1: owner, 2: other]
        checkPanel(systemPromptCount(windows, owners: [owner]) { ids[$0] } == 2, true,
                   "prompt: count windows with a listed owner at or above the threshold")
        checkPanel(systemPromptCount(windows, owners: []) { ids[$0] } == 0, true, "prompt: empty owner list")
        // Per owner: the Accessibility prompt's process counts at layer 0, the others only from 8.
        let access = "com.apple.accessibility.universalAccessAuthWarn", agent = "com.apple.SecurityAgent"
        let shown: [NSDictionary] = [[pid: 4, layer: 0], [pid: 5, layer: 0], [pid: 5, layer: 1000]]
        let who: [pid_t: String] = [4: access, 5: agent]
        checkPanel(systemPromptCount(shown, owners: systemPromptOwners) { who[$0] } == 2, true,
                   "prompt: the Accessibility prompt at layer 0 counts; SecurityAgent only at its own layer")
        var state = SystemPromptState()
        let samples: [Bool?] = [false, true, false, true, nil, true, true, true, false, true, false, false, false]
        let expected: [Bool?] = [nil, nil, nil, nil, nil, nil, true, nil, nil, nil, nil, false, nil]
        checkPanel(samples.map { state.observe($0) } == expected, true,
                   "prompt: two consecutive reads in either direction, one report per change")
        let sampled = DispatchSemaphore(value: 0)
        let monitor = SystemPromptMonitor {
            precondition(!Thread.isMainThread)
            sampled.signal()
            return nil
        }
        monitor.start()
        checkPanel(sampled.wait(timeout: .now() + 3) == .success, true, "prompt: timer samples off the main thread")
        monitor.stop()
        let limit = owner + String(repeating: "a", count: 128 - owner.utf8.count)
        checkPanel(validSystemPromptOwner(limit) && !validSystemPromptOwner(limit + "a")
                   && !validSystemPromptOwner(owner + "\n") && !validSystemPromptOwner("--flag"),
                   true, "prompt: reject oversized identifiers and invalid characters")
    }
    let standard = AXWindowInfo(bounds: win, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String)
    let sheet = AXWindowInfo(bounds: panel, role: kAXSheetRole as String, subrole: "")
    let dialog = AXWindowInfo(bounds: panel, role: kAXWindowRole as String, subrole: kAXDialogSubrole as String)
    let fake = AXWindowInfo(bounds: panel, role: kAXWindowRole as String, subrole: kAXStandardWindowSubrole as String)
    checkPanel(panelAllowed(serviceWindow: panel, appWindows: [standard, sheet], appInFront: true), true, "the application's own panel, a sheet")
    checkPanel(panelAllowed(serviceWindow: panel, appWindows: [standard, dialog], appInFront: true), true, "the application's own panel, a dialog")
    checkPanel(panelAllowed(serviceWindow: panel, appWindows: [standard], appInFront: true), false, "a panel the application does not show (another application's)")
    checkPanel(panelAllowed(serviceWindow: panel, appWindows: [standard, fake], appInFront: true), false, "a standard window of the same place and size does not count")
    checkPanel(panelAllowed(serviceWindow: panel, appWindows: [standard, sheet], appInFront: false), false, "no panel while the application is not in front")
    checkPanel(capturedWindows(appWindows: [1, 2], serviceWindows: [10, 11], allowed: [10]) == [1, 2, 10], true,
               "a new window of the panel service (another application's) is not captured")
    checkPanel(capturedWindows(appWindows: [1], serviceWindows: [10], allowed: []) == [1], true, "nothing of the service by default")
    checkPanel(passesWhileLocked(kind: "key", pressed: true, held: false), false, "locked: a key press is dropped")
    checkPanel(passesWhileLocked(kind: "key", pressed: false, held: true), true, "locked: the release of a held key passes")
    checkPanel(passesWhileLocked(kind: "key", pressed: false, held: false), false, "locked: a release of a key not held is dropped")
    checkPanel(passesWhileLocked(kind: "text", pressed: nil, held: false), false, "locked: text is dropped")
    checkPanel(passesWhileLocked(kind: "pointer_at", pressed: nil, held: false), false, "locked: a move is dropped")
    checkPanel(passesWhileLocked(kind: "button", pressed: false, held: true), true, "locked: the release of a held button passes")
    checkPanel(modifierFlags(of: [42, 30]) == .maskShift, true, "flags from the keys held: Shift")
    checkPanel(modifierFlags(of: [30]) == [], true, "flags from the keys held: none once the modifier is up")
    checkPanel(modifierFlags(of: [125, 29]) == [.maskCommand, .maskControl], true, "flags from the keys held: Command and Control")
    let slow = Lookout()
    slow.lookup = { _ in Thread.sleep(forTimeInterval: 1.0); return [] }
    let began = ProcessInfo.processInfo.systemUptime
    slow.refresh(1)
    _ = slow.windows
    slow.refresh(1)
    checkPanel(ProcessInfo.processInfo.systemUptime - began < 0.05, true, "a slow Accessibility lookup never holds the input up")
    // The injector itself, with stand-ins for the window list, the lock and the posting.
    let appWin = WindowInfo(id: 7, pid: 100, layer: 0, bounds: win, alpha: 1, onScreen: true)
    let other = WindowInfo(id: 8, pid: 200, layer: 0, bounds: win, alpha: 1, onScreen: true)
    let moved = WindowInfo(id: 7, pid: 100, layer: 0, bounds: win.offsetBy(dx: 500, dy: 0), alpha: 1, onScreen: true)
    func clicks(list: [WindowInfo], lockAfterPress: Bool = false) -> Int {
        let injector = Injector()
        var shown = list
        var locked = false
        var posted = 0
        injector.mayPost = true
        injector.clickForm = 4
        injector.listNow = { shown }
        injector.lockedNow = { locked }
        injector.inRange = { _, top in top?.pid == 100 }
        injector.windowAt = { point in shown.first { $0.pid == 100 && $0.bounds.contains(point) } }
        injector.postSession = { event in
            if [.leftMouseDown, .leftMouseUp].contains(event.type) { posted += 1 }
        }
        injector.postProcess = { _, _ in }
        func send(_ object: [String: Any]) { injector.handle(try! JSONSerialization.data(withJSONObject: object)) }
        // The list seen when the pointer came there: the application's window.
        shown = [appWin]
        send(["kind": "pointer_at", "x": 150, "y": 150])
        shown = list  // what it is right at the press
        send(["kind": "button", "code": 0x110, "pressed": true])
        if lockAfterPress { locked = true }
        send(["kind": "button", "code": 0x110, "pressed": false])
        return posted
    }
    checkPanel(clicks(list: [appWin]) == 2, true, "injector: a press and release onto the application's window go through")
    checkPanel(clicks(list: [other, appWin]) == 0, true, "injector: a window that came in front since the last look is not pressed through")
    checkPanel(clicks(list: []) == 0, true, "injector: a window that closed since the last look is not pressed")
    checkPanel(clicks(list: [moved]) == 0, true, "injector: a window that moved away since the last look is not pressed")
    checkPanel(clicks(list: [appWin], lockAfterPress: true) == 2, true, "injector: a press made before the lock is released in it")
    do {
        // While a new stream is made: nothing but releases.
        let injector = Injector()
        var gate = false
        var downs = 0, ups = 0
        injector.mayPost = true
        injector.clickForm = 4
        injector.listNow = { [appWin] }
        injector.lockedNow = { false }
        injector.gateClosed = { gate }
        injector.inRange = { _, top in top?.pid == 100 }
        injector.windowAt = { point in appWin.bounds.contains(point) ? appWin : nil }
        injector.postSession = { event in
            if event.type == .leftMouseDown { downs += 1 }
            if event.type == .leftMouseUp { ups += 1 }
        }
        injector.postProcess = { _, _ in }
        func send(_ object: [String: Any]) { injector.handle(try! JSONSerialization.data(withJSONObject: object)) }
        send(["kind": "pointer_at", "x": 150, "y": 150])
        send(["kind": "button", "code": 0x110, "pressed": true])
        gate = true
        send(["kind": "button", "code": 0x110, "pressed": false])
        send(["kind": "button", "code": 0x110, "pressed": true])
        checkPanel(downs == 1 && ups == 1, true, "restarting: a held button is released, a new press is dropped")
    }
    checkPanel(restartResumes(picture: "running", detached: false, offered: true, held: false, wordsMeanwhile: []), true, "resize: a running picture comes back")
    checkPanel(restartResumes(picture: "running", detached: false, offered: true, held: false, wordsMeanwhile: ["pause"]), false, "resize: a pause during the restart is kept")
    checkPanel(restartResumes(picture: "running", detached: false, offered: true, held: false, wordsMeanwhile: ["detach"]), false, "resize: a detach during the restart is kept")
    checkPanel(restartResumes(picture: "running", detached: false, offered: true, held: true, wordsMeanwhile: []), false, "resize: held for a measurement, it stays stopped")
    checkPanel(restartResumes(picture: "paused", detached: false, offered: true, held: false, wordsMeanwhile: []), false, "resize: a stopped picture stays stopped")
    func steps(_ admitting: AdmitState, offered: Bool, listens: Bool = true) -> [String] {
        var calls: [String] = []
        startStream(admitting: admitting, offered: offered,
                    listenClosed: { calls.append("listen-closed"); return listens },
                    offerNot: { calls.append("offer-0") },
                    admitPaired: { calls.append("admit-paired") },
                    admitOnly: { id, _ in calls.append("admit-\(id)") })
        return calls
    }
    checkPanel(steps(.paired, offered: true) == ["listen-closed", "admit-paired"], true, "start: paired, after listening closed")
    checkPanel(steps(.only(id: "a", key: "k"), offered: true) == ["listen-closed", "admit-a"], true, "start: one device, after listening closed")
    checkPanel(steps(.nobody, offered: true) == ["listen-closed"], true, "start: nobody, nothing more")
    checkPanel(steps(.paired, offered: false) == ["listen-closed", "offer-0", "admit-paired"], true, "start: not offered before anybody is admitted")
    checkPanel(steps(.paired, offered: true, listens: false) == ["listen-closed"], true, "start: nothing is called when listening failed")
    let after = stateAfter(words: ["admit a k", "detach"], admit: .paired, offered: true, detached: false, held: false)
    checkPanel(after.0 == .only(id: "a", key: "k") && after.1 == false && after.2 == true, true, "restart: words that came meanwhile decide the state first")
    let held = stateAfter(words: ["resume"], admit: .nobody, offered: false, detached: true, held: true)
    checkPanel(held.1 == false && held.2 == true, true, "restart: a resume while held for a measurement does not offer")
    checkPanel(lockResumes(pausedForLock: true, detached: false, held: false), true, "unlock: the picture the lock stopped comes back")
    checkPanel(lockResumes(pausedForLock: false, detached: false, held: false), false, "unlock: an explicit pause stays")
    checkPanel(lockResumes(pausedForLock: true, detached: true, held: false), false, "unlock: a detach stays")
    checkPanel(lockResumes(pausedForLock: true, detached: false, held: true), false, "unlock: held for a measurement, it stays stopped")
    let shownWin = CGRect(x: 100, y: 100, width: 600, height: 400)
    checkPanel(keysMayGo(appInFront: true, focused: shownWin, shown: shownWin), true, "keys: the shown window is the focused one")
    checkPanel(keysMayGo(appInFront: true, focused: shownWin.offsetBy(dx: 40, dy: 30), shown: shownWin), false, "keys: a raise that did not take (another window has the keys) sends nothing")
    checkPanel(keysMayGo(appInFront: true, focused: nil, shown: shownWin), false, "keys: no focused window known, nothing is sent")
    checkPanel(keysMayGo(appInFront: false, focused: shownWin, shown: shownWin), false, "keys: not in front, nothing is sent")
    do {
        // The key path asks Accessibility nothing: a thousand keys, one lookup (the notification's).
        let counted = Lookout()
        var lookups = 0
        counted.focusedLookup = { _ in lookups += 1; return shownWin }
        counted.refreshFocused(1)
        counted.queue.sync {}
        var allowed = 0
        for _ in 0..<1000 where keysMayGo(appInFront: true, focused: counted.focused, shown: shownWin) { allowed += 1 }
        checkPanel(lookups == 1 && allowed == 1000, true, "keys: no Accessibility call on the key path; the notification's answer is used")
        // A notification that the key window moved away: keys stop at once.
        counted.focusedLookup = { _ in lookups += 1; return shownWin.offsetBy(dx: 200, dy: 0) }
        counted.refreshFocused(1)
        counted.queue.sync {}
        checkPanel(!keysMayGo(appInFront: true, focused: counted.focused, shown: shownWin), true, "keys: after the key window moved away, nothing is sent")
    }
    do {
        // A stand-in raise that does not take: the focused window stays another one.
        let stuck = Lookout()
        stuck.focusedLookup = { _ in shownWin.offsetBy(dx: 40, dy: 30) }
        let done = DispatchSemaphore(value: 0)
        stuck.queue.async { done.signal() }
        stuck.refreshFocused(1)
        stuck.queue.sync {}
        _ = done.wait(timeout: .now() + 1)
        checkPanel(!keysMayGo(appInFront: true, focused: stuck.focused, shown: shownWin), true, "keys: after a raise that did not take, keys are not sent")
    }
    let mainDisplay = CGRect(x: 0, y: 0, width: 3440, height: 1440)
    checkPanel(cornerOrigin(main: mainDisplay) == CGPoint(x: 3440, y: 1440), true, "virtual display: touches the main one at its bottom right corner only")
    checkPanel(rowOrigin(main: mainDisplay, width: 1710, slot: 2) == CGPoint(x: 5150, y: 1440), true, "virtual display 2: in the row, touching only the first")
    let visibleMain = CGRect(x: 0, y: 25, width: 3440, height: 1415)
    checkPanel(fitInto(CGRect(x: 5000, y: 2000, width: 600, height: 400), visibleMain) == CGRect(x: 2840, y: 1040, width: 600, height: 400), true, "back: a place now off every display is fitted into the main one")
    checkPanel(fitInto(CGRect(x: 10, y: 0, width: 5000, height: 400), visibleMain) == CGRect(x: 0, y: 25, width: 3440, height: 400), true, "back: too large is made as large as fits")
    checkPanel(onSomeDisplay(CGRect(x: 100, y: 100, width: 50, height: 50), displays: [mainDisplay]), true, "back: an original place still on a display is kept")
    checkPanel(!onSomeDisplay(CGRect(x: 4000, y: 100, width: 50, height: 50), displays: [mainDisplay]), true, "back: an original place on no display is not kept")
    do {
        // After a Command chord, nothing goes until the next lookup has landed.
        let chord = Lookout()
        chord.focusedLookup = { _ in shownWin }
        chord.refreshFocused(1)
        chord.queue.sync {}
        let before = keysMayGo(appInFront: true, focused: chord.focused, shown: shownWin)
        chord.invalidateFocused()
        let during = keysMayGo(appInFront: true, focused: chord.focused, shown: shownWin)
        chord.refreshFocused(1)
        chord.queue.sync {}
        let after = keysMayGo(appInFront: true, focused: chord.focused, shown: shownWin)
        checkPanel(before && !during && after, true, "keys: stopped after a Command chord until the key window is known again")
    }
    checkPanel(!keysMayGo(appInFront: true, focused: shownWin, shown: shownWin, shownIsFront: false), true, "keys: a window at the same place in front of the shown one does not pass")
    func chordRun(flush: Bool) -> (posted: Int, discarded: Int) {
        let injector = Injector()
        var posted = 0
        injector.mayPost = true
        injector.target = { 100 }
        injector.keysAllowed = { true }
        injector.lockedNow = { false }
        injector.postProcess = { event, _ in if event.type == .keyDown { posted += 1 } }
        injector.onCommandChord = { code in if chordMayChangeWindow(code) { injector.beginHold() } }
        func send(_ object: [String: Any]) { injector.handle(try! JSONSerialization.data(withJSONObject: object)) }
        send(["kind": "key", "code": 125, "pressed": true])   // Command
        send(["kind": "key", "code": 49, "pressed": true])    // N: the chord, sent; the hold begins
        send(["kind": "key", "code": 49, "pressed": false])
        send(["kind": "key", "code": 125, "pressed": false])  // the release of a key sent before: goes
        send(["kind": "key", "code": 45, "pressed": true])    // typing after the chord: held
        send(["kind": "key", "code": 45, "pressed": false])
        let before = posted
        injector.endHold(flush: flush)
        return (posted - before, injector.counts["held_discarded"] ?? 0)
    }
    checkPanel(!chordMayChangeWindow(47) && !chordMayChangeWindow(46) && chordMayChangeWindow(49), true, "chords: Command-V and -C pass straight on, Command-N is looked at")
    do {
        // A chord's hold holds composing text too, and lets everything go through the host's own
        // order (where a commit's wait for "inserted" may hold it again), never straight here.
        let injector = Injector()
        var preedits = 0
        var again: [String] = []
        injector.mayPost = true
        injector.target = { 100 }
        injector.keysAllowed = { true }
        injector.lockedNow = { false }
        injector.postProcess = { _, _ in }
        injector.onPreedit = { _, _ in preedits += 1 }
        injector.onText = { _ in true }
        injector.redispatch = { data in again.append(((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])?["kind"] as? String ?? "?") }
        func send(_ object: [String: Any]) { injector.handle(try! JSONSerialization.data(withJSONObject: object)) }
        injector.beginHold()
        send(["kind": "text", "text": "a"])
        send(["kind": "preedit", "text": "b", "cursor": [1, 1]])
        send(["kind": "key", "code": 30, "pressed": true])
        checkPanel(preedits == 0 && injector.counts["held"] == 3, true, "chords: text, composing text and keys are held alike")
        injector.endHold(flush: true)
        checkPanel(again == ["text", "preedit", "key"] && preedits == 0, true, "chords: let go through the host's dispatch, in order")
    }
    do {
        // Command-V then typing: nothing is held at all.
        let injector = Injector()
        var posted = 0
        injector.mayPost = true
        injector.target = { 100 }
        injector.keysAllowed = { true }
        injector.lockedNow = { false }
        injector.postProcess = { event, _ in if event.type == .keyDown { posted += 1 } }
        injector.onCommandChord = { code in if chordMayChangeWindow(code) { injector.beginHold() } }
        func send(_ object: [String: Any]) { injector.handle(try! JSONSerialization.data(withJSONObject: object)) }
        send(["kind": "key", "code": 125, "pressed": true]); send(["kind": "key", "code": 47, "pressed": true])
        send(["kind": "key", "code": 47, "pressed": false]); send(["kind": "key", "code": 125, "pressed": false])
        send(["kind": "key", "code": 30, "pressed": true]); send(["kind": "key", "code": 30, "pressed": false])
        checkPanel(posted == 2 && !injector.holdingForChord, true, "Command-V then typing: the letter goes at once")
    }
    do {
        // A clipboard chord goes through the same rules as a key: closed, nothing is posted.
        let injector = Injector()
        var posted = 0
        injector.mayPost = true
        injector.target = { 100 }
        injector.keysAllowed = { true }
        injector.lockedNow = { false }
        injector.gateClosed = { true }
        injector.postProcess = { _, _ in posted += 1 }
        injector.postChord(46)
        let closed = posted
        injector.gateClosed = { false }
        injector.postChord(46)
        checkPanel(closed == 0 && posted == 4, true, "clipboard chord: through the input's rules (none while closed, Command and C when open)")
    }
    let flushed = chordRun(flush: true)
    checkPanel(flushed.posted == 1 && flushed.discarded == 0, true, "chord then typing: the held keys go once the key window is confirmed")
    let dropped = chordRun(flush: false)
    checkPanel(dropped.posted == 0 && dropped.discarded > 0, true, "chord that opened a new window: the held keys are let go of, with the notice")
    do {
        var carry = (0.0, 0.0)
        let finger = scrollPlan(dx: 0, dy: 10, source: "finger", v120: nil, pixels: [0, 21], phase: "changed", momentum: nil,
                                pointsPerPixel: 0.5, carry: &carry)
        checkPanel(finger == ScrollPlan(lines: false, wheel1: -10, wheel2: 0, exact1: -10.5, exact2: -0, continuous: true, phase: 2, momentum: 0), true,
                   "scroll: fingers in points, continuous, the phase said")
        checkPanel(abs(carry.1 + 0.5) < 1e-9, true, "scroll: the half point left over is kept for the next")
        var carry2 = (0.0, 0.0)
        let wheel = scrollPlan(dx: 0, dy: 15, source: "wheel", v120: [0, 360], pixels: nil, phase: nil, momentum: nil, pointsPerPixel: 1, carry: &carry2)
        checkPanel(wheel.lines && wheel.wheel1 == -3 && !wheel.continuous, true, "scroll: a wheel in lines, three notches, not continuous")
        var carry3 = (0.0, 0.0)
        let inertia = scrollPlan(dx: 0, dy: 4, source: "finger", v120: nil, pixels: [0, 4], phase: nil, momentum: "ended", pointsPerPixel: 1, carry: &carry3)
        checkPanel(inertia.momentum == 3 && inertia.phase == 0, true, "scroll: the inertia's end is momentum 3")
        var carry4 = (0.0, 0.0)
        let old = scrollPlan(dx: 2, dy: -7, source: nil, v120: nil, pixels: nil, phase: nil, momentum: nil, pointsPerPixel: 1, carry: &carry4)
        checkPanel(old.wheel1 == 7 && old.wheel2 == -2 && !old.lines, true, "scroll: without a source, as before")
        var carry5 = (0.0, 0.0)
        let least = scrollPlan(dx: Int32.max, dy: Int32.min, source: nil, v120: nil, pixels: nil, phase: nil, momentum: nil, pointsPerPixel: 1, carry: &carry5)
        checkPanel(least.wheel1 == 10_000 && least.wheel2 == -10_000, true, "scroll: Int32's least and greatest are bounded, nothing traps")
        var carry6 = (0.0, 0.0)
        let huge = scrollPlan(dx: 0, dy: 0, source: "finger", v120: nil, pixels: [1e300, -1e300], phase: nil, momentum: nil, pointsPerPixel: 1, carry: &carry6)
        checkPanel(abs(huge.wheel1) <= 1_000_000 && abs(huge.wheel2) <= 1_000_000, true, "scroll: enormous pixels are bounded, nothing traps")
    }
    do {
        checkPanel(judgeText(String(repeating: "a", count: 65536)) == .text, true, "clipboard: 65536 bytes go")
        checkPanel(judgeText(String(repeating: "a", count: 65537)) == .tooLarge, true, "clipboard: one byte more is too large")
        checkPanel(judgeText("") == .notText && judgeText("a\u{07}b") == .notText && judgeText("a\u{85}") == .notText, true, "clipboard: empty and control characters are not text")
        checkPanel(judgeText("a\tb\nc\r") == .text, true, "clipboard: tab, line feed and carriage return are text")
        let a = clipboardAnswer(nonce: "n", result: "text", text: "t", why: "x")
        let b = clipboardAnswer(nonce: "n", result: "refused", text: "t", why: "password_field")
        let c = clipboardAnswer(nonce: "n", result: "none", text: "t", why: "x")
        checkPanel(a["text"] != nil && a["why"] == nil && b["text"] == nil && b["why"] != nil && c["text"] == nil && c["why"] == nil, true,
                   "clipboard: text only with text, why only with refused")
        // A pasteboard of its own (never the Studio's real one).
        let own = SystemBoard(NSPasteboard.withUniqueName())
        let change = own.put("trial")
        checkPanel(own.changeCount == change && own.plainText() == "trial", true, "clipboard: a paste's text is put on the pasteboard")
        checkPanel(own.types().contains("org.nspasteboard.TransientType"), true, "clipboard: and marked transient (no history keeps it)")
        let bridge = ClipboardBridge(board: own)
        bridge.finishCopy(nonce: String(repeating: "a", count: 32))
        checkPanel(bridge.counts["copy_refused"] == 1, true, "clipboard: a copy finds a secret marker and refuses before reading")
        own.clear()
        own.pasteboard.setString("plain", forType: .string)
        bridge.finishCopy(nonce: String(repeating: "b", count: 32))
        checkPanel(bridge.counts["copy_text"] == 1, true, "clipboard: plain text is copied")
        own.pasteboard.releaseGlobally()
        let gate = PasteGate()
        gate.begin("n", now: 0)
        gate.hold(Data([1])); gate.hold(Data([2]))
        checkPanel(!gate.expired(now: 0.1) && gate.expired(now: 0.3) && gate.end() == [Data([1]), Data([2])], true,
                   "clipboard: what comes after a late paste is held 0.2 s at most, then given in order")
    }
    let guardedFlag = Guarded()
    var ran = 0
    _ = guardedFlag.use { ran += 1 }
    guardedFlag.close { ran += 10 }
    _ = guardedFlag.use { ran += 100 }
    guardedFlag.close { ran += 1000 }
    checkPanel(ran == 11, true, "a closed stream takes no more calls, and is closed once")
    failed += inlineSelfTest()
    print(failed == 0 ? "all passed" : "\(failed) failed")
    exit(failed == 0 ? 0 : 1)
}

/// Accessibility calls, away from the input: an application that hangs can hold such a call for
/// seconds (0.25 s with the timeout set), so they run on a queue of their own, and the input uses
/// what they found last. Work that is asked for while some is running replaces what waits.
final class Lookout {
    let queue = DispatchQueue(label: "istesso.daily-host.accessibility", qos: .userInitiated)
    private let lock = NSLock()
    private var cached: [AXWindowInfo] = []
    private var asked = false
    /// The lookup itself; a stand-in in --self-test.
    var lookup: (pid_t) -> [AXWindowInfo] = axWindows

    /// The application's windows as Accessibility listed them last; never waits.
    var windows: [AXWindowInfo] { lock.lock(); defer { lock.unlock() }; return cached }

    /// Asks for a fresh look; returns at once.
    func refresh(_ pid: pid_t) {
        lock.lock()
        if asked { lock.unlock(); return }
        asked = true
        lock.unlock()
        queue.async {
            let found = self.lookup(pid)
            self.lock.lock()
            self.cached = found
            self.asked = false
            self.lock.unlock()
        }
    }

    /// Runs one Accessibility action away from the input; returns at once.
    func act(_ work: @escaping () -> Void) { queue.async(execute: work) }

    /// The place of the application's focused window (the one keys go to), as last looked up.
    private var focusedCache: CGRect?
    var focusedLookup: (pid_t) -> CGRect? = axFocusedWindowBounds
    /// Not known any more (a Command chord may have opened or switched a window): no key goes
    /// until the next lookup has landed.
    func invalidateFocused() { lock.lock(); focusedCache = nil; lock.unlock() }
    var focused: CGRect? { lock.lock(); defer { lock.unlock() }; return focusedCache }
    /// Looks up the focused window again, away from the input; `then` runs on the main queue.
    func refreshFocused(_ pid: pid_t, then: (() -> Void)? = nil) {
        queue.async {
            let found = self.focusedLookup(pid)
            self.lock.lock(); self.focusedCache = found; self.lock.unlock()
            if let then { DispatchQueue.main.async(execute: then) }
        }
    }
}

/// The place of an application's focused window through Accessibility (global points, top left).
func axFocusedWindowBounds(_ pid: pid_t) -> CGRect? {
    let app = AXUIElementCreateApplication(pid)
    var window: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute as CFString, &window) == .success,
          let window else { return nil }
    var position: CFTypeRef?, extent: CFTypeRef?
    guard AXUIElementCopyAttributeValue(window as! AXUIElement, kAXPositionAttribute as CFString, &position) == .success,
          AXUIElementCopyAttributeValue(window as! AXUIElement, kAXSizeAttribute as CFString, &extent) == .success,
          let position, let extent else { return nil }
    var p = CGPoint.zero, s = CGSize.zero
    AXValueGetValue(position as! AXValue, .cgPoint, &p)
    AXValueGetValue(extent as! AXValue, .cgSize, &s)
    return CGRect(origin: p, size: s)
}

/// Command chords that edit or format and never open or switch a window: no hold after them
/// (C, V, X, Z, A, L, F, B, I, U, G, E, by the Linux key numbers).
// (A function, not a global constant: main.swift's globals are made in the order the file runs,
// and --self-test runs before this line would.)
func chordMayChangeWindow(_ code: UInt32) -> Bool { ![46, 47, 45, 44, 30, 38, 33, 48, 23, 22, 34, 18].contains(code) }

/// Whether keys may go to the application now: it is in front, and the window keys go to (its
/// focused window, by Accessibility) is the very window that is shown, by its place and size in
/// the window list. Otherwise typed text would land in a window nobody sees.
func keysMayGo(appInFront: Bool, focused: CGRect?, shown: CGRect, shownIsFront: Bool = true) -> Bool {
    guard appInFront, shownIsFront, let focused, shown != .zero else { return false }
    return abs(focused.minX - shown.minX) < 2 && abs(focused.minY - shown.minY) < 2
        && abs(focused.width - shown.width) < 2 && abs(focused.height - shown.height) < 2
}

/// After an unlock, whether the picture comes back: only when the lock stopped it, and the
/// controller has not paused or detached it since, and no measurement holds it.
func lockResumes(pausedForLock: Bool, detached: Bool, held: Bool) -> Bool {
    pausedForLock && !detached && !held
}

/// After a resize, whether the picture is started again on the new stream: only when it runs now
/// and nothing that came meanwhile says otherwise (a pause, a detach, a hold of the signal).
func restartResumes(picture: String, detached: Bool, offered: Bool, held: Bool, wordsMeanwhile: [String]) -> Bool {
    guard picture == "running", !detached, offered, !held else { return false }
    return !wordsMeanwhile.contains { $0.hasPrefix("pause") || $0.hasPrefix("detach") }
}

/// One scroll event for the Mac, from the other side's (ADR 0007, the trial's scroll section):
/// fingers in pixels (scaled from picture pixels to points, fractions kept), continuous, with the
/// gesture's phase and the inertia's momentum phase; a wheel in lines (notches, 120 to one), not
/// continuous; neither said, the old dx and dy in pixels.
struct ScrollPlan: Equatable {
    var lines: Bool
    var wheel1: Int32
    var wheel2: Int32
    var exact1: Double
    var exact2: Double
    var continuous: Bool
    var phase: Int64
    var momentum: Int64
}

func scrollPlan(dx: Int32, dy: Int32, source: String?, v120: [Double]?, pixels: [Double]?, phase: String?, momentum: String?,
                pointsPerPixel: Double, carry: inout (Double, Double)) -> ScrollPlan {
    let phases: [String: Int64] = ["began": 1, "changed": 2, "ended": 4, "cancelled": 8]
    let momenta: [String: Int64] = ["began": 1, "changed": 2, "ended": 3]
    func whole(_ x: Double, _ y: Double) -> (Int32, Int32) {
        // The Mac's axis 1 is up and down, positive towards the top: the other way round.
        let a = -y + carry.1, b = -x + carry.0
        let ra = min(max(a.rounded(.towardZero), -1_000_000), 1_000_000), rb = min(max(b.rounded(.towardZero), -1_000_000), 1_000_000)
        carry = (b - rb, a - ra)
        return (Int32(clamping: Int(ra)), Int32(clamping: Int(rb)))
    }
    if source == "finger", let p = pixels {
        let x = p[0] * pointsPerPixel, y = p[1] * pointsPerPixel
        let (w1, w2) = whole(x, y)
        return ScrollPlan(lines: false, wheel1: w1, wheel2: w2, exact1: -y, exact2: -x, continuous: true,
                          phase: phase.flatMap { phases[$0] } ?? 0, momentum: momentum.flatMap { momenta[$0] } ?? 0)
    }
    if source == "wheel", let n = v120 {
        let x = n[0] / 120, y = n[1] / 120
        let (w1, w2) = whole(x, y)
        return ScrollPlan(lines: true, wheel1: w1, wheel2: w2, exact1: -y, exact2: -x, continuous: false, phase: 0, momentum: 0)
    }
    let y = min(max(dy, -10_000), 10_000), x = min(max(dx, -10_000), 10_000)
    return ScrollPlan(lines: false, wheel1: 0 &- y, wheel2: 0 &- x, exact1: -Double(y), exact2: -Double(x), continuous: false, phase: 0, momentum: 0)
}

/// Whether an input event may pass while the screen is locked or the session is not on the
/// console (ADR 0017, "守ること" 3): only the release of a key or a button that is down here.
func passesWhileLocked(kind: String, pressed: Bool?, held: Bool) -> Bool {
    (kind == "key" || kind == "button") && pressed == false && held
}

/// The modifiers that are down, derived from the keys that are down (never kept on their own: a
/// release that was lost would leave a modifier held for good).
func modifierFlags(of keysDown: Set<UInt32>) -> CGEventFlags {
    var flags: CGEventFlags = []
    for code in keysDown { if let flag = modifierFlag(code) { flags.insert(flag) } }
    return flags
}

/// Whether the login session's screen is locked now (unreadable counts as locked).
func screenLocked() -> Bool {
    guard let session = CGSessionCopyCurrentDictionary() as? [String: Any] else { return true }
    let onConsole = (session[kCGSessionOnConsoleKey as String] as? Bool) ?? false
    return !onConsole || ((session["CGSSessionScreenIsLocked"] as? Bool) ?? false)
}

/// Makes an application the one in front, and the window at `bounds` its front window, through
/// Accessibility (ADR 0017: bringing the application's window to the front). An accessory
/// program's own NSRunningApplication.activate() is only a request that macOS may decline.
func bringToFront(_ pid: pid_t, bounds: CGRect?) {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetAttributeValue(app, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    if let bounds {
        var value: CFTypeRef?
        if AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
           let windows = value as? [AXUIElement] {
            for window in windows {
                var position: CFTypeRef?, extent: CFTypeRef?
                AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position)
                AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent)
                var p = CGPoint.zero, s = CGSize.zero
                if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
                if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &s) }
                if abs(p.x - bounds.minX) < 2 && abs(p.y - bounds.minY) < 2 && abs(s.width - bounds.width) < 2 && abs(s.height - bounds.height) < 2 {
                    AXUIElementPerformAction(window, kAXRaiseAction as CFString)
                    AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
                    break
                }
            }
        }
    }
    NSRunningApplication(processIdentifier: pid)?.activate()
}

/// Turns the other device's input into events for the Mac and posts them to one process.
final class Injector {
    /// Where the picture lies on the screen: picture pixels to global points.
    var origin = CGPoint.zero
    var scale = 1.0  // points per picture pixel
    /// The process input goes to now, and the window a pointer event is aimed at.
    var target: () -> pid_t? = { nil }
    var windowAt: (CGPoint) -> WindowInfo? = { _ in nil }
    var mayPost = false
    var log: (String) -> Void = { _ in }

    /// How pointer events are made and posted (`--click-form`, or the word `click-form N`):
    /// 1, 2, 3: to the window's process only (m4-plan.md 5c); 4: through the session, the Mac's
    /// own pointer moves with it (for comparing; never while the screen is locked).
    var clickForm = settings.clickForm
    /// The range input may reach (the owner's rule): whether a point whose top window belongs to
    /// this process is in it, and whether keys may be sent now. Set by the host for its mode.
    var inRange: (CGPoint, WindowInfo?) -> Bool = { _, _ in false }
    var keysAllowed: () -> Bool = { false }
    /// The other side's window became the one typed into; it was resized (picture pixels).
    var onFocus: () -> Void = {}
    var onResize: (Int, Int) -> Void = { _, _ in }
    /// A key was not taken because the application was not in front (said once until one is).
    var onKeysDropped: (Bool) -> Void = { _ in }
    /// A key went down with Command held (a chord that may change windows).
    var onCommandChord: (UInt32) -> Void = { _ in }
    /// After such a chord, keys and text are held (in order, at most 64) until the host has looked
    /// again which window takes the keys: then they all go, or, only if that moved to a window
    /// that is not shown, they are let go of with a notice. Releases of keys already sent always go.
    private(set) var holdingForChord = false
    private var held: [Data] = []
    private var heldPresses: Set<UInt32> = []
    var onHoldFull: () -> Void = {}
    /// With --inline only (inline.swift): composing text, committed text and key presses go to the
    /// inline composition first. Without, `preedit` is unknown here and text goes as before.
    var onPreedit: ((String, [Int]) -> Void)?
    var onButtonPressed: (() -> Void)?
    /// Where events let go of after a chord's hold go: the host's dispatch (set by the host).
    var redispatch: ((Data) -> Void)?
    var onText: ((String) -> Bool)?
    var onKeyPressed: (() -> Void)?
    func beginHold() { holdingForChord = true }
    func endHold(flush: Bool) {
        holdingForChord = false
        let events = held
        held = []
        heldPresses = []
        if flush {
            // Through the host's own order (a commit's wait for "inserted" begun meanwhile holds
            // them too), or straight here when nothing else is in the way.
            for event in events { if let redispatch { redispatch(event) } else { handle(event) } }
        } else if !events.isEmpty {
            counts["held_discarded", default: 0] += events.count
            onKeysDropped(true)
        }
    }
    private var keysDroppedSaid = false
    /// What the injector looks at and where it posts; replaced by stand-ins in --self-test, so that
    /// the tests go through the same path as the input does.
    var listNow: () -> [WindowInfo] = { windowList() }
    var lockedNow: () -> Bool = screenLocked
    /// Closed while the host makes a new stream (a resize): as when locked, only releases pass.
    var gateClosed: () -> Bool = { false }
    var postSession: (CGEvent) -> Void = { $0.post(tap: .cgSessionEventTap) }
    var postProcess: (CGEvent, pid_t) -> Void = { $0.postToPid($1) }
    /// When each modifier key went down (uptime).
    private var modifierSince: [UInt32: TimeInterval] = [:]

    /// The process of the window that is on top at a point of the screen, whatever its layer.
    private func topWindow(at point: CGPoint) -> WindowInfo? {
        listNow().first { $0.bounds.contains(point) && $0.alpha > 0.01 && !isDockCover($0) }
    }

    /// The Dock's window that covers a whole display at the Dock's layer (20), seen on top of every
    /// application window on the Mac mini and the Mac Studio. It does not take clicks: with it on
    /// top, clicks through the session reached the counting window below it (2026-11-19, Studio).
    /// Only that exact window is passed over; any other window on top stops the click.
    private func isDockCover(_ window: WindowInfo) -> Bool {
        guard window.layer == 20,
              NSRunningApplication(processIdentifier: window.pid)?.bundleIdentifier == "com.apple.dock" else { return false }
        var count: UInt32 = 0
        var displays = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(16, &displays, &count)
        return displays.prefix(Int(count)).contains { CGDisplayBounds($0) == window.bounds }
    }
    private let source = CGEventSource(stateID: .privateState)

    /// Presses posted through the session that have not been released yet: the button, and the
    /// bounds of the application's window it was pressed in.
    private var sessionPresses: [UInt32: CGRect] = [:]

    /// Posts a pointer event: to the process, or with form 4 through the session (refused while
    /// the screen is locked: ADR 0017, "守ること" 3), as `sessionStep` decides.
    private func deliver(_ event: CGEvent, to pid: pid_t, window: WindowInfo) {
        guard clickForm == 4 else { return postProcess(event, pid) }
        if lockedNow() {
            releaseSessionPresses()
            return count("dropped_locked_session_tap")
        }
        let button = UInt32(event.getIntegerValueField(.mouseEventButtonNumber))
        let onTarget = inRange(event.location, topWindow(at: event.location))
        switch sessionStep(type: event.type, at: event.location, onTarget: onTarget,
                           pressWindow: sessionPresses[button], window: window.bounds) {
        case let .post(place, remember):
            event.location = offCorners(place)
            switch remember {
            case let .press(bounds): sessionPresses[button] = bounds
            case .release: sessionPresses[button] = nil
            case .nothing: break
            }
            postSession(event)
        case .toProcess:
            postProcess(event, pid)
        case .drop:
            count("dropped_not_on_top")
        }
    }

    /// Releases every press posted through the session, inside the window it was made in. Also
    /// while the screen is locked: a button must never stay down in the session.
    func releaseSessionPresses() {
        for (button, bounds) in sessionPresses {
            let type: CGEventType = button == 0 ? .leftMouseUp : button == 1 ? .rightMouseUp : .otherMouseUp
            let place = clamp(point, into: bounds)
            if let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: place,
                                   mouseButton: CGMouseButton(rawValue: button) ?? .left) {
                postSession(event)
            }
        }
        sessionPresses.removeAll()
    }

    /// Keeps a point posted through the session at least 2 points from every corner of a display:
    /// a corner may start a hot-corner action of the Mac.
    private func offCorners(_ point: CGPoint) -> CGPoint {
        var count: UInt32 = 0
        var displays = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(16, &displays, &count)
        guard let display = displays.prefix(Int(count)).map(CGDisplayBounds).first(where: { $0.contains(point) }) else { return point }
        var p = point
        let nearLeft = p.x < display.minX + 2, nearRight = p.x > display.maxX - 3
        let nearTop = p.y < display.minY + 2, nearBottom = p.y > display.maxY - 3
        if (nearLeft || nearRight) && (nearTop || nearBottom) {
            p.x = nearLeft ? display.minX + 2 : display.maxX - 3
            p.y = nearTop ? display.minY + 2 : display.maxY - 3
        }
        return p
    }

    func setClickForm(_ form: Int) {
        if clickForm == 4 && form != 4 { releaseSessionPresses() }
        if clickForm != 4 && form == 4 { for code in buttonsDown { button(code, false) }; buttonsDown.removeAll() }
        clickForm = form
    }
    private var flags: CGEventFlags = []
    /// When the last input came (uptime): the watchdog lets go of what is held after a silence.
    var lastInput: TimeInterval = 0
    var holding: Bool { !keysDown.isEmpty || !buttonsDown.isEmpty }
    private var keysDown: Set<UInt32> = []
    private var keyDownPostedAt: [UInt32: UInt64] = [:]
    private var buttonsDown: Set<UInt32> = []
    private var point = CGPoint.zero
    var pointerPlace: CGPoint { point }
    private var clicks = 0
    private var lastClick: (time: TimeInterval, point: CGPoint, button: UInt32)?
    private(set) var counts: [String: Int] = [:]

    private func count(_ what: String) { counts[what, default: 0] += 1 }

    func handle(_ data: Data) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { return count("unreadable") }
        lastInput = ProcessInfo.processInfo.systemUptime
        if holdingForChord && (kind == "key" || kind == "text" || kind == "preedit") {
            let code = (object["code"] as? NSNumber)?.uint32Value ?? 0
            let pressed = object["pressed"] as? Bool ?? true
            // The release of a key that was sent before the chord's hold goes at once.
            if !(kind == "key" && !pressed && keysDown.contains(code) && !heldPresses.contains(code)) {
                if kind == "key" && pressed { heldPresses.insert(code) }
                held.append(data)
                count("held")
                if held.count >= 64 { onHoldFull() }
                return
            }
        }
        if !settings.inputDry && (lockedNow() || gateClosed()) {
            // Locked, or not on the console: nothing but the release of what is down here, in every
            // form (ADR 0017, "守ること" 3).
            let pressed = object["pressed"] as? Bool
            let code = (object["code"] as? NSNumber)?.uint32Value ?? 0
            let held = kind == "key" ? keysDown.contains(code) : buttonsDown.contains(code)
            guard passesWhileLocked(kind: kind, pressed: pressed, held: held) else { return count("dropped_locked") }
        }
        if settings.inputDry || !mayPost {
            if settings.inputDry, kind == "text", let text = object["text"] as? String {
                // For checks: how many characters, in which order of pieces; never what.
                counts["dry_text_characters", default: 0] += text.count
                log("{\"kind\":\"text_in\",\"t_ns\":\(nowNs()),\"characters\":\(text.count),\"bytes\":\(text.utf8.count)}")
            }
            return count(settings.inputDry ? "dry_\(kind)" : "dropped_no_permission")
        }
        switch kind {
        case "key":
            guard let code = (object["code"] as? NSNumber)?.uint32Value, let pressed = object["pressed"] as? Bool else { return count("unreadable") }
            key(code, pressed)
        case "pointer_at":
            guard let x = (object["x"] as? NSNumber)?.doubleValue, let y = (object["y"] as? NSNumber)?.doubleValue else { return count("unreadable") }
            moved(to: CGPoint(x: origin.x + x * scale, y: origin.y + y * scale))
        case "button":
            guard let code = (object["code"] as? NSNumber)?.uint32Value, let pressed = object["pressed"] as? Bool else { return count("unreadable") }
            button(code, pressed)
        case "scroll":
            // Bounded before anything is done with them: Int32's least value has no opposite.
            guard let rawX = (object["dx"] as? NSNumber)?.doubleValue, let rawY = (object["dy"] as? NSNumber)?.doubleValue,
                  rawX.isFinite, rawY.isFinite, abs(rawX) <= 10_000, abs(rawY) <= 10_000 else { return count("unreadable") }
            let dx = Int32(rawX), dy = Int32(rawY)
            let numbers = { (key: String) -> [Double]? in
                guard let list = object[key] as? [NSNumber], list.count == 2 else { return nil }
                let values = list.map(\.doubleValue)
                return values.allSatisfy { $0.isFinite && abs($0) <= 120_000 } ? values : nil
            }
            let plan = scrollPlan(dx: dx, dy: dy, source: object["source"] as? String, v120: numbers("v120"), pixels: numbers("pixels"),
                                  phase: object["phase"] as? String, momentum: object["momentum"] as? String,
                                  pointsPerPixel: scale, carry: &scrollCarry)
            scroll(plan)
        case "text":
            guard let text = object["text"] as? String else { return count("unreadable") }
            if let onText {
                guard Wire.acceptable(text, allowEmpty: false) else { return count("text_dropped") }
                guard target() != nil else { return count("dropped_no_target") }
                guard keysAllowed() else { return count("dropped_not_in_front") }
                if onText(text) { count("text_inline") }
            } else {
                type(text)
            }
        case "preedit" where onPreedit != nil:
            guard let text = object["text"] as? String, let cursor = (object["cursor"] as? [NSNumber])?.map(\.intValue) else { return count("unreadable") }
            guard target() != nil, keysAllowed() else { return count("preedit_not_in_front") }
            onPreedit?(text, cursor)
            count("preedit")
        case "focus":
            count("focus")
            onFocus()
        case "resize":
            guard let w = (object["width"] as? NSNumber)?.intValue, let h = (object["height"] as? NSNumber)?.intValue,
                  (64...8192).contains(w), (64...8192).contains(h) else { return count("unreadable") }
            count("resize")
            onResize(w, h)
        default:
            count("unknown")
        }
    }

    private func key(_ code: UInt32, _ pressed: Bool) {
        guard let pid = target() else { return count("dropped_no_target") }
        // A release is always let through for a key that went down, so that none stays down.
        guard keysAllowed() || (!pressed && keysDown.contains(code)) else {
            if !keysDroppedSaid { keysDroppedSaid = true; onKeysDropped(true) }
            return count("dropped_not_in_front")
        }
        if pressed && keysDroppedSaid { keysDroppedSaid = false; onKeysDropped(false) }
        if pressed { onKeyPressed?() }
        guard let virtual = virtualKeys[code] else { return count("unknown_key") }
        if pressed { keysDown.insert(code) } else { keysDown.remove(code) }
        if modifierFlag(code) != nil { modifierSince[code] = pressed ? (modifierSince[code] ?? ProcessInfo.processInfo.systemUptime) : nil }
        flags = modifierFlags(of: keysDown)
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: virtual, keyDown: pressed) else { return }
        withHeld(event)
        // Posted to a process, a modifier key is not turned into a change of the modifiers by
        // anything on the way: it is made one here.
        if modifierFlag(code) != nil { event.type = .flagsChanged }
        if pressed { log("{\"kind\":\"key_in\",\"t_ns\":\(nowNs())}") }
        // Never repeated by anyone: a posted key is one press, however long it is held.
        event.setIntegerValueField(.keyboardEventAutorepeat, value: 0)
        postProcess(event, pid)
        // How long each key was down between our own down and up (times only, never which key).
        let at = nowNs()
        if pressed { keyDownPostedAt[code] = at } else if let down = keyDownPostedAt.removeValue(forKey: code) {
            log("{\"kind\":\"key_hold\",\"t_ns\":\(at),\"ms\":\(String(format: "%.1f", Double(at - down) / 1e6))}")
        }
        if pressed && modifierFlag(code) == nil && flags.contains(.maskCommand) { onCommandChord(code) }
        count("key")
    }

    /// The modifiers held now, in place of the ones the event source set; its other flags (the
    /// keypad's and Fn's, for arrows and function keys) stay.
    private func withHeld(_ event: CGEvent) {
        event.flags = event.flags.subtracting([.maskShift, .maskControl, .maskAlternate, .maskCommand, .maskAlphaShift]).union(flags)
    }

    private func otherHeld(_ flag: CGEventFlags, besides code: UInt32) -> Bool {
        keysDown.contains { $0 != code && modifierFlag($0) == flag }
    }

    private func moved(to place: CGPoint) {
        point = place
        let type: CGEventType
        let button: CGMouseButton
        if buttonsDown.contains(buttonLeft) { type = .leftMouseDragged; button = .left }
        else if buttonsDown.contains(buttonRight) { type = .rightMouseDragged; button = .right }
        else if buttonsDown.contains(buttonMiddle) { type = .otherMouseDragged; button = .center }
        else { type = .mouseMoved; button = .left }
        guard let window = windowAt(place) else { return count("dropped_no_window") }
        let pid = window.pid
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: place, mouseButton: button) else { return }
        withHeld(event)
        aim(event, at: window)
        deliver(event, to: pid, window: window)
        count("move")
    }

    private func aim(_ event: CGEvent, at window: WindowInfo) {
        event.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.id))
        event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.id))
    }

    private func button(_ code: UInt32, _ pressed: Bool) {
        if pressed { freshWindowList(); onButtonPressed?() }
        guard let window = windowAt(point) else { return count("dropped_no_window") }
        let pid = window.pid
        if pressed { buttonsDown.insert(code) } else { buttonsDown.remove(code) }
        if pressed {
            let now = ProcessInfo.processInfo.systemUptime
            if let last = lastClick, last.button == code, now - last.time <= NSEvent.doubleClickInterval,
               abs(last.point.x - point.x) <= 4, abs(last.point.y - point.y) <= 4 {
                clicks += 1
            } else {
                clicks = 1
            }
            lastClick = (now, point, code)
        }
        let nsType: NSEvent.EventType
        let cgType: CGEventType
        let cgButton: CGMouseButton
        switch code {
        case buttonRight: nsType = pressed ? .rightMouseDown : .rightMouseUp; cgType = pressed ? .rightMouseDown : .rightMouseUp; cgButton = .right
        case buttonMiddle: nsType = pressed ? .otherMouseDown : .otherMouseUp; cgType = pressed ? .otherMouseDown : .otherMouseUp; cgButton = .center
        default: nsType = pressed ? .leftMouseDown : .leftMouseUp; cgType = pressed ? .leftMouseDown : .leftMouseUp; cgButton = .left
        }
        var event: CGEvent?
        if clickForm >= 3 {
            // Core Graphics alone, the window named in its fields.
            event = CGEvent(mouseEventSource: source, mouseType: cgType, mouseCursorPosition: point, mouseButton: cgButton)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
            if let event { aim(event, at: window) }
        } else {
            // Form 1 of docs/m4-plan.md 5c: an AppKit event that carries the window's number, its
            // place in the window's own coordinates (from the bottom left), turned into a Core
            // Graphics event. Form 2 also says the place on the screen again afterwards.
            let local = NSPoint(x: point.x - window.bounds.minX, y: window.bounds.maxY - point.y)
            let made = NSEvent.mouseEvent(with: nsType, location: local, modifierFlags: NSEvent.ModifierFlags(rawValue: UInt(flags.rawValue)),
                                          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: Int(window.id), context: nil,
                                          eventNumber: 0, clickCount: clicks, pressure: pressed ? 1 : 0)
            event = made?.cgEvent
            if clickForm == 2 { event?.location = point }
        }
        guard let event else { return count("unmade_click") }
        withHeld(event)
        deliver(event, to: pid, window: window)
        count("button")
    }

    /// What is left over of fractions of pixels or notches, so that nothing is lost by rounding.
    private var scrollCarry: (Double, Double) = (0, 0)

    private func scroll(_ plan: ScrollPlan) {
        freshWindowList()
        guard let window = windowAt(point) else { return count("dropped_no_window") }
        let pid = window.pid
        // The other side says how far to scroll; the Mac, how far the content moves.
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: plan.lines ? .line : .pixel, wheelCount: 2,
                                  wheel1: plan.wheel1, wheel2: plan.wheel2, wheel3: 0) else { return }
        event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: plan.exact1)
        event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2, value: plan.exact2)
        event.setIntegerValueField(.scrollWheelEventIsContinuous, value: plan.continuous ? 1 : 0)
        if plan.phase != 0 { event.setIntegerValueField(.scrollWheelEventScrollPhase, value: plan.phase) }
        if plan.momentum != 0 { event.setIntegerValueField(.scrollWheelEventMomentumPhase, value: plan.momentum) }
        event.location = point
        withHeld(event)
        aim(event, at: window)
        deliver(event, to: pid, window: window)
        count("scroll")
    }

    /// Committed text from the other device's input method (ADR 0007, "確定した文字"): at most 256
    /// characters and 1024 bytes, none of them a control character; else the event is dropped
    /// whole. Never written anywhere: only how many.
    func type(_ text: String) {
        let scalars = text.unicodeScalars
        guard !text.isEmpty, scalars.count <= 256, text.utf8.count <= 1024,
              !scalars.contains(where: { $0.value < 0x20 || (0x7f...0x9f).contains($0.value) }) else {
            return count("text_dropped")
        }
        guard let pid = target() else { return count("dropped_no_target") }
        guard keysAllowed() else { return count("dropped_not_in_front") }
        // A keyboard event carries at most 20 UTF-16 units; whole characters only in each.
        var chunk: [UniChar] = []
        func post() {
            guard !chunk.isEmpty else { return }
            for down in [true, false] {
                guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { continue }
                event.flags = []
                event.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
                postProcess(event, pid)
            }
            chunk.removeAll()
        }
        for character in text {
            let units = Array(String(character).utf16)
            if chunk.count + units.count > 20 { post() }
            chunk += units
        }
        post()
        counts["text", default: 0] += 1
        counts["text_characters", default: 0] += text.count
    }

    /// Lets go of every key and button still held, as the other side would (detach, a stop).
    /// A modifier key down for longer than this is let go, however busy the typing (its release
    /// may have been lost, and every letter would carry it: ⌘Q, ⌘W). The events carry no state of
    /// the other side's modifiers to check against.
    func releaseStaleModifiers(olderThan seconds: TimeInterval) {
        let now = ProcessInfo.processInfo.systemUptime
        for (code, since) in modifierSince where now - since > seconds && keysDown.contains(code) {
            key(code, false)
            count("modifier_let_go")
        }
    }

    /// A Command chord for copy and paste: with the host's own Command as it is now - only the key
    /// when Command is down already, else Command down, the key, Command up. Nothing stays down.
    func postChord(_ code: UInt32) {
        // Through handle(), as any key: the lock, the restart's gate, the chord's hold, a dry run
        // and the front rule all apply.
        let commandDown = keysDown.contains(125) || keysDown.contains(126)
        func send(_ key: UInt32, _ pressed: Bool) {
            if let data = try? JSONSerialization.data(withJSONObject: ["kind": "key", "code": key, "pressed": pressed]) { handle(data) }
        }
        if !commandDown { send(125, true) }
        send(code, true)
        send(code, false)
        if !commandDown { send(125, false) }
    }

    func releaseAll() {
        releaseSessionPresses()
        guard mayPost, !settings.inputDry else { keysDown.removeAll(); buttonsDown.removeAll(); flags = []; return }
        for code in keysDown { key(code, false) }
        for code in buttonsDown { button(code, false) }
        keysDown.removeAll()
        buttonsDown.removeAll()
        flags = []
    }
}

// MARK: - The host

final class DailyHost: NSObject, NSApplicationDelegate {
    var transport: Transport!
    var pump: Pump!
    var capture: Capture?
    var generated: Generated?
    let injector = Injector()
    let lookout = Lookout()
    let clipboard = ClipboardBridge(board: SystemBoard())
    let pasteGate = PasteGate()
    private lazy var gateWired: Void = { injector.gateClosed = { [weak self] in self?.restarting ?? false } }()
    var width = 0, height = 0
    var app: NSRunningApplication?
    var display: SCDisplay?
    var windowId: CGWindowID?
    var windowBounds = CGRect.zero
    var windowCount = -1
    var detached = false
    var picture = "running"  // running, paused
    var offered = true
    var inputSocket: Int32 = -1
    var timers: [DispatchSourceTimer] = []
    var promptMonitor: SystemPromptMonitor?
    var mayCapture = false
    /// The inline composition (--inline only), and the kind 5 said last (told again to a new stream).
    var inline: InlineHost?
    var lastField: FieldReport?
    /// A stop has begun (a signal, or quit): the inline part is not started any more.
    var stopping = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let path = settings.pidFile {
            try? "\(getpid())\n".write(toFile: path, atomically: true, encoding: .utf8)
        }
        // An application that hangs must not hold the input up for seconds.
        AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), 0.25)
        // Locked: everything that is down is let go at once, not at the next event.
        for name in ["com.apple.screenIsLocked", "com.apple.sessionDidMoveOffConsole"] {
            DistributedNotificationCenter.default().addObserver(forName: Notification.Name(name), object: nil, queue: .main) { [weak self] _ in
                self?.injector.releaseAll()
                self?.enforceLock()
            }
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            self?.injector.releaseAll()
        }
        // Something held, and no input for 8 s: a release was lost on the way; let go.
        let watchdog = DispatchSource.makeTimerSource(queue: .main)
        watchdog.schedule(deadline: .now() + 1, repeating: .seconds(1))
        watchdog.setEventHandler { [weak self] in
            self?.keepAwakeAndTellLock()
            self?.manageVirtualDisplay()
            self?.injector.releaseStaleModifiers(olderThan: 8)
            guard let self, self.injector.holding,
                  ProcessInfo.processInfo.systemUptime - self.injector.lastInput > 8 else { return }
            note("input held for 8 s without anything new: everything is let go")
            self.injector.releaseAll()
        }
        watchdog.resume()
        timers.append(watchdog)
        _ = gateWired
        mayCapture = settings.generated || CGPreflightScreenCaptureAccess()
        injector.mayPost = CGPreflightPostEventAccess()
        say("permissions", [("screen_recording", mayCapture), ("accessibility", injector.mayPost)])
        guard mayCapture else {
            say("refused", [("word", "start"), ("reason", "Screen Recording is not granted to this program (System Settings > Privacy & Security > Screen & System Audio Recording); nothing is captured")])
            return quit(3)
        }
        if !injector.mayPost && !settings.inputDry {
            note("Accessibility is not granted to this program (System Settings > Privacy & Security > Accessibility): input is dropped")
        }
        switch settings.target {
        case .display: startDisplay()
        case let .app(name): startApp(name)
        }
    }

    func quit(_ code: Int32) {
        stopping = true
        inline?.shutdown()
        promptMonitor?.stop()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { exit(code) }
    }

    // ---- Starting ----

    func listen(width: Int, height: Int) -> Bool {
        self.width = width
        self.height = height
        guard let made = Transport(settings: settings, width: width, height: height, admitting: admitState) else {
            say("refused", [("word", "start"), ("reason", "cannot listen for paired devices")])
            quit(1)
            return false
        }
        transport = made
        transport.ownCursor()
        if let lastField { transport.setTextInput(lastField) }
        pump = Pump(transport: transport, width: width, height: height)
        pictureWindowSize = nil
        if frameLog.on {
            frameLog.write("{\"kind\":\"settings\",\"t_ns\":\(nowNs()),\"size\":\"\(width)x\(height)\",\"fps\":\(settings.fps),\"bitrate_kbit\":\(settings.bitrateKbit),\"chroma\":\"\(settings.chroma)\",\"keep_awake\":\(settings.keepAwake),\"min_qp\":\(settings.minQP),\"source\":\"\(settings.generated ? "generated" : "screencapturekit")\",\"transport\":\"channel\",\"encoder\":\"videotoolbox\"}")
        }
        startTimers()
        watchCursor()
        startInput()
        startControl()
        watchSignal()
        return true
    }

    func startDisplay() {
        if settings.generated {
            let (w, h) = settings.size ?? (1280, 720)
            guard listen(width: w, height: h) else { return }
            injector.target = { NSWorkspace.shared.frontmostApplication?.processIdentifier }
            injector.windowAt = { _ in nil }  // nothing to point at: the input is only counted
            injector.keysAllowed = { true }
            openPicture { why in self.started(why) }
            return
        }
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
            DispatchQueue.main.async {
                let main = CGMainDisplayID()
                guard let display = content?.displays.first(where: { $0.displayID == main }) ?? content?.displays.first else {
                    say("refused", [("word", "start"), ("reason", "no display may be captured: \(error?.localizedDescription ?? "none listed")")])
                    return self.quit(1)
                }
                self.display = display
                let density = pixelsPerPoint(display.displayID)
                let native = (Int(Double(display.width) * density) & ~1, Int(Double(display.height) * density) & ~1)
                // Not more than 2560 pixels wide unless asked: a 5K display would take the encoder
                // several times as long.
                let capped = native.0 > 2560 ? (2560, Int(Double(native.1) * 2560 / Double(native.0)) & ~1) : native
                let (w, h) = settings.size ?? capped
                guard self.listen(width: w, height: h) else { return }
                // The display keeps its shape in the picture: black bars where the shapes differ.
                let frame = display.frame
                let k = min(Double(w) / frame.width, Double(h) / frame.height)
                let barX = (Double(w) - frame.width * k) / 2, barY = (Double(h) - frame.height * k) / 2
                self.injector.origin = CGPoint(x: frame.minX - barX / k, y: frame.minY - barY / k)
                self.injector.scale = 1 / k
                // Keys go to the application in front; the pointer to the window under it, in its process.
                self.injector.target = {
                    // Never the login window (the lock screen's password field).
                    guard let front = NSWorkspace.shared.frontmostApplication, front.bundleIdentifier != "com.apple.loginwindow" else { return nil }
                    return front.processIdentifier
                }
                // The whole display is the range: any application.
                self.injector.inRange = { _, _ in true }
                self.injector.keysAllowed = { true }
                self.injector.windowAt = { [weak self] point in self?.topWindow(at: point) }
                self.openPicture { why in self.started(why) }
            }
        }
    }

    /// In the whole-display form: the process of the window last under the pointer gets the pointer;
    /// a press makes it the application in front, so that its keys go there too.
    var pointerPid: pid_t?

    func topWindow(at point: CGPoint) -> WindowInfo? {
        var found = windowList().first { window in
            window.bounds.contains(point) && window.alpha > 0.05 && window.layer >= 0 && window.layer < 1000 && window.layer != 20
        }
        if found == nil {
            // The desktop: its icons are the Finder's.
            let finder = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.finder").first?.processIdentifier
            found = windowList(desktop: true).first { $0.pid == finder && $0.bounds.contains(point) }
        }
        if case .display = settings.target {
            pointerPid = found?.pid
        }
        return found
    }

    func startApp(_ name: String) {
        guard let url = applicationURL(name) else {
            say("refused", [("word", "start"), ("reason", "no application called \(name) was found")])
            return quit(1)
        }
        // The picture's size: as asked (and the window is made that size), or else the window's
        // own size in its screen's pixels, found when it is first seen (one picture pixel for one
        // pixel of the screen: no scaling, and the viewer's window has the window's shape).
        if let size = settings.size {
            guard listen(width: size.0, height: size.1) else { return }
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        configuration.addsToRecentItems = false
        // For this launch only (the argument domain of its defaults; never written): no windows of
        // last time come back with a new document, so no one else's old document opens.
        if settings.noRestore { configuration.arguments = ["-ApplePersistenceIgnoreState", "YES"] }
        let opened: (NSRunningApplication?, Error?) -> Void = { running, error in
            DispatchQueue.main.async {
                guard let running else {
                    say("refused", [("word", "start"), ("reason", "the application did not open: \(error?.localizedDescription ?? "unknown")")])
                    return self.quit(1)
                }
                self.app = running
                say("opened", [("pid", Int(running.processIdentifier)), ("bundle", running.bundleIdentifier ?? "")])
                self.injector.target = { [weak self] in self?.app?.processIdentifier }
                // Every window of the application is the range (its menus, pop-ups, sheets, other
                // windows); the open and save panel, shown by a process of the system's own for it,
                // while the application is in front. Keys only while it is in front.
                self.injector.inRange = { [weak self] _, top in
                    guard let self, let pid = self.app?.processIdentifier, let top else { return false }
                    // The application's own windows; of the panel service, only the very window
                    // that is the application's panel.
                    return top.pid == pid || self.appPanels().contains { $0.id == top.id }
                }
                self.injector.keysAllowed = { [weak self] in
                    guard let self else { return false }
                    // The shown window must also be the application's front window in the window list (a
                    // window opened at exactly the same place does not pass for it).
                    let pid = self.app?.processIdentifier
                    let front = windowList().first { $0.pid == pid && $0.layer == 0 && $0.onScreen && $0.bounds.width > 40 && $0.bounds.height > 40 }?.id
                    return keysMayGo(appInFront: self.appInFront(), focused: self.lookout.focused, shown: self.windowBounds,
                                     shownIsFront: front != nil && front == self.windowId)
                }
                self.injector.onFocus = { [weak self] in self?.raiseShownWindow() }
                self.injector.onCommandChord = { [weak self] code in
                    // Editing chords pass straight on; others are held for at most 30 ms while the
                    // window list (WindowServer, which answers at once) is looked at every 5 ms for
                    // a new front window of the application. None: the keys go. A new one that is
                    // not the shown window: they are let go of, with the notice.
                    guard let self, let pid = self.app?.processIdentifier, chordMayChangeWindow(code), !self.injector.holdingForChord else { return }
                    self.injector.beginHold()
                    let shown = self.windowId
                    var looks = 0
                    func look() {
                        guard self.injector.holdingForChord else { return }
                        looks += 1
                        let front = readWindowList(onScreenOnly: true, desktop: false).first { $0.pid == pid && $0.layer == 0 && $0.bounds.width > 40 && $0.bounds.height > 40 }?.id
                        if let front, front != shown {
                            self.lookout.invalidateFocused()
                            self.lookout.refreshFocused(pid)
                            return self.injector.endHold(flush: false)
                        }
                        if looks >= 6 { return self.injector.endHold(flush: true) }
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.005) { look() }
                    }
                    self.injector.onHoldFull = { self.injector.endHold(flush: true) }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.005) { look() }
                }
                self.injector.onResize = { [weak self] w, h in self?.resizeAsked(width: w, height: h) }
                self.injector.onKeysDropped = { dropped in say(dropped ? "keys-dropped" : "keys-taken") }
                self.injector.windowAt = { [weak self] point in self?.appWindow(at: point) }
                self.watchFocus(running.processIdentifier)
                self.watchApp()
                self.startInline(running)
            }
        }
        // The windows the application has before this open: with a new document only the window
        // that comes with it is shown, never one the application had open already (another
        // stream's document, the owner's own).
        if settings.document != nil, let bundle = Bundle(url: url)?.bundleIdentifier,
           let running = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first {
            windowsAtOpen = Set(windowList(onScreenOnly: false).filter { $0.pid == running.processIdentifier }.map(\.id))
        } else if settings.document != nil {
            windowsAtOpen = []
        }
        if let document = settings.document {
            // With a document of the trial's own, so that the application opens no panel of its
            // own at the start and none of the owner's documents.
            let file = URL(fileURLWithPath: (document as NSString).expandingTildeInPath)
            NSWorkspace.shared.open([file], withApplicationAt: url, configuration: configuration, completionHandler: opened)
        } else {
            NSWorkspace.shared.openApplication(at: url, configuration: configuration, completionHandler: opened)
        }
    }

    /// The windows of the system's open and save panel that are the application's own (see
    /// panelAllowed), on the screen now.
    func appPanels() -> [WindowInfo] {
        guard let pid = app?.processIdentifier else { return [] }
        let services = windowList().filter { isPanelService($0.pid) && $0.layer >= 0 && $0.alpha > 0.05 }
        guard !services.isEmpty else { return [] }
        // What Accessibility found last (never waited for here), and a fresh look asked for.
        lookout.refresh(pid)
        let own = lookout.windows
        let front = appInFront()
        return services.filter { panelAllowed(serviceWindow: $0.bounds, appWindows: own, appInFront: front) }
    }

    /// The application and the window that is shown, to the front (the other side's window became
    /// the one typed into, or a first key comes): through Accessibility, away from the input.
    var raisedWindow: CGWindowID?
    func raiseShownWindow() {
        guard let pid = app?.processIdentifier, windowBounds != .zero else { return }
        let bounds = windowBounds
        raisedWindow = windowId
        lookout.act { bringToFront(pid, bounds: bounds) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { self.lookout.refreshFocused(pid) }
    }

    /// The other side asks for another size (once its resizing has ended). The shown window of
    /// the application is made that size, as near as the application and the screen let it
    /// (Accessibility); the answer is a new stream of the size taken, also when nothing changed.
    func resizeAsked(width: Int, height: Int) {
        guard settings.size == nil, let pid = app?.processIdentifier, windowBounds != .zero, !restarting else { return }
        // The request is in picture pixels; the window is sized in points of the display it is on
        // (2 pixels a point on the virtual display), and kept inside that display's visible area.
        let bounds = windowBounds
        let density = densityAt(bounds)
        let mainTop = NSScreen.screens.first?.frame.maxY ?? 0
        let screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: bounds.midX, y: mainTop - bounds.midY)) } ?? NSScreen.main
        let visible = screen.map { CGRect(x: $0.visibleFrame.minX, y: mainTop - $0.visibleFrame.maxY, width: $0.visibleFrame.width, height: $0.visibleFrame.height) }
            ?? CGRect(x: 0, y: 0, width: 8192, height: 8192)
        var target = CGSize(width: Double(width) / density, height: Double(height) / density)
        target.width = min(target.width, visible.maxX - bounds.minX); target.height = min(target.height, visible.maxY - bounds.minY)
        let restartsBefore = restarts
        lookout.act {
            let element = AXUIElementCreateApplication(pid)
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value) == .success,
                  let windows = value as? [AXUIElement] else { return }
            for window in windows {
                var position: CFTypeRef?, extent: CFTypeRef?
                AXUIElementCopyAttributeValue(window, kAXPositionAttribute as CFString, &position)
                AXUIElementCopyAttributeValue(window, kAXSizeAttribute as CFString, &extent)
                var p = CGPoint.zero, s = CGSize.zero
                if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
                if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &s) }
                guard abs(p.x - bounds.minX) < 2, abs(p.y - bounds.minY) < 2,
                      abs(s.width - bounds.width) < 2, abs(s.height - bounds.height) < 2 else { continue }
                var size = target
                if let made = AXValueCreate(.cgSize, &size) { AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, made) }
                break
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                // The answer: a stream of the size the window has now (the look that follows the
                // window would also notice a change; this answers when nothing changed as well).
                guard let self, let now = windowList(onScreenOnly: true).first(where: { $0.id == self.windowId }) else { return }
                // The answer in pixels of the display the window is on now.
                let taken = densityAt(now.bounds)
                let w = max(64, min(3840, Int(now.bounds.width * taken)) & ~1), h = max(64, min(2160, Int(now.bounds.height * taken)) & ~1)
                // One answer only: when the window's own change already made a new stream, that is it.
                if !self.restarting && self.restarts == restartsBefore { self.restartStream(width: w, height: h, windowSize: now.bounds.size) }
            }
        }
    }

    /// Which window takes the keys is looked up again only when it may have changed: the
    /// application's focused or main window changed (Accessibility notifications), or another
    /// application came to the front, and after a raise. Never on a keystroke: each key uses the
    /// answer found last (the input never waits on Accessibility).
    var focusObserver: AXObserver?
    func watchFocus(_ pid: pid_t) {
        lookout.refreshFocused(pid)
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in
            self?.lookout.refreshFocused(pid)
        }
        let callback: AXObserverCallback = { _, _, _, refcon in
            guard let refcon else { return }
            let host = Unmanaged<DailyHost>.fromOpaque(refcon).takeUnretainedValue()
            if let pid = host.app?.processIdentifier { host.lookout.refreshFocused(pid) }
        }
        var observer: AXObserver?
        guard AXObserverCreate(pid, callback, &observer) == .success, let observer else {
            return note("the application's focus cannot be watched: keys wait for a click")
        }
        let element = AXUIElementCreateApplication(pid)
        let me = Unmanaged.passUnretained(self).toOpaque()
        // A move or a resize of the window also changes the place keys are checked against.
        for name in [kAXFocusedWindowChangedNotification, kAXMainWindowChangedNotification,
                     kAXWindowMovedNotification, kAXWindowResizedNotification] {
            AXObserverAddNotification(observer, element, name as CFString, me)
        }
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .defaultMode)
        focusObserver = observer
    }

    func appInFront() -> Bool {
        guard let pid = app?.processIdentifier, let front = NSWorkspace.shared.frontmostApplication else { return false }
        return front.processIdentifier == pid
    }

    /// The window of the application under the point (its menus and sheets are windows of its own
    /// process too); the window that is shown when there is none there. Never another process's.
    func appWindow(at point: CGPoint) -> WindowInfo? {
        guard let pid = app?.processIdentifier else { return nil }
        // The application's own panel first: it lies over its windows. A press on it keeps its
        // drag and release inside it.
        if let panel = appPanels().first(where: { $0.bounds.contains(point) }) { return panel }
        let mine = windowList().filter { $0.pid == pid && $0.alpha > 0.05 }
        return mine.first { $0.bounds.contains(point) } ?? mine.first { $0.id == windowId }
    }

    /// Looks at the application's windows every 200 ms: which one is shown, where it is, how many.
    func watchApp() {
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now(), repeating: .milliseconds(200))
        timer.setEventHandler { [weak self] in self?.lookAtApp() }
        timer.resume()
        timers.append(timer)
    }

    func lookAtApp() {
        guard let app else { return }
        if app.isTerminated {
            for timer in timers { timer.cancel() }
            timers.removeAll()
            self.app = nil
            say("empty")
            stopPicture(offered: false) {}
            return quit(0)
        }
        let all = windowList(onScreenOnly: false).filter { $0.pid == app.processIdentifier && $0.layer == 0 && $0.bounds.width > 40 && $0.bounds.height > 40 }
        if settings.newWindow {
            if windowsAtOpen == nil { windowsAtOpen = Set(all.map(\.id)) }
            if !askedNewWindow, appInFront(), injector.mayPost {
                // Command-N, to the application's process only.
                askedNewWindow = true
                for (key, down) in [(UInt16(0x37), true), (UInt16(0x2d), true), (UInt16(0x2d), false), (UInt16(0x37), false)] {
                    if let event = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: down) {
                        event.flags = down || key == 0x2d ? .maskCommand : []
                        if key == 0x37 { event.type = .flagsChanged }
                        event.postToPid(app.processIdentifier)
                    }
                }
            }
            // Until the new window is there, nothing is shown.
            guard all.contains(where: { !(windowsAtOpen?.contains($0.id) ?? true) }) else { return }
        }
        if all.count != windowCount {
            windowCount = all.count
            say("windows", [("count", all.count)])
        }
        var shown = all.filter { $0.onScreen && $0.alpha > 0.05 }
        // With a new window or a new document, only windows that came after the start are shown.
        if settings.newWindow || settings.document != nil, let before = windowsAtOpen { shown = shown.filter { !before.contains($0.id) } }
        // Keep the window shown so far while it is there; else the largest one.
        let chosen = shown.first { $0.id == windowId } ?? shown.max { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
        guard let chosen else { return }
        if transport == nil {
            let density = densityAt(chosen.bounds)
            let w = min(3840, Int(chosen.bounds.width * density)) & ~1, h = min(2160, Int(chosen.bounds.height * density)) & ~1
            guard listen(width: max(64, w), height: max(64, h)) else { return }
        }
        if chosen.id != windowId {
            windowId = chosen.id
            // Which window is shown (its number only): the close path checks it is gone before
            // the trial's own document is deleted.
            say("shown", [("window", Int(chosen.id))])
            windowBounds = .zero
            if settings.fitWindow && settings.size != nil { fit(chosen) }
        }
        // A panel of the system's for the application came or went: placed again, with it.
        // Every window of the service on the screen, and which are allowed: any change places the
        // capture again.
        let services = windowList().filter { isPanelService($0.pid) }.map { Int($0.id) }.sorted()
        let panelShown = services + [-1] + appPanels().map { Int($0.id) }.sorted()
        followSize(chosen)
        if restarting { return }
        // Locked: nothing of the application is shown (the lock's privacy). The program is ready,
        // the other side says that the Mac is locked, and the picture begins once it is unlocked.
        if screenLocked() {
            if transport != nil && !started { started(nil) }
            return
        }
        if chosen.bounds != windowBounds || panelShown != self.panelShown {
            self.panelShown = panelShown
            windowBounds = chosen.bounds
            place(chosen)
        }
    }

    /// Makes the window as large as the picture, in points of its screen, where it fits (ADR 0017:
    /// moving and sizing the application's window needs Accessibility).
    func fit(_ window: WindowInfo) {
        guard injector.mayPost, AXIsProcessTrusted(), let pid = app?.processIdentifier else { return }
        let screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: window.bounds.midX, y: NSScreen.screens[0].frame.maxY - window.bounds.midY)) } ?? NSScreen.main
        guard let screen else { return }
        let scale = screen.backingScaleFactor
        var size = CGSize(width: Double(width) / scale, height: Double(height) / scale)
        let room = screen.visibleFrame.size
        let shrink = min(1, room.width / size.width, room.height / size.height)
        size = CGSize(width: (size.width * shrink).rounded(.down), height: (size.height * shrink).rounded(.down))
        // The Accessibility calls themselves away from the input (an application may hang).
        let visibleFrame = screen.visibleFrame, mainTop = NSScreen.screens[0].frame.maxY
        lookout.act {
            let element = AXUIElementCreateApplication(pid)
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value) == .success,
                  let windows = value as? [AXUIElement] else { return }
            for candidate in windows {
                var position: CFTypeRef?, extent: CFTypeRef?
                AXUIElementCopyAttributeValue(candidate, kAXPositionAttribute as CFString, &position)
                AXUIElementCopyAttributeValue(candidate, kAXSizeAttribute as CFString, &extent)
                var p = CGPoint.zero, s = CGSize.zero
                if let position { AXValueGetValue(position as! AXValue, .cgPoint, &p) }
                if let extent { AXValueGetValue(extent as! AXValue, .cgSize, &s) }
                guard abs(p.x - window.bounds.minX) < 2, abs(p.y - window.bounds.minY) < 2,
                      abs(s.width - window.bounds.width) < 2, abs(s.height - window.bounds.height) < 2 else { continue }
                // Keep it on its screen: its top left moves up and left as far as needed.
                let visible = visibleFrame
                let top = mainTop - visible.maxY
                var origin = p
                origin.x = max(visible.minX, min(origin.x, visible.maxX - size.width))
                origin.y = max(top, min(origin.y, top + visible.height - size.height))
                var newOrigin = origin
                var newSize = size
                if let place = AXValueCreate(.cgPoint, &newOrigin) { AXUIElementSetAttributeValue(candidate, kAXPositionAttribute as CFString, place) }
                if let extent = AXValueCreate(.cgSize, &newSize) { AXUIElementSetAttributeValue(candidate, kAXSizeAttribute as CFString, extent) }
                return
            }
        }
    }

    /// Points the capture at the window's place: the part of its screen with the picture's shape,
    /// from the window's top left, at the screen's own density when the window fits.
    func place(_ window: WindowInfo) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, _ in
            DispatchQueue.main.async {
                let center = CGPoint(x: window.bounds.midX, y: window.bounds.midY)
                guard let content, let app = self.app,
                      let display = content.displays.first(where: { $0.frame.contains(center) }) ?? content.displays.first,
                      let scApp = content.applications.first(where: { $0.processID == app.processIdentifier }) else {
                    self.windowBounds = .zero  // looked at again on the next round
                    return
                }
                // The window fills the picture (its shape kept); the part of the display read is the
                // picture's shape, from the window's top left, kept inside the display.
                let W = Double(self.width), H = Double(self.height)
                var fit = min(W / window.bounds.width, H / window.bounds.height)
                var rect = CGRect(x: window.bounds.minX - display.frame.minX, y: window.bounds.minY - display.frame.minY,
                                  width: W / fit, height: H / fit)
                let room = CGSize(width: display.frame.width, height: display.frame.height)
                if rect.width > room.width || rect.height > room.height {
                    fit = max(W / room.width, H / room.height)
                    rect.size = CGSize(width: W / fit, height: H / fit)
                }
                rect.origin.x = max(0, min(rect.origin.x, room.width - rect.width))
                rect.origin.y = max(0, min(rect.origin.y, room.height - rect.height))
                self.injector.origin = CGPoint(x: rect.minX + display.frame.minX, y: rect.minY + display.frame.minY)
                self.injector.scale = 1 / fit
                self.scApplication = scApp
                self.captureRect = rect
                let configuration = Capture.configuration(width: self.width, height: self.height, sourceRect: rect)
                // The system's open and save panel is another process's: in the picture while the
                // application is in front (the same range as the input).
                let allowed = self.appPanels()
                let shownPanels = Set(allowed.map(\.pid))
                let allowedIds = Set(allowed.map(\.id))
                let panels = content.applications.filter { shownPanels.contains($0.processID) }
                let included = [scApp] + panels
                _ = included
                // Nothing of the service by default. While the application's panel shows, the
                // filter names windows one by one: the application's own and exactly the allowed
                // panel windows; a new window of the service is never in it.
                let filter: SCContentFilter
                if allowedIds.isEmpty {
                    filter = SCContentFilter(display: display, including: [scApp], exceptingWindows: [])
                    self.panelWindows = []
                } else {
                    let own = content.windows.filter { $0.owningApplication?.processID == scApp.processID }.map(\.windowID)
                    let service = content.windows.filter { $0.owningApplication.map { shownPanels.contains($0.processID) } ?? false }.map(\.windowID)
                    let named = Set(capturedWindows(appWindows: own, serviceWindows: service, allowed: allowedIds))
                    let windows = content.windows.filter { named.contains($0.windowID) }
                    filter = SCContentFilter(display: display, including: windows)
                    self.panelWindows = windows
                }
                let names = [Int(scApp.processID)] + allowedIds.map { Int($0) }.sorted()
                self.panelApplications = panels
                if let capture = self.capture, self.display?.displayID == display.displayID {
                    if names != self.includedPids { capture.update(filter) }
                    capture.update(configuration)
                } else if let capture = self.capture {
                    self.display = display
                    capture.update(filter)
                    capture.update(configuration)
                } else {
                    self.display = display
                    if self.picture == "running" && !self.opening {
                        self.openPicture { why in self.started(why) }
                    }
                }
                self.includedPids = names
                if frameLog.on {
                    frameLog.write("{\"kind\":\"window\",\"t_ns\":\(nowNs()),\"points\":\"\(Int(window.bounds.width))x\(Int(window.bounds.height))\",\"scale\":\(String(format: "%.3f", fit))}")
                }
            }
        }
    }

    /// The part of the display the application's picture is read from (its points, from its top
    /// left); set by place().
    var captureRect: CGRect?
    /// The processes the capture includes now, and the panel processes among them.
    var includedPids: [Int] = []
    var panelShown: [Int] = []
    var panelApplications: [SCRunningApplication] = []
    /// While the application's panel shows: the windows the capture names, one by one.
    var panelWindows: [SCWindow] = []

    var started = false
    func started(_ why: String?) {
        if let why {
            say("refused", [("word", "start"), ("reason", why)])
            return quit(1)
        }
        guard !started else { return }
        started = true
        say("ready", [("port", settings.port), ("size", "\(width)x\(height)")])
    }

    // ---- The picture ----

    /// Opens the encoder and starts capturing (on the main thread).
    var opening = false
    func openPicture(resumeAsked: UInt64? = nil, done: @escaping (String?) -> Void) {
        // Never while locked: the picture waits for the unlock.
        if screenLocked() && !settings.generated {
            if picture == "running" { picture = "paused" }
            pausedForLock = true
            return done(nil)
        }
        opening = true
        let done: (String?) -> Void = { why in self.opening = false; done(why) }
        pump.queue.async {
            if let why = self.pump.open() {
                return DispatchQueue.main.async { done("no encoder could be opened: \(why)") }
            }
            if let asked = resumeAsked { self.pump.resumed(asked: asked) }
            DispatchQueue.main.async {
                if settings.generated {
                    let source = Generated(width: self.width, height: self.height)
                    self.generated = source
                    source.start(on: self.pump.queue, fps: settings.fps) { [weak self] buffer, at in self?.pump.picture(buffer, displayNs: at) }
                    return done(nil)
                }
                guard let display = self.display else { return done(nil) }  // the app's window comes later
                let capture = Capture(queue: self.pump.queue) { [weak self] buffer, at in self?.pump.picture(buffer, displayNs: at) }
                capture.onStop = { why in note("the capture stopped: \(why)") }
                let filter: SCContentFilter
                var rect: CGRect?
                if self.app != nil {
                    // Only the application's windows; other applications are not in the picture.
                    guard let scApp = self.scApplication, let placed = self.captureRect else { return done(nil) }  // placed later
                    filter = self.panelWindows.isEmpty
                        ? SCContentFilter(display: display, including: [scApp], exceptingWindows: [])
                        : SCContentFilter(display: display, including: self.panelWindows)
                    rect = placed
                } else {
                    filter = SCContentFilter(display: display, excludingWindows: [])
                }
                self.capture = capture
                capture.start(filter: filter, configuration: Capture.configuration(width: self.width, height: self.height, sourceRect: rect), done: { why in
                    DispatchQueue.main.async { done(why.map { "the capture did not start: \($0)" }) }
                })
            }
        }
    }

    /// The application as ScreenCaptureKit lists it; filled in by place(), on the main thread.
    var scApplication: SCRunningApplication?

    /// Stops capturing and gives the encoder back (on the main thread).
    func stopPicture(offered: Bool, done: @escaping () -> Void) {
        let asked = nowNs()
        if !offered { transport?.offer(false) }
        self.offered = offered
        injector.releaseAll()
        generated?.stop()
        generated = nil
        let capture = self.capture
        self.capture = nil
        let finish: () -> Void = {
            guard let pump = self.pump else { return done() }
            pump.queue.async {
                self.pump.close()
                let took = Double(nowNs() - asked) / 1e6
                if frameLog.on { frameLog.write("{\"kind\":\"picture\",\"t_ns\":\(nowNs()),\"state\":\"paused\",\"offered\":\(offered ? 1 : 0),\"took_ms\":\(String(format: "%.3f", took))}") }
                say("paused", [("offered", offered), ("took_ms", took)])
                DispatchQueue.main.async(execute: done)
            }
        }
        if let capture { capture.stop(done: finish) } else { finish() }
    }

    /// A resume that came while the picture was stopping.
    var again = false
    func stopped() {
        picture = "paused"
        if again {
            again = false
            if screenLocked() { pausedForLock = true } else { word("resume") }
        }
    }

    /// Said by the lock itself (not by whoever runs the session): a pause or a resume that must not
    /// clear what the controller said.
    var lockWord = false
    func sayForLock(_ line: String) {
        lockWord = true
        word(line)
        lockWord = false
    }

    /// The lock, enforced at every look (not only when it changes): locked, nothing of the
    /// application is captured, and a resume that was on its way waits for the unlock.
    func enforceLock() {
        guard transport != nil else { return }
        let locked = screenLocked()
        if locked {
            if picture == "stopping" && again { again = false; pausedForLock = true }
            if picture == "running" { pausedForLock = true; sayForLock("pause") }
        } else if lockResumes(pausedForLock: pausedForLock, detached: detached, held: heldForSignal) && picture == "paused" {
            pausedForLock = false
            if app != nil { windowBounds = .zero }
            sayForLock("resume")
        }
    }

    // ---- Words ----

    func startControl() {
        guard let path = settings.control else { return }
        // Read and write: the pipe never ends when a writer goes.
        let fd = open(path, O_RDWR | O_NONBLOCK)
        guard fd >= 0 else { return note("cannot open the control pipe \(path)") }
        var text = ""
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in
            var buffer = [UInt8](repeating: 0, count: 4096)
            let n = read(fd, &buffer, buffer.count)
            guard n > 0 else { return }
            text += String(decoding: buffer[0..<n], as: UTF8.self)
            while let end = text.firstIndex(of: "\n") {
                let line = text[..<end].trimmingCharacters(in: .whitespaces)
                text.removeSubrange(...end)
                self?.word(line)
            }
        }
        source.resume()
        controlSource = source
    }
    var controlSource: DispatchSourceRead?

    func word(_ line: String) {
        if restarting, !line.isEmpty {
            // What closes is done at once (nobody is shown anything, nothing more is passed on);
            // the rest is said to the new stream, in order.
            if line.hasPrefix("detach") {
                detached = true
                offered = false
                injector.releaseAll()
            }
            if line.hasPrefix("admit") { injector.releaseAll() }
            return wordsDuringRestart.append(line)
        }
        let words = line.split(separator: " ").map(String.init)
        switch words.first {
        case "pause":
            // An explicit pause wins over the lock's own: an unlock does not undo it.
            if !lockWord { pausedForLock = false }
            again = false
            if picture == "stopping" { return }  // answered when it has stopped
            guard picture == "running" else { return say("paused", [("offered", offered), ("took_ms", 0.0)]) }
            picture = "stopping"
            stopPicture(offered: true) { self.stopped() }
        case "detach":
            if !lockWord { pausedForLock = false }
            detached = true
            again = false
            if picture != "running" {
                transport.offer(false)
                offered = false
                if picture == "paused" { say("paused", [("offered", false), ("took_ms", 0.0)]) }
                return
            }
            picture = "stopping"
            stopPicture(offered: false) { self.stopped() }
        case "resume":
            guard !heldForSignal else {
                return say("refused", [("word", "resume"), ("reason", "a latency measurement runs on this Mac; the picture comes back when it is over")])
            }
            // Locked: nothing of the application is shown; resumed once unlocked (unless a pause or
            // a detach comes before).
            guard !screenLocked() else {
                pausedForLock = true
                return say("refused", [("word", "resume"), ("reason", "the Mac is locked; the picture comes back once it is unlocked")])
            }
            detached = false
            transport.offer(true)
            offered = true
            if picture == "stopping" { again = true; return }  // resumed once it has stopped
            guard picture == "paused" else { return }
            picture = "running"
            let asked = nowNs()
            if app != nil { windowBounds = .zero }  // looked at again: places the capture anew
            openPicture(resumeAsked: asked) { why in
                if let why {
                    self.picture = "paused"
                    return say("refused", [("word", "resume"), ("reason", why)])
                }
            }
        case "click-form":
            // For comparing the ways of posting pointer events while a person tries them.
            if words.count == 2, let form = Int(words[1]), (1...4).contains(form) {
                injector.setClickForm(form)
                say("click-form", [("form", form)])
            }
        case "admit":
            if words.count == 3 {
                let ok = transport.admit(words[1], words[2])
                admitState = ok ? .only(id: words[1], key: words[2]) : .nobody
                say("admitted", [("device", ok ? words[1] : nil)])
            } else {
                _ = transport.admit(nil, nil)
                admitState = .nobody
                say("admitted", [("device", nil)])
            }
        case "close":
            // As the Quit of its menu: an application with nothing to lose ends; one with unsaved
            // work asks, and stays. Nothing is ended by force.
            let asked = app?.terminate() ?? false
            say("close", [("asked", asked)])
        case nil:
            return
        default:
            note("'\(line)' is not something this program can be told")
        }
    }

    // ---- Input ----

    func startInput() {
        guard let path = settings.inputSocket else { return }
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_DGRAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { return note("the input socket's path is too long") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0 else { return note("cannot take input on \(path): \(String(cString: strerror(errno)))") }
        chmod(path, 0o600)
        _ = fcntl(fd, F_SETFL, O_NONBLOCK)
        // Room for a burst: a release that does not fit would be lost, and a key would stay down.
        var room: Int32 = 1 << 20
        setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &room, socklen_t(MemoryLayout<Int32>.size))
        inputSocket = fd
        injector.log = { line in frameLog.write(line) }
        if settings.clipboard {
            clipboard.postChord = { [weak self] code in self?.injector.postChord(code) }
            clipboard.keysAllowed = { [weak self] in self?.injector.keysAllowed() ?? false }
            clipboard.targetPid = { [weak self] in self?.injector.target() }
            clipboard.lookout = lookout
            if !clipboard.open(beside: path) { note("the clipboard socket could not be opened: copy and paste stay off") }
        }
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in
            var buffer = [UInt8](repeating: 0, count: 4096)
            while true {
                let n = recv(fd, &buffer, buffer.count, 0)
                guard n > 0 else { break }
                // Delivered only while the picture is shown (ADR 0017, "守ること" 1).
                guard let self, !self.detached, self.picture == "running" else { continue }
                self.take(Data(buffer[0..<n]))
            }
        }
        source.resume()
        inputSource = source
    }
    var inputSource: DispatchSourceRead?

    /// One input event. An application that is not in front has no key window, and takes no
    /// keys: a key, a press or committed text first makes the application the one in front
    /// (in the whole-display form, the one under the pointer for a press), and the events wait
    /// 60 ms for that, in their order.
    var waiting: [Data] = []
    func take(_ data: Data) {
        // A paste that waits for its text holds everything after it, in order (0.2 s at most).
        if pasteGate.waitingFor != nil { return pasteGate.hold(data) }
        // Behind whatever waits for a raise, in order: a copy right after a select-all goes after it.
        if !waiting.isEmpty { return waiting.append(data) }
        let notRaised = app != nil && raisedWindow != windowId && isKeyOrText(data)
        if needsFront(data), !screenLocked(), let running = frontCandidate(data),
           notRaised || running.processIdentifier != NSWorkspace.shared.frontmostApplication?.processIdentifier {
            if app != nil && isKeyOrText(data) { raisedWindow = windowId }
            // The window under the pointer for a press, else the application's shown window.
            let isPress = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["kind"] as? String == "button"
            let bounds = isPress ? injector.windowAt(injector.pointerPlace)?.bounds : (app != nil ? windowBounds : nil)
            let pid = running.processIdentifier
            lookout.act { bringToFront(pid, bounds: bounds) }
            waiting = [data]
            // The raise is looked at again before anything is sent: the keys go only if it took.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { self.lookout.refreshFocused(pid) }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                let events = self.waiting
                self.waiting = []
                for event in events where !self.detached && self.picture == "running" {
                    if self.pasteGate.waitingFor != nil { self.pasteGate.hold(event) } else { self.dispatch(event) }
                }
            }
            return
        }
        dispatch(data)
    }

    /// One event in its turn: a mark of copy or paste to the clipboard's part, anything else to the
    /// injector. After a commit through the input source, everything waits (in order) until it is
    /// inserted (inline.swift; 50 ms at most).
    func dispatch(_ data: Data) {
        if let inline, inline.hold(data) { return }
        dispatchNow(data)
    }

    /// One event that waits for nothing any more.
    func dispatchNow(_ data: Data) {
        if settings.clipboard, let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let kind = object["kind"] as? String, kind == "clipboard_copy" || kind == "clipboard_paste",
           let nonce = object["nonce"] as? String {
            guard !detached, picture == "running" else { return }
            if kind == "clipboard_copy" { return clipboard.copy(nonce: nonce, cut: object["cut"] as? Bool ?? false) }
            let withText = object["with_text"] as? Bool ?? false
            if withText && clipboard.textFor(nonce) == nil {
                pasteGate.begin(nonce, now: ProcessInfo.processInfo.systemUptime)
                func look() {
                    let now = ProcessInfo.processInfo.systemUptime
                    if clipboard.textFor(nonce) != nil {
                        clipboard.paste(nonce: nonce, withText: true)
                    } else if !pasteGate.expired(now: now) {
                        return DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { look() }
                    } else {
                        clipboard.answer(nonce, "none", what: "paste")
                    }
                    for event in pasteGate.end() { take(event) }
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { look() }
                return
            }
            return clipboard.paste(nonce: nonce, withText: withText)
        }
        injector.handle(data)
    }

    // ---- Composing text inline (--inline only; inline.swift) ----

    func startInline(_ running: NSRunningApplication) {
        // A stop that came before the application opened: nothing is started any more.
        guard !stopping else { return }
        guard settings.inline else {
            // Off. Only if an earlier run with --inline ended without putting the source back (its
            // file is still there) is that source put back now; otherwise nothing is touched.
            if let path = Wire.socketPath(slot: settings.virtualSlot) {
                let folder = (path as NSString).deletingLastPathComponent
                if access((folder as NSString).appendingPathComponent("previous-source"), F_OK) == 0,
                   let restored = SourceSwitch(folder: folder, slot: settings.virtualSlot, system: TISInputSources()).recoverAtStart() {
                    note("inline is off; a source left by an earlier run, restored: \(restored ? "yes" : "no")")
                }
            }
            return
        }
        guard let bundle = running.bundleIdentifier, let made = InlineHost(app: bundle, slot: settings.virtualSlot, idle: settings.inlineIdle) else {
            return note("inline: no bundle identifier or no socket path: the strip only")
        }
        made.composer.deliverText = { [weak self] text in self?.injector.type(text) }
        made.composer.setField = { [weak self] field in
            self?.lastField = field
            self?.transport?.setTextInput(field)
            if frameLog.on { frameLog.write("{\"kind\":\"field\",\"t_ns\":\(nowNs()),\"state\":\(field.state),\"serial\":\(field.serial)}") }
        }
        made.composer.releaseHold = { [weak self] in self?.flushHeld() }
        made.composer.densityAtWindow = { [weak self] in densityAt(self?.windowBounds ?? .zero) }
        guard made.start() else { return note("inline: the slot's socket could not be made: the strip only") }
        injector.onPreedit = { made.composer.preedit($0, cursor: $1) }
        injector.onText = { made.composer.commit($0) }
        injector.onKeyPressed = { made.composer.keyPressed() }
        injector.onButtonPressed = { made.viewerPressed() }
        injector.redispatch = { [weak self] event in self?.dispatch(event) }
        made.pid = { [weak self] in self?.app?.processIdentifier }
        inline = made
        note("inline: on, slot \(settings.virtualSlot)")
        var ticks = 0
        var inlineShownAt = -1000.0
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + 0.1, repeating: .milliseconds(10))
        timer.setEventHandler { [weak self] in
            guard let self, let inline = self.inline else { return }
            ticks += 1
            if ticks % 10 != 0 { if inline.composer.holding { inline.composer.tick() }; return }
            let composer = inline.composer
            composer.shownWindow = self.windowBounds
            composer.origin = self.injector.origin
            composer.pointsPerPixel = self.injector.scale
            composer.picture = CGSize(width: self.width, height: self.height)
            let now = ProcessInfo.processInfo.systemUptime
            let viewers = self.transport?.viewers ?? 0
            if viewers > 0 { inlineShownAt = now }
            let shown = self.transport != nil && inlineViewerShown(viewers: viewers, restarting: self.restarting, sinceShown: now - inlineShownAt)
            let connected = shown && self.picture == "running" && !self.detached && !self.heldForSignal
            inline.tick(connected: connected, locked: screenLocked(), pid: self.app?.processIdentifier)
            if ticks % 100 == 0, frameLog.on, !composer.counts.isEmpty {
                let counts = composer.counts.map { "\"\($0.key)\":\($0.value)" }.joined(separator: ",")
                frameLog.write("{\"kind\":\"inline\",\"t_ns\":\(nowNs()),\(counts)}")
            }
        }
        timer.resume()
        timers.append(timer)
    }

    /// What waited for an "inserted", in order, until something waits again.
    func flushHeld() {
        guard let inline else { return }
        while !inline.held.isEmpty && !inline.composer.holding {
            let event = inline.held.removeFirst()
            guard !detached, picture == "running" else { continue }
            dispatchNow(event)
        }
    }

    private func isKeyOrText(_ data: Data) -> Bool {
        let kind = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["kind"] as? String
        return kind == "key" || kind == "text" || kind == "clipboard_copy" || kind == "clipboard_paste"
    }

    private func needsFront(_ data: Data) -> Bool {
        guard injector.mayPost, !settings.inputDry,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { return false }
        switch kind {
        case "key", "text", "clipboard_copy", "clipboard_paste": return true
        case "button": return object["pressed"] as? Bool == true
        default: return false
        }
    }

    private func frontCandidate(_ data: Data) -> NSRunningApplication? {
        if let app { return app }
        guard let pid = pointerPid else { return nil }
        let isPress = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["kind"] as? String == "button"
        return isPress ? NSRunningApplication(processIdentifier: pid) : nil
    }

    // ---- Timers ----

    /// The signal of a latency measurement on this Mac (AGENTS.md, "計測の合図"), read every two
    /// seconds: while it says busy the picture is stopped and the encoder given back, nobody is
    /// shown anything and no input is passed; when it is free again the picture comes back. The
    /// file is only read.
    func watchSignal() {
        let owners = settings.promptOwners
        if !settings.generated, !owners.isEmpty {
            promptMonitor = SystemPromptMonitor {
                let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [NSDictionary]
                return windows.map {
                    systemPromptCount($0, owners: owners) { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier } > 0
                }
            }
        }
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now(), repeating: .seconds(2))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let first = (try? String(contentsOfFile: settings.signal, encoding: .utf8))?
                .split(whereSeparator: { $0 == " " || $0 == "\n" }).first.map(String.init)
            let busy = first == "busy"
            if busy {
                self.promptMonitor?.stop()
            } else {
                self.promptMonitor?.start()
            }
            if busy && !self.heldForSignal {
                self.heldForSignal = true
                note("a latency measurement runs on this Mac: the picture stops until it is over")
                self.resumeAfterSignal = self.picture == "running"
                self.word("detach")
            } else if !busy && self.heldForSignal {
                self.heldForSignal = false
                if self.resumeAfterSignal {
                    note("the measurement is over: the picture comes back")
                    self.word("resume")
                }
            }
        }
        timer.resume()
        timers.append(timer)
    }
    var heldForSignal = false

    /// The virtual display of the application form, and the place each window had before it was
    /// moved there (to put it back).
    var virtualScreen: VirtualScreen?
    var windowsAtOpen: Set<CGWindowID>?
    var askedNewWindow = false
    var makingVirtual = false
    var virtualFailedAt: TimeInterval = -1000
    var movedFrom: [CGWindowID: CGRect] = [:]

    /// Looked at every second: while a device is shown the picture (and the Mac is not locked),
    /// the shown window is on the virtual display; once nobody has been shown it for the time a
    /// viewer may take to come back, the window goes back and the display away.
    func manageVirtualDisplay() {
        guard let spec = settings.virtualDisplay, app != nil, transport != nil, windowId != nil else { return }
        let now = ProcessInfo.processInfo.systemUptime
        let viewed = transport.viewers > 0 || settings.virtualNow
        if viewed && !screenLocked() && virtualScreen == nil && !makingVirtual && now - virtualFailedAt > 30 {
            makingVirtual = true
            let screen = VirtualScreen(width: spec.width, height: spec.height, hertz: spec.hertz, slot: settings.virtualSlot)
            screen.make { frame in
                self.makingVirtual = false
                guard let frame else {
                    // Tried again only after a while, not every second.
                    self.virtualFailedAt = ProcessInfo.processInfo.systemUptime
                    return note("the virtual display could not be made: the window stays where it is")
                }
                self.virtualScreen = screen
                if frameLog.on { frameLog.write("{\"kind\":\"virtual\",\"t_ns\":\(nowNs()),\"state\":\"made\",\"points\":\"\(Int(frame.width))x\(Int(frame.height))\"}") }
                say("virtual", [("state", "made"), ("points", "\(Int(frame.width))x\(Int(frame.height))")])
                self.moveShownWindow(to: frame)
            }
        } else if let screen = virtualScreen, !viewed, now - lastViewed > settings.virtualKeep {
            // The windows first (the moves are queued for Accessibility), the display after them.
            virtualScreen = nil
            moveWindowsBack(on: screen)
            lookout.queue.async {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    screen.remove()
                    say("virtual", [("state", "removed")])
                }
            }
        }
    }

    /// The shown window onto the virtual display (its top left, as large as fits); its place
    /// before is kept.
    func moveShownWindow(to frame: CGRect) {
        guard let pid = app?.processIdentifier, let id = windowId, windowBounds != .zero else { return }
        let before = windowBounds
        if movedFrom[id] == nil { movedFrom[id] = before }
        let target = fitInto(CGRect(x: frame.minX, y: frame.minY + 30, width: before.width, height: before.height),
                             CGRect(x: frame.minX, y: frame.minY + 30, width: frame.width, height: frame.height - 30))
        lookout.act { setWindowFrame(pid, matching: before, to: target) }
        if settings.inline {
            // Seen on the Studio (2026-11-19 21:37 and 21:46): moving the window onto the virtual
            // display made WindowServer resign its key appearance ("for space"), the input source
            // was deactivated and never activated again. With the application in front, the moved
            // window is raised and made main again, so that it is key and its field active.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                guard let self, self.appInFront() else { return }
                self.lookout.act { bringToFront(pid, bounds: target) }
            }
        }
    }

    /// Every moved window back to where it was; a place that is on no display now is fitted into
    /// the visible area of the main display.
    func moveWindowsBack(on screen: VirtualScreen? = nil) {
        guard let pid = app?.processIdentifier else { return }
        let screen = screen ?? virtualScreen
        let current = windowList(onScreenOnly: false).filter { $0.pid == pid }
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(16, &ids, &count)
        let virtualId = screen?.id
        let displays = ids.prefix(Int(count)).filter { $0 != virtualId }.map(CGDisplayBounds)
        let main = NSScreen.screens.first
        let mainTop = main?.frame.maxY ?? 0
        let visible = main.map { CGRect(x: $0.visibleFrame.minX, y: mainTop - $0.visibleFrame.maxY, width: $0.visibleFrame.width, height: $0.visibleFrame.height) }
            ?? CGDisplayBounds(CGMainDisplayID())
        for (id, original) in movedFrom {
            guard let now = current.first(where: { $0.id == id }) else { continue }
            let back = onSomeDisplay(original, displays: displays) ? original : fitInto(original, visible)
            lookout.act { setWindowFrame(pid, matching: now.bounds, to: back) }
        }
        movedFrom.removeAll()
    }

    /// While a device is shown the picture, the Mac's display does not sleep (and so does not lock
    /// from idleness): an assertion of this program, let go 10 s after the last viewer left
    /// (m4-plan.md 4.5b). No permission, no setting changed.
    var awake: IOPMAssertionID = 0
    var lastViewed: TimeInterval = 0
    var wasLocked: Bool?

    var pausedForLock = false
    func keepAwakeAndTellLock() {
        let now = ProcessInfo.processInfo.systemUptime
        let viewed = transport != nil && transport.viewers > 0
        if viewed { lastViewed = now }
        if viewed && awake == 0 {
            var id: IOPMAssertionID = 0
            if IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                           "Istesso: a device is shown a session" as CFString, &id) == kIOReturnSuccess {
                awake = id
                var activity: IOPMAssertionID = 0
                IOPMAssertionDeclareUserActivity("Istesso: a device is shown a session" as CFString, kIOPMUserActiveLocal, &activity)
                note("the display is kept awake while a device is shown the picture")
            }
        } else if !viewed && awake != 0 && now - lastViewed > 10 {
            IOPMAssertionRelease(awake)
            awake = 0
            note("nobody is shown the picture: the display may sleep again")
        }
        // Said when the screen locks or unlocks, for the window on the other side to show.
        let locked = screenLocked()
        if locked != wasLocked {
            wasLocked = locked
            say(locked ? "locked" : "unlocked")
        }
        enforceLock()
    }

    /// The Mac's cursor, as large as the host's screen looks in the viewer's window, and of the
    /// shape it has now (an I-beam over text, for one); looked at ten times a second.
    func watchCursor() {
        guard !settings.generated else { return }
        var told: Int?
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + 0.2, repeating: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            guard let self, self.transport != nil else { return }
            let cursor = NSCursor.currentSystem ?? NSCursor.arrow
            _ = self.transport.setCursor(cursor, pixelsPerPoint: 1 / self.injector.scale, told: &told)
        }
        timer.resume()
        timers.append(timer)
    }
    var resumeAfterSignal = false

    func startTimers() {
        let tick = DispatchSource.makeTimerSource(flags: .strict, queue: pump.queue)
        tick.schedule(deadline: .now(), repeating: .milliseconds(4), leeway: .microseconds(500))
        tick.setEventHandler { [weak self] in self?.pump.tick() }
        tick.resume()
        let second = DispatchSource.makeTimerSource(queue: pump.queue)
        second.schedule(deadline: .now() + 1, repeating: .seconds(1))
        second.setEventHandler { [weak self] in
            self?.pump.second()
            DispatchQueue.main.async {
                guard let self, frameLog.on, !self.injector.counts.isEmpty else { return }
                let counts = self.injector.counts.map { "\"\($0.key)\":\($0.value)" }.joined(separator: ",")
                frameLog.write("{\"kind\":\"input\",\"t_ns\":\(nowNs()),\(counts)}")
            }
        }
        second.resume()
        pumpTimers = [tick, second]
    }
    var pumpTimers: [DispatchSourceTimer] = []
    /// Who is shown the picture, as last said; a new stream (after a resize) starts with it.
    var admitState: AdmitState = settings.admitNobody ? .nobody : .paired
    /// Words that came while a new stream was being made: said to the new one, in order.
    var wordsDuringRestart: [String] = []

    // ---- A new size: the application's window was resized ----

    /// The window's size (points) the picture was made for, in the form that follows the window.
    var pictureWindowSize: CGSize?
    var resizeSeen: (size: CGSize, at: TimeInterval)?
    var restarting = false

    /// Looked at with every look at the application: a window that has had another size for
    /// 150 ms gets a stream of that size (the viewer connects again, and is told the new size).
    func followSize(_ window: WindowInfo) {
        guard settings.size == nil, transport != nil, !restarting else { return }
        let size = window.bounds.size
        // By pixels: a window moved onto a display of another density (the virtual one) needs a
        // picture of another size too.
        let density = densityAt(window.bounds)
        let wantW = max(64, min(3840, Int(size.width * density)) & ~1), wantH = max(64, min(2160, Int(size.height * density)) & ~1)
        if wantW == width && wantH == height { resizeSeen = nil; return }
        let now = ProcessInfo.processInfo.systemUptime
        guard let seen = resizeSeen, abs(seen.size.width - size.width) < 2, abs(seen.size.height - size.height) < 2 else {
            resizeSeen = (size, now)
            return
        }
        guard now - seen.at >= 0.15 else { return }
        resizeSeen = nil
        restartStream(width: wantW, height: wantH, windowSize: size)
    }

    /// Stops the picture, gives the encoder back, ends the stream, and starts one of the new size on
    /// the same port; the picture comes back as it was (running or not).
    var restarts = 0
    func restartStream(width w: Int, height h: Int, windowSize: CGSize) {
        restarting = true
        restarts += 1
        let old = transport!
        let oldPump = pump!
        for timer in pumpTimers { timer.cancel() }
        pumpTimers = []
        let capture = self.capture
        self.capture = nil
        let afterCapture = {
            oldPump.queue.async {
                oldPump.close()
                // The encoder's last callbacks have long finished after 0.2 s; only then is the old
                // stream ended (nobody may use it afterwards) and its port taken again.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                    old.close()
                    // The words that came meanwhile decide who may look and whether it is offered
                    // BEFORE the new stream listens; it is never offered to anybody it should not be.
                    let queued = self.wordsDuringRestart
                    let (admit, offer, gone) = stateAfter(words: queued, admit: self.admitState, offered: self.offered,
                                                          detached: self.detached, held: self.heldForSignal)
                    self.admitState = admit
                    guard let made = Transport(settings: settings, width: w, height: h, admitting: admit, offered: offer && !gone) else {
                        // The old stream is closed: every call on it does nothing now; the program ends.
                        say("refused", [("word", "resize"), ("reason", "cannot listen again for paired devices")])
                        self.restarting = false
                        return self.quit(1)
                    }
                    self.width = w
                    self.height = h
                    self.transport = made
                if let field = self.lastField { made.setTextInput(field) }
                    made.ownCursor()
                    self.pump = Pump(transport: made, width: w, height: h)
                    self.pictureWindowSize = windowSize
                    self.startTimers()
                    if frameLog.on { frameLog.write("{\"kind\":\"resized\",\"t_ns\":\(nowNs()),\"size\":\"\(w)x\(h)\"}") }
                    say("resized", [("size", "\(w)x\(h)")])
                    self.windowBounds = .zero  // placed again for the new size
                    self.restarting = false
                    // Decided now, not on what was true before: the words that came meanwhile are
                    // said first.
                    let words = self.wordsDuringRestart
                    self.wordsDuringRestart = []
                    let resumes = restartResumes(picture: self.picture, detached: self.detached, offered: self.offered,
                                                 held: self.heldForSignal, wordsMeanwhile: words)
                    for word in words { self.word(word) }
                    if resumes && self.picture == "running" && !self.detached && self.offered && !self.heldForSignal {
                        self.openPicture { why in if let why { note("after the resize: \(why)") } }
                    }
                }
            }
        }
        if let capture { capture.stop(done: afterCapture) } else { afterCapture() }
    }
}

// MARK: - Start

var signalSources: [DispatchSourceSignal] = []
signal(SIGPIPE, SIG_IGN)
let application = NSApplication.shared
application.setActivationPolicy(.accessory)
let daily = DailyHost()
application.delegate = daily
for name in [SIGTERM, SIGINT] {
    signal(name, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: name, queue: .main)
    source.setEventHandler {
        daily.injector.releaseAll()
        // Istesso's input source is let go of before anything else (the Mac's own keyboard).
        daily.stopping = true
        daily.inline?.shutdown()
        // A stop: the windows go back where they were and the virtual display away, then the end.
        if daily.virtualScreen != nil {
            daily.moveWindowsBack()
            daily.lookout.queue.async {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    daily.virtualScreen?.remove()
                    exit(0)
                }
            }
        } else {
            exit(0)
        }
    }
    source.resume()
    signalSources.append(source)
}
application.run()
