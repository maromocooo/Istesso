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
//            posted to the application's own process only (CGEventPostToPid; ADR 0017, "守ること"
//            3). The session tap is never used.
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
import ScreenCaptureKit
import VideoToolbox

// MARK: - Settings

enum Target {
    case display
    case app(String)
}

struct Settings {
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
    var signal = NSHomeDirectory() + "/.local/state/istesso/measuring"
}

func usage() -> Never {
    FileHandle.standardError.write(Data("""
    usage: istesso-daily-host [--port N] [--bind ADDR] [--config-dir DIR] [--admit nobody]
           [--display | --app NAME|BUNDLE-ID|PATH] [--size WxH] [--fps N] [--chroma 444|420]
           [--bitrate KBIT] [--keep-awake FPS] [--min-qp N] [--input-socket PATH] [--input-dry]
           [--control FIFO] [--log FILE|-] [--no-fit-window] [--click-form 1|2|3] [--generated]
           [--pid-file FILE] [--signal FILE]

    """.utf8))
    exit(2)
}

func readSettings() -> Settings {
    var s = Settings()
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
        default: usage()
        }
    }
    return s
}

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

final class Transport {
    let host: OpaquePointer
    init?(settings: Settings, width: Int, height: Int) {
        var info = IstessoMediaStreamInfo(codec: 1, width: UInt32(width), height: UInt32(height),
                                          fps: UInt32(settings.fps), chroma: settings.chroma, colour: 1)
        let bind = "\(settings.bind):\(settings.port)"
        let made: OpaquePointer?
        if settings.admitNobody {
            made = istesso_media_host_listen_closed(settings.configDir, bind, &info)
        } else {
            made = istesso_media_host_listen(settings.configDir, bind, &info)
        }
        guard let made else {
            let why = istesso_media_last_error().map { String(cString: $0) } ?? "unknown"
            note("cannot listen for paired devices: \(why)")
            return nil
        }
        host = made
    }
    var viewers: Int { Int(istesso_media_host_viewers(host)) }
    func keyFrameWanted() -> Bool { istesso_media_host_key_frame_wanted(host) != 0 }
    func offer(_ on: Bool) { istesso_media_host_offer(host, on ? 1 : 0) }
    func admit(_ id: String?, _ key: String?) -> Bool { istesso_media_host_admit(host, id, key) == 0 }
    func send(_ data: [UInt8], pts: UInt64, encoded: UInt64, key: Bool) -> IstessoMediaSent? {
        var sent = IstessoMediaSent()
        let result = data.withUnsafeBufferPointer {
            istesso_media_host_send(host, $0.baseAddress, $0.count, pts, encoded, key ? 1 : 0, &sent)
        }
        return result == 0 ? sent : nil
    }
    /// The viewers draw the Mac's own cursor where their pointer is: the picture has none, and the
    /// host's pointer does not move (input goes to the application's process, not to the screen).
    func ownCursor() {
        "default".withCString { name in
            var cursor = IstessoMediaCursor(shape: 1, width: 0, height: 0, hotspot_x: 0, hotspot_y: 0, scale: 1,
                                            pixels: nil, pixels_len: 0, name: name)
            _ = istesso_media_host_set_cursor(host, &cursor)
        }
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
func windowList(onScreenOnly: Bool = true) -> [WindowInfo] {
    let options: CGWindowListOption = onScreenOnly ? [.optionOnScreenOnly, .excludeDesktopElements] : [.optionAll, .excludeDesktopElements]
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

    private let source = CGEventSource(stateID: .privateState)
    private var flags: CGEventFlags = []
    private var keysDown: Set<UInt32> = []
    private var buttonsDown: Set<UInt32> = []
    private var point = CGPoint.zero
    private var clicks = 0
    private var lastClick: (time: TimeInterval, point: CGPoint, button: UInt32)?
    private(set) var counts: [String: Int] = [:]

    private func count(_ what: String) { counts[what, default: 0] += 1 }

    func handle(_ data: Data) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { return count("unreadable") }
        if settings.inputDry || !mayPost {
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
            guard let dx = (object["dx"] as? NSNumber)?.int32Value, let dy = (object["dy"] as? NSNumber)?.int32Value else { return count("unreadable") }
            scroll(dx: dx, dy: dy)
        case "text":
            guard let text = object["text"] as? String else { return count("unreadable") }
            type(text)
        default:
            count("unknown")
        }
    }

    private func key(_ code: UInt32, _ pressed: Bool) {
        guard let pid = target() else { return count("dropped_no_target") }
        guard let virtual = virtualKeys[code] else { return count("unknown_key") }
        if let flag = modifierFlag(code) {
            if pressed { flags.insert(flag) } else if !otherHeld(flag, besides: code) { flags.remove(flag) }
        }
        if pressed { keysDown.insert(code) } else { keysDown.remove(code) }
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: virtual, keyDown: pressed) else { return }
        withHeld(event)
        // Posted to a process, a modifier key is not turned into a change of the modifiers by
        // anything on the way: it is made one here.
        if modifierFlag(code) != nil { event.type = .flagsChanged }
        if pressed { log("{\"kind\":\"key_in\",\"t_ns\":\(nowNs())}") }
        event.postToPid(pid)
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
        event.postToPid(pid)
        count("move")
    }

    private func aim(_ event: CGEvent, at window: WindowInfo) {
        event.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.id))
        event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.id))
    }

    private func button(_ code: UInt32, _ pressed: Bool) {
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
        if settings.clickForm == 3 {
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
            if settings.clickForm == 2 { event?.location = point }
        }
        guard let event else { return count("unmade_click") }
        withHeld(event)
        event.postToPid(pid)
        count("button")
    }

    private func scroll(dx: Int32, dy: Int32) {
        guard let window = windowAt(point) else { return count("dropped_no_window") }
        let pid = window.pid
        // The other side says how far to scroll; the Mac, how far the content moves.
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: -dy, wheel2: -dx, wheel3: 0) else { return }
        event.location = point
        withHeld(event)
        aim(event, at: window)
        event.postToPid(pid)
        count("scroll")
    }

    /// Committed text from the other device's input method (ADR 0007, "確定した文字"): at most 256
    /// characters and 1024 bytes, none of them a control character; else the event is dropped
    /// whole. Never written anywhere: only how many.
    private func type(_ text: String) {
        let scalars = text.unicodeScalars
        guard !text.isEmpty, scalars.count <= 256, text.utf8.count <= 1024,
              !scalars.contains(where: { $0.value < 0x20 || (0x7f...0x9f).contains($0.value) }) else {
            return count("text_dropped")
        }
        guard let pid = target() else { return count("dropped_no_target") }
        // A keyboard event carries at most 20 UTF-16 units; whole characters only in each.
        var chunk: [UniChar] = []
        func post() {
            guard !chunk.isEmpty else { return }
            for down in [true, false] {
                guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { continue }
                event.flags = []
                event.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
                event.postToPid(pid)
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
    func releaseAll() {
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
    var mayCapture = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let path = settings.pidFile {
            try? "\(getpid())\n".write(toFile: path, atomically: true, encoding: .utf8)
        }
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
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { exit(code) }
    }

    // ---- Starting ----

    func listen(width: Int, height: Int) -> Bool {
        self.width = width
        self.height = height
        guard let made = Transport(settings: settings, width: width, height: height) else {
            say("refused", [("word", "start"), ("reason", "cannot listen for paired devices")])
            quit(1)
            return false
        }
        transport = made
        transport.ownCursor()
        pump = Pump(transport: transport, width: width, height: height)
        if frameLog.on {
            frameLog.write("{\"kind\":\"settings\",\"t_ns\":\(nowNs()),\"size\":\"\(width)x\(height)\",\"fps\":\(settings.fps),\"bitrate_kbit\":\(settings.bitrateKbit),\"chroma\":\"\(settings.chroma)\",\"keep_awake\":\(settings.keepAwake),\"min_qp\":\(settings.minQP),\"source\":\"\(settings.generated ? "generated" : "screencapturekit")\",\"transport\":\"channel\",\"encoder\":\"videotoolbox\"}")
        }
        startTimers()
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
                self.injector.target = { NSWorkspace.shared.frontmostApplication?.processIdentifier }
                self.injector.windowAt = { [weak self] point in self?.topWindow(at: point) }
                self.openPicture { why in self.started(why) }
            }
        }
    }

    /// In the whole-display form: the process of the window last under the pointer gets the pointer;
    /// a press makes it the application in front, so that its keys go there too.
    var pointerPid: pid_t?

    func topWindow(at point: CGPoint) -> WindowInfo? {
        let found = windowList().first { window in
            window.bounds.contains(point) && window.alpha > 0.05 && window.layer >= 0 && window.layer < 1000 && window.layer != 20
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
        guard listen(width: settings.size?.0 ?? 1720, height: settings.size?.1 ?? 720) else { return }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        configuration.addsToRecentItems = false
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { running, error in
            DispatchQueue.main.async {
                guard let running else {
                    say("refused", [("word", "start"), ("reason", "the application did not open: \(error?.localizedDescription ?? "unknown")")])
                    return self.quit(1)
                }
                self.app = running
                say("opened", [("pid", Int(running.processIdentifier)), ("bundle", running.bundleIdentifier ?? "")])
                self.injector.target = { [weak self] in self?.app?.processIdentifier }
                self.injector.windowAt = { [weak self] point in self?.appWindow(at: point) }
                self.watchApp()
            }
        }
    }

    /// The window of the application under the point (its menus and sheets are windows of its own
    /// process too); the window that is shown when there is none there. Never another process's.
    func appWindow(at point: CGPoint) -> WindowInfo? {
        guard let pid = app?.processIdentifier else { return nil }
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
        if all.count != windowCount {
            windowCount = all.count
            say("windows", [("count", all.count)])
        }
        let shown = all.filter { $0.onScreen && $0.alpha > 0.05 }
        // Keep the window shown so far while it is there; else the largest one.
        let chosen = shown.first { $0.id == windowId } ?? shown.max { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
        guard let chosen else { return }
        if chosen.id != windowId {
            windowId = chosen.id
            windowBounds = .zero
            if settings.fitWindow { fit(chosen) }
        }
        if chosen.bounds != windowBounds {
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
            let visible = screen.visibleFrame
            let top = NSScreen.screens[0].frame.maxY - visible.maxY
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
                let filter = SCContentFilter(display: display, including: [scApp], exceptingWindows: [])
                if let capture = self.capture, self.display?.displayID == display.displayID {
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
                if frameLog.on {
                    frameLog.write("{\"kind\":\"window\",\"t_ns\":\(nowNs()),\"points\":\"\(Int(window.bounds.width))x\(Int(window.bounds.height))\",\"scale\":\(String(format: "%.3f", fit))}")
                }
            }
        }
    }

    /// The part of the display the application's picture is read from (its points, from its top
    /// left); set by place().
    var captureRect: CGRect?

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
                    filter = SCContentFilter(display: display, including: [scApp], exceptingWindows: [])
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
            word("resume")
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
        let words = line.split(separator: " ").map(String.init)
        switch words.first {
        case "pause":
            again = false
            if picture == "stopping" { return }  // answered when it has stopped
            guard picture == "running" else { return say("paused", [("offered", offered), ("took_ms", 0.0)]) }
            picture = "stopping"
            stopPicture(offered: true) { self.stopped() }
        case "detach":
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
        case "admit":
            if words.count == 3 {
                let ok = transport.admit(words[1], words[2])
                say("admitted", [("device", ok ? words[1] : nil)])
            } else {
                _ = transport.admit(nil, nil)
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
        inputSocket = fd
        injector.log = { line in frameLog.write(line) }
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
        if !waiting.isEmpty { return waiting.append(data) }
        if needsFront(data), let running = frontCandidate(data),
           running.processIdentifier != NSWorkspace.shared.frontmostApplication?.processIdentifier {
            running.activate()
            waiting = [data]
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) {
                let events = self.waiting
                self.waiting = []
                for event in events where !self.detached && self.picture == "running" { self.injector.handle(event) }
            }
            return
        }
        injector.handle(data)
    }

    private func needsFront(_ data: Data) -> Bool {
        guard injector.mayPost, !settings.inputDry,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { return false }
        switch kind {
        case "key", "text": return true
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
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now(), repeating: .seconds(2))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let first = (try? String(contentsOfFile: settings.signal, encoding: .utf8))?
                .split(whereSeparator: { $0 == " " || $0 == "\n" }).first.map(String.init)
            let busy = first == "busy"
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
        timers += [tick, second]
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
        exit(0)
    }
    source.resume()
    signalSources.append(source)
}
application.run()
