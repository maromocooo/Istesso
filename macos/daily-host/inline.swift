// Composing text inline in the relayed application, through the trial input source Istesso Dev
// Input (ADR 0023; the input source and its wire are macos/input-method/). TRIAL ONLY, and OFF
// unless the host is started with --inline (or ISTESSO_DAILY_INLINE=1): without it nothing here
// runs, no socket is made, no input source is touched and no field is announced, so the viewer
// keeps its strip as before.
//
// What it does, in the application form only:
//   - owns this slot's socket (a folder of its own, 0700, in this user's temporary folder; the
//     socket 0600) and takes only the input source, checked by its audit token and code signature
//     (Wire.checkPeer: same user, same leaf certificate, its identifier, hardened runtime);
//   - says "hello" with the relayed application's bundle identifier, then turns the viewer's
//     composing text (`preedit` events) into mark / commit / clear with the host's own composition
//     serials, and holds what comes after a commit until the input source says "inserted" (50 ms
//     at most; timeouts are counted);
//   - selects Istesso's source for as long as a viewer is shown the picture, except while this
//     Mac's own keyboard is typing (the HID state's time since the last key: no Input Monitoring),
//     and puts the source before it back on disconnect, on a stop, at the next start after a crash;
//   - goes inline only after the input source says "active" about the relayed application and the
//     caret is inside the shown window; else the viewer's strip (kind 5 state 0);
//   - never loses a character: a refused commit goes by today's text path, a refused mark puts
//     that composition on the strip, a stale one (ended already) is dropped;
//   - says what the field is with kind 5 (ADR 0015): 2 for AXSecureTextField, 3 only after 100 ms
//     of "not a text field" (and no active input-source session), 1 inline, 0 otherwise.
//
// Never written anywhere: the text (composing or committed), its hashes, other applications'
// identifiers, the identifiers of the owner's input sources. Logs hold counts and states only.
import AppKit
import ApplicationServices
import Carbon

// MARK: - The input sources (selected, put back)

/// The system's input sources, behind a protocol so that the self-test never touches the real ones.
protocol InputSourceSystem: AnyObject {
    /// The keyboard input source selected now.
    func currentID() -> String?
    /// Selects an enabled source; true when the system says it is the current one afterwards.
    func select(_ id: String) -> Bool
    /// The input method a source belongs to (its bundle), so that the modes of one input method
    /// (kana, Roman) count as one choice; a keyboard layout is its own.
    func family(_ id: String) -> String
}

/// The real input sources (Text Input Sources). Used only by a host started with --inline.
final class TISInputSources: InputSourceSystem {
    private func property<T>(_ source: TISInputSource, _ key: CFString) -> T? {
        guard let raw = TISGetInputSourceProperty(source, key) else { return nil }
        return Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? T
    }

    func currentID() -> String? {
        guard let current = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else { return nil }
        return property(current, kTISPropertyInputSourceID)
    }

    private func source(_ id: String) -> TISInputSource? {
        let filter = [kTISPropertyInputSourceID as String: id] as CFDictionary
        return (TISCreateInputSourceList(filter, false)?.takeRetainedValue() as? [TISInputSource])?.first
    }

    func select(_ id: String) -> Bool {
        guard let source = source(id) else { return false }
        _ = TISSelectInputSource(source)
        return currentID() == id
    }

    func family(_ id: String) -> String {
        guard let source = source(id), let bundle: String = property(source, kTISPropertyBundleID) else { return id }
        return bundle
    }
}

/// Which source to put back, shared by the hosts of all slots (the selected source is the Mac's,
/// not a slot's): the source before Istesso's is written once, by the first slot that selects
/// Istesso's, and put back by the last one that lets go. Each slot that holds says so in a file of
/// its own with its process number, so that a slot that crashed does not hold forever. Everything
/// is in the socket folder (0700), under a lock.
final class SourceSwitch {
    static let ours = Wire.inputMethodIdentifier
    let folder: String
    let slot: Int
    let system: InputSourceSystem
    /// Whether a process number is a running process (a stand-in in the self-test).
    var alive: (pid_t) -> Bool = { kill($0, 0) == 0 || errno == EPERM }
    var pid: pid_t = getpid()
    private(set) var holding = false
    /// The source this slot put back last (kept in memory only, never written down or logged):
    /// a source the owner picks by hand afterwards is respected until the next connection.
    private(set) var restoredTo: String?

    init(folder: String, slot: Int, system: InputSourceSystem) {
        self.folder = folder
        self.slot = slot
        self.system = system
    }

    var previousFile: String { (folder as NSString).appendingPathComponent("previous-source") }
    private func holdFile(_ slot: Int) -> String { (folder as NSString).appendingPathComponent("hold-slot-\(slot)") }

    private func locked<T>(_ work: () -> T) -> T {
        let fd = open((folder as NSString).appendingPathComponent("switch.lock"), O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0o600)
        if fd >= 0 { flock(fd, LOCK_EX) }
        defer { if fd >= 0 { flock(fd, LOCK_UN); close(fd) } }
        return work()
    }

    /// A small private file, never through a link: written to a new name, then renamed.
    private func write(_ text: String, to path: String) -> Bool {
        let temporary = path + ".new"
        unlink(temporary)
        let fd = open(temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { return false }
        let ok = Wire.writeAll(fd, Data(text.utf8))
        close(fd)
        guard ok, rename(temporary, path) == 0 else { unlink(temporary); return false }
        return true
    }

    private func read(_ path: String) -> String? {
        let fd = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var buffer = [UInt8](repeating: 0, count: 512)
        let n = Darwin.read(fd, &buffer, buffer.count)
        guard n > 0 else { return nil }
        let text = String(decoding: buffer[0..<n], as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty || text.utf8.count > 255 ? nil : text
    }

    func othersHolding() -> Bool {
        Wire.slots.contains { other in
            guard other != slot, let text = read(holdFile(other)), let number = Int32(text) else { return false }
            return alive(number)
        }
    }

    /// Istesso's source, after writing down the one selected now (unless a slot did already, or it
    /// is Istesso's already). Nothing is selected when that cannot be written down.
    func acquire() -> Bool {
        locked {
            let before = system.currentID()
            if before != Self.ours, access(previousFile, F_OK) != 0 {
                guard let before, write(before, to: previousFile) else { return false }
            }
            guard write("\(pid)", to: holdFile(slot)) else { return false }
            holding = true
            return system.select(Self.ours)
        }
    }

    /// Lets go of Istesso's source for this slot. nil: another slot still holds it (nothing is put
    /// back yet); else whether the source before it is the current one afterwards.
    @discardableResult
    func release() -> Bool? {
        locked {
            unlink(holdFile(slot))
            let was = holding
            holding = false
            _ = was
            if othersHolding() { return nil }
            return restoreWrittenDown()
        }
    }

    /// At the start: what this slot held before (a crash) is let go, and if no other slot holds
    /// Istesso's source, the source written down is put back. nil when there was nothing to do.
    func recoverAtStart() -> Bool? {
        locked {
            unlink(holdFile(slot))
            guard access(previousFile, F_OK) == 0, !othersHolding() else { return nil }
            return restoreWrittenDown()
        }
    }

    /// Puts back the source written down and forgets it; true when it is the current one (read
    /// back from the system). A source the owner chose by hand meanwhile (not Istesso's) is left
    /// as it is.
    private func restoreWrittenDown() -> Bool {
        guard let id = read(previousFile) else {
            unlink(previousFile)
            restoredTo = system.currentID()
            return restoredTo != Self.ours
        }
        if let current = system.currentID(), current != Self.ours { unlink(previousFile); restoredTo = current; return true }
        let back = system.select(id)
        if back { unlink(previousFile); restoredTo = id }
        return back
    }

    /// The owner picked another input method by hand since this slot put the source back (or
    /// while Istesso's was selected): it is not taken away from them until the next connection.
    func ownerChoseAnother() -> Bool {
        guard let current = system.currentID(), current != Self.ours else { return false }
        if holding { return true }
        guard let restoredTo else { return false }
        return system.family(current) != system.family(restoredTo)
    }

    /// A new connection: the owner's earlier choice no longer stands in the way.
    func forgetRestored() { restoredTo = nil }
}

/// Whether a viewer counts as shown for the inline part. A new stream after a resize, and the
/// start (the first resize, the virtual display made, the window moved), leave no viewer for a
/// while: that is not a disconnect (seen on the Studio, 2026-11-19 21:46 and 21:57: each such gap
/// put the source back and selected Istesso's again, longer than 1.5 s at the start, and the input
/// source was activated and deactivated with it). The rule: nobody shown counts only after 10 s
/// without a viewer. The owner loses nothing by it: their own keyboard takes their source back at
/// once (local typing first), and a lock or a stop puts it back at once too.
/// A new stream being made (`restarting`) counts only inside that same grace: the owner resizing
/// a window at the Studio while no viewer has been shown for long must never select the source
/// (review, 2026-11-19).
let inlineNobodyAfter = 10.0
func inlineViewerShown(viewers: Int, restarting: Bool, sinceShown: Double) -> Bool {
    _ = restarting  // adds nothing beyond the grace: it counts only inside it
    return viewers > 0 || sinceShown < inlineNobodyAfter
}

/// What the switching does in one tick (ADR 0023, 2.). Istesso's source is selected only while a
/// viewer is really there (never in the grace after the last one left); once selected it is kept
/// through the grace; it goes back on local typing, a lock, the end of the grace. `clearFirst`:
/// what is shown is cleared before it goes back (no viewer is there to end the composition, or the
/// screen locks); with a viewer there, local typing lets the input source end it as an input method
/// does (committed once, "ended").
enum SourceStep: Equatable { case none, acquire, keep, release(clearFirst: Bool) }
func inlineSourceStep(holding: Bool, connected: Bool, viewers: Int, locked: Bool, localIdle: Double, idle: Double) -> SourceStep {
    let idleEnough = localIdle >= idle
    if !holding {
        return connected && viewers > 0 && !locked && idleEnough ? .acquire : .none
    }
    if connected && !locked && idleEnough { return .keep }
    return .release(clearFirst: !connected || locked || viewers == 0)
}

/// Whether the moved window may be raised and made main again: the application is in front, and
/// the Mac's own keyboard has been idle (an owner typing in another window of the same application
/// keeps their window). Checked when it is decided and again right before the call.
func inlineMayRekey(appInFront: Bool, localIdle: Double, idle: Double) -> Bool {
    appInFront && localIdle >= idle
}

/// Whether Istesso's source should be selected now (ADR 0023, 2.): while a viewer is shown the
/// picture, the screen is not locked, and this Mac's own keyboard has been idle for `idle` seconds.
func inlineSourceWanted(connected: Bool, locked: Bool, localIdle: Double, idle: Double) -> Bool {
    connected && !locked && localIdle >= idle
}

// MARK: - The socket

/// This slot's socket. The host makes it; the input source connects (once a second while it runs)
/// and is taken only after its signature is checked. One input source at a time: a new one that
/// passes the check replaces the old connection.
final class InlineLink {
    let path: String
    var peerCheck: (Int32) -> Wire.PeerResult = { Wire.checkPeer($0, identifiers: [Wire.inputMethodIdentifier]) }
    var onConnected: () -> Void = {}
    var onGone: () -> Void = {}
    var onMessage: ([String: Any]) -> Void = { _ in }
    var onRefused: (String) -> Void = { _ in }
    private(set) var listener: Int32 = -1
    private(set) var fd: Int32 = -1
    private var reader = Wire.LineReader()
    private var sources: [DispatchSourceRead] = []
    private var peerSource: DispatchSourceRead?

    init(path: String) { self.path = path }
    var connected: Bool { fd >= 0 }

    /// The folder (0700, this user's, not a link) and a new socket in it (0600 from the start).
    func open() -> Bool {
        let folder = (path as NSString).deletingLastPathComponent
        if mkdir(folder, 0o700) != 0 && errno != EEXIST { return false }
        var info = stat()
        guard lstat(folder, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR, info.st_uid == getuid(), (info.st_mode & 0o077) == 0 else { return false }
        if lstat(path, &info) == 0 {
            guard (info.st_mode & S_IFMT) == S_IFSOCK else { return false }
            unlink(path)
        }
        guard var address = Wire.socketAddress(path) else { return false }
        let made = socket(AF_UNIX, SOCK_STREAM, 0)
        guard made >= 0 else { return false }
        let old = umask(0o177)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(made, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        umask(old)
        guard bound == 0, chmod(path, 0o600) == 0, Darwin.listen(made, 4) == 0 else { Darwin.close(made); return false }
        _ = fcntl(made, F_SETFL, O_NONBLOCK)
        _ = fcntl(made, F_SETFD, FD_CLOEXEC)
        listener = made
        return true
    }

    /// Read handlers on the main queue (the host); the self-test calls poll() itself instead.
    func watch() {
        let source = DispatchSource.makeReadSource(fileDescriptor: listener, queue: .main)
        source.setEventHandler { [weak self] in self?.acceptWaiting() }
        source.resume()
        sources.append(source)
    }

    func poll() {
        acceptWaiting()
        readWaiting()
    }

    private func acceptWaiting() {
        while listener >= 0 {
            let new = accept(listener, nil, nil)
            if new < 0 { return }
            var on: Int32 = 1
            setsockopt(new, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
            _ = fcntl(new, F_SETFD, FD_CLOEXEC)
            switch peerCheck(new) {
            case .refused(let why):
                Darwin.close(new)
                onRefused(why)
            case .ok:
                if fd >= 0 { drop() }
                _ = fcntl(new, F_SETFL, O_NONBLOCK)
                fd = new
                reader = Wire.LineReader()
                if !sources.isEmpty {
                    let source = DispatchSource.makeReadSource(fileDescriptor: new, queue: .main)
                    source.setEventHandler { [weak self] in self?.readWaiting() }
                    source.resume()
                    peerSource = source
                }
                onConnected()
            }
        }
    }

    private func readWaiting() {
        guard fd >= 0 else { return }
        var buffer = [UInt8](repeating: 0, count: 4096)
        while fd >= 0 {
            let n = read(fd, &buffer, buffer.count)
            if n < 0 && (errno == EAGAIN || errno == EINTR) { return }
            if n <= 0 { return drop() }
            guard let messages = reader.feed(Data(buffer[0..<n])) else { return drop() }
            for message in messages { onMessage(message) }
        }
    }

    func send(_ object: [String: Any]) -> Bool {
        guard fd >= 0, let data = Wire.encode(object) else { return false }
        if !Wire.writeAll(fd, data) { drop(); return false }
        return true
    }

    /// The input source went away (or must): what it showed is cleared on its side.
    func drop() {
        guard fd >= 0 else { return }
        peerSource?.cancel()
        peerSource = nil
        Darwin.close(fd)
        fd = -1
        onGone()
    }

    func close() {
        drop()
        for source in sources { source.cancel() }
        sources.removeAll()
        if listener >= 0 { Darwin.close(listener); listener = -1 }
        unlink(path)
    }
}

// MARK: - The composition

/// What Accessibility says about the relayed application's focused element.
enum FieldClass: Equatable { case unknown, text, secure, notText }

/// Kind 5 as the host says it (ADR 0015): state, caret in picture pixels, field serial.
struct FieldReport: Equatable {
    var state: UInt32 = 0
    var x: Int32 = 0, y: Int32 = 0, width: UInt32 = 0, height: UInt32 = 0
    var serial: UInt32 = 0
}

/// Global points (top left of the main display, as CGWindow bounds) to picture pixels:
/// `origin` is where the picture's top left lies, `pointsPerPixel` as the injector has it.
func caretPixels(_ caret: CGRect, origin: CGPoint, pointsPerPixel: Double, picture: CGSize) -> (Int32, Int32, UInt32, UInt32) {
    guard pointsPerPixel > 0 else { return (0, 0, 0, 0) }
    func px(_ v: Double) -> Double { (v / pointsPerPixel).rounded() }
    let x = max(0, min(picture.width, px(caret.minX - origin.x)))
    let y = max(0, min(picture.height, px(caret.minY - origin.y)))
    let w = max(0, min(65535, px(caret.width))), h = max(0, min(65535, px(caret.height)))
    return (Int32(x), Int32(y), UInt32(w), UInt32(h))
}

/// The state of the link with the input source and of the composition, for one slot. Everything
/// it does to the outside goes through the closures, so that the self-test runs the same code.
final class InlineComposer {
    let app: String
    /// To the input source (one line); false when it is not connected.
    var send: ([String: Any]) -> Bool = { _ in false }
    /// Today's text path (a key event carrying the text), for whatever the input source refuses.
    var deliverText: (String) -> Void = { _ in }
    /// Kind 5, when it changed.
    var setField: (FieldReport) -> Void = { _ in }
    /// Events held after a commit may go on (in order).
    var releaseHold: () -> Void = {}
    var now: () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
    var note: (String) -> Void = { _ in }

    // What the host tells it.
    var linked = false            // the input source is connected and took "hello"
    var sourceOurs = false        // Istesso's source is selected, by this host
    var fieldClass = FieldClass.unknown { didSet { if fieldClass != oldValue { notTextSince = fieldClass == .notText ? now() : nil } } }
    var shownWindow = CGRect.zero // global points
    var origin = CGPoint.zero
    var pointsPerPixel = 1.0
    var picture = CGSize.zero
    /// The input-source side's pixels a point where the caret is (for the record: compared with ours).
    var densityAtWindow: () -> Double = { 1 }

    private(set) var active = false
    private(set) var caret: CGRect?
    private(set) var comp: Int?
    private var nextComp = 1
    private(set) var stripComposition = false
    private var endedUntil: TimeInterval?
    private var notTextSince: TimeInterval?
    private var serial: UInt32 = 0
    private var seq = 0
    private var pending: [Int: (op: String, comp: Int?, text: String?)] = [:]
    private(set) var awaiting: (seq: Int, until: TimeInterval)?
    private(set) var report = FieldReport()
    private var reported = false
    /// Compositions the input source said were ended (committed once by the application): a commit
    /// of one of them that comes back refused is never typed again.
    private var endedSerials: [Int] = []
    /// Commits whose "inserted" did not come in time: the keys after them went on, so a refusal
    /// that comes later is not typed again either (it would land after those keys).
    private var timedOut: [Int] = []
    private var compStartedAt: TimeInterval = -1
    private var lastViewerPress: TimeInterval = -1
    /// When the host last posted a key (or text by the key path) to the application, and until
    /// when a mark or a commit that came right after it waits.
    private var keyPostedAt: TimeInterval = -1000
    private var afterKeyUntil: TimeInterval?
    /// A mark sent right after a posted key overtook it on the Studio (2026-11-19 21:47, twice:
    /// "かな", Return, "です": the Return and "です" were lost). Keys go through the window server,
    /// marks through the input source; nothing orders them. So a mark or a commit that comes within
    /// this time of a posted key waits until it has passed (ADR 0023, 4., "キーのあとの変換").
    static let afterKey = 0.05
    private(set) var counts: [String: Int] = [:]
    static let insertedWithin = 0.05
    static let notTextAfter = 0.1

    init(app: String) { self.app = app }

    var holding: Bool { awaiting != nil || afterKeyUntil != nil }

    /// The host posted a key (or text by the key path) to the application.
    func keyPosted() { keyPostedAt = now() }

    /// The stop: nothing waits any more (what waited is dropped by the host).
    func dropWaits() { afterKeyUntil = nil }

    /// Whether this event (composing text, or a commit) must wait because a key was posted just
    /// before it while composing inline; the wait then begins (everything after it waits too).
    func waitsAfterKey(kind: String, text: String?) -> Bool {
        guard kind == "text" || (kind == "preedit" && !(text ?? "").isEmpty) else { return false }
        guard engaged || comp != nil else { return false }
        let until = keyPostedAt + Self.afterKey
        guard now() < until else { return false }
        afterKeyUntil = until
        count("waited_after_key")
        return true
    }
    private func count(_ what: String) { counts[what, default: 0] += 1 }

    /// Inline now: the input source said "active" about the relayed application, Istesso's source
    /// is selected, the caret is in the shown window, and this composition was not put on the strip.
    var engaged: Bool {
        guard linked, sourceOurs, active, !stripComposition, fieldClass == .text, let caret else { return false }
        return shownWindow.insetBy(dx: -2, dy: -2).contains(CGPoint(x: caret.minX, y: caret.minY))
    }

    @discardableResult
    private func ask(_ object: [String: Any], comp: Int? = nil, text: String? = nil) -> Int? {
        seq += 1
        var object = object
        object["seq"] = seq
        guard send(object) else { return nil }
        pending[seq] = (object["op"] as? String ?? "", comp, text)
        // Bounded, but never the commit being waited on.
        if pending.count > 256, let oldest = pending.keys.filter({ $0 != awaiting?.seq }).min() { pending[oldest] = nil }
        return seq
    }

    // ---- The link ----

    func connected() {
        linked = false
        active = false
        _ = ask(["op": "hello", "app": app])
        count("connected")
    }

    /// The input source went away: it cleared what it showed (never committed), so the rest of a
    /// composition goes on the strip and its commit by the text path.
    func gone() {
        linked = false
        active = false
        if comp != nil { comp = nil; stripComposition = true }
        let wasHolding = holding
        afterKeyUntil = nil
        if awaiting != nil {
            // Whether it went in cannot be known; it is not sent again (it would land twice).
            count("commit_unknown_at_cut")
            awaiting = nil
        }
        if wasHolding { releaseHold() }
        pending.removeAll()
        count("gone")
        recompute()
    }

    private func remember(ended c: Int) {
        endedSerials.append(c)
        if endedSerials.count > 64 { endedSerials.removeFirst(endedSerials.count - 64) }
    }

    func isPending(_ s: Int) -> Bool { pending[s] != nil }

    /// The application ended this composition (committed once there): the viewer is told by a new
    /// field serial and ends its own without committing; what crosses that is dropped for a while.
    private func endedByApplication(_ c: Int) {
        remember(ended: c)
        comp = nil
        endedUntil = now() + 1.0
        serial = (serial + 1) % 256
        count("ended_by_application")
    }

    /// Before Istesso's source is let go of on a disconnect or a stop: what is shown is cleared
    /// (never committed into the document by the switch). The seq of the clear, to wait for.
    func clearNow() -> Int? {
        guard linked, let c = comp else { return nil }
        remember(ended: c)
        comp = nil
        stripComposition = true
        recompute()
        return ask(["op": "clear"], comp: c)
    }

    func message(_ m: [String: Any]) {
        let op = m["op"] as? String ?? ""
        if let s = m["seq"] as? Int, let asked = pending.removeValue(forKey: s) {
            return answer(m, to: asked, seq: s)
        }
        switch op {
        case "active":
            active = true
            takeCaret(m)
            count("active")
        case "inactive":
            // Seen on the Mac Studio: "inactive" straight after "active" when Istesso's source came
            // back after local typing. The strip is used meanwhile (nothing is lost), and the caret
            // question (answered only while the relayed application's field is active) tells
            // whether it is active after all.
            active = false
            count("inactive")
        case "ended":
            // The application ended the composition (a click elsewhere, the focus moved, the source
            // switched back): the input source committed it once. The viewer is told by a new field
            // serial and ends its own without committing; what crosses that on the way is dropped.
            if let c = m["comp"] as? Int {
                remember(ended: c)
                if c == comp {
                    endedByApplication(c)
                } else {
                    // A composition whose commit is on its way: that commit will be refused, and
                    // must not be typed again.
                    count("ended_while_committing")
                }
            }
        default:
            count("unknown_op")
        }
        recompute()
    }

    private func answer(_ m: [String: Any], to asked: (op: String, comp: Int?, text: String?), seq: Int) {
        let ok = m["ok"] as? Bool ?? true
        let why = m["why"] as? String
        switch asked.op {
        case "hello":
            linked = ok
            if !ok { note("inline: the input source refused this slot's application (another slot relays it?): strip") }
        case "mark":
            if ok { takeCaret(m); break }
            if why == "ended" || why == "stale" {
                // Ended there already (the application ended it, or it was committed or cleared):
                // never shown again; if this host still took it for the current one, it ends here.
                if let c = asked.comp, c == comp { endedByApplication(c) }
                break
            }
            // Refused (too long, secure input, not active...): this composition goes on the strip.
            count("mark_refused")
            if comp != nil { _ = ask(["op": "clear"]); comp = nil }
            stripComposition = true
        case "commit":
            if ok {
                if m["inserted"] as? Bool == false {
                    // In already (ended by the application, or a repeat): not again, not by text.
                    count("commit_already_in")
                } else {
                    takeCaret(m)
                    count("commit_inserted")
                }
            } else if why == "cleared", !timedOut.contains(seq) {
                // Cleared there, never inserted: by today's text path.
                count("commit_cleared_text_path")
                if let text = asked.text { deliverText(text) }
            } else if why == "stale" {
                count("commit_stale")
            } else if let c = asked.comp, endedSerials.contains(c) {
                // The application ended it: the input source committed it once already.
                count("commit_refused_after_end")
            } else if why == nil {
                // Refused without a reason: whether it went in is not known (an older input
                // source); not typed again.
                count("commit_unknown")
            } else if timedOut.contains(seq) {
                count("commit_refused_after_timeout")
            } else if let text = asked.text {
                count("commit_refused_text_path")
                deliverText(text)
            }
            timedOut.removeAll { $0 == seq }
            if awaiting?.seq == seq { awaiting = nil; if afterKeyUntil == nil { releaseHold() } }
        case "caret":
            // Answered from the relayed application's active field (a rectangle or not), or
            // refused because it is not active.
            if why == nil {
                if !active { count("active_by_caret") }
                active = true
                takeCaret(m)
            } else if why == "relayed app not active" {
                active = false
            }
        case "clear":
            if ok, m["x"] != nil { takeCaret(m) }
        default: break
        }
        recompute()
    }

    /// The input source's caret: global points, top from the main display's top.
    private func takeCaret(_ m: [String: Any]) {
        guard (m["rect_ok"] as? Bool) ?? (m["ok"] as? Bool) ?? true,
              let x = (m["x"] as? NSNumber)?.doubleValue, let top = (m["top"] as? NSNumber)?.doubleValue,
              let w = (m["w"] as? NSNumber)?.doubleValue, let h = (m["h"] as? NSNumber)?.doubleValue,
              x.isFinite, top.isFinite, w.isFinite, h.isFinite, w >= 0, h >= 0, w < 10_000, h < 10_000 else { return }
        caret = CGRect(x: x, y: top, width: w, height: h)
        if let scale = (m["scale"] as? NSNumber)?.doubleValue, scale > 0, abs(scale - densityAtWindow()) > 0.01 {
            count("caret_scale_differs")
        }
    }

    /// Asks where the caret is (and, by the answer, whether the relayed application's field is
    /// active): while the source is ours and nothing waits.
    func probe() {
        guard linked, sourceOurs, awaiting == nil else { return }
        _ = ask(["op": "caret"])
    }

    // ---- From the viewer ----

    /// A `preedit` event: the whole composing text and its selected part (UTF-8 bytes).
    func preedit(_ text: String, cursor: [Int]) {
        guard cursor.count == 2, Wire.acceptable(text, allowEmpty: true), cursor[0] <= cursor[1],
              Wire.utf16Offset(text, utf8: cursor[0]) != nil, Wire.utf16Offset(text, utf8: cursor[1]) != nil else {
            return count("preedit_unreadable")
        }
        if let until = endedUntil {
            // Ended by the application: until the viewer has ended it too (its empty preedit), what
            // it still sends of that composition is dropped.
            if text.isEmpty || now() > until { endedUntil = nil; if text.isEmpty { return } } else { return count("preedit_after_end_dropped") }
        }
        if text.isEmpty {
            if let c = comp { _ = ask(["op": "clear"], comp: c); comp = nil }
            return
        }
        if stripComposition { return count("preedit_on_strip") }
        guard engaged else {
            // The viewer has not heard yet that the host is not inline: the strip, for this one.
            // One that was inline until now (Accessibility unsure once, the caret out of the
            // window) is cleared there first, so that it is never left in the application to be
            // committed by its deactivation as well as typed by the text path.
            if let c = comp {
                _ = ask(["op": "clear"], comp: c)
                comp = nil
                count("inline_to_strip_cleared")
            }
            stripComposition = true
            count("preedit_not_inline")
            return recompute()
        }
        if comp == nil { compStartedAt = now() }
        let c = comp ?? { defer { nextComp += 1 }; return nextComp }()
        comp = c
        if ask(["op": "mark", "text": text, "sel": cursor, "comp": c], comp: c) == nil {
            comp = nil
            stripComposition = true
            recompute()
        }
        count("mark")
    }

    /// Committed text. True when it went to the input source: what comes after it is then held
    /// until it is inserted (at most 50 ms).
    func commit(_ text: String) -> Bool {
        if let until = endedUntil, now() <= until {
            // The viewer's commit of the composition the application ended: it landed already.
            count("commit_after_end_dropped")
            return false
        }
        endedUntil = nil
        if stripComposition {
            stripComposition = false
            deliverText(text)
            count("commit_text_path")
            recompute()
            return false
        }
        guard comp != nil || engaged else { deliverText(text); count("commit_text_path"); return false }
        let c = comp ?? { defer { nextComp += 1 }; return nextComp }()
        comp = nil
        if c >= nextComp { nextComp = c + 1 }
        guard let s = ask(["op": "commit", "text": text, "comp": c], comp: c, text: text) else {
            deliverText(text)
            count("commit_text_path")
            recompute()
            return false
        }
        awaiting = (s, now() + Self.insertedWithin)
        count("commit")
        return true
    }

    /// A key went down from the viewer: no composition is going on there (the input method takes
    /// the keys while it composes).
    func keyPressed() {
        lastViewerPress = now()
        if stripComposition || endedUntil != nil {
            stripComposition = false
            endedUntil = nil
            recompute()
        }
    }

    /// A press of the viewer's (a click may move the focus over here).
    func buttonPressed() { lastViewerPress = now() }

    /// Another element of the relayed application has the keyboard now (Accessibility, read a
    /// little later than the click or key that moved it).
    func fieldChanged() {
        if let c = comp {
            if compStartedAt > lastViewerPress {
                // Begun after the click or key that moved the focus: it is in the new field
                // already, and the viewer keeps it (no new serial, which would wipe it there).
                count("field_changed_composition_kept")
                return recompute()
            }
            // Begun before: the input source ends it in the old field (and says so).
            remember(ended: c)
            comp = nil
            endedUntil = now() + 1.0
        }
        serial = (serial + 1) % 256
        recompute()
    }

    /// Called often (every 10 ms while something is held, every 100 ms otherwise).
    func tick() {
        if let until = afterKeyUntil, now() >= until {
            afterKeyUntil = nil
            if awaiting == nil { releaseHold() }
        }
        if let waiting = awaiting, now() > waiting.until {
            count("inserted_timeout")
            timedOut.append(waiting.seq)
            if timedOut.count > 64 { timedOut.removeFirst(timedOut.count - 64) }
            awaiting = nil
            if afterKeyUntil == nil { releaseHold() }
        }
        recompute()
    }

    func recompute() {
        var next = FieldReport(serial: serial)
        if fieldClass == .secure {
            next.state = 2
        } else if engaged, let caret {
            next.state = 1
            (next.x, next.y, next.width, next.height) = caretPixels(caret, origin: origin, pointsPerPixel: pointsPerPixel, picture: picture)
        } else if fieldClass == .notText, !active, let since = notTextSince, now() - since >= Self.notTextAfter {
            next.state = 3
        }
        guard !reported || next != report else { return }
        reported = true
        report = next
        setField(next)
    }
}

// MARK: - Accessibility: what the focused element is

/// The relayed application's focused element: a password field, a text field, something else, or
/// unknown (not readable, or another process's). Roles and subroles only; never a value.
func readFieldClass(_ pid: pid_t) -> (FieldClass, Int?) {
    let application = AXUIElementCreateApplication(pid)
    var focused: CFTypeRef?
    guard AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
          let focused, CFGetTypeID(focused) == AXUIElementGetTypeID() else { return (.unknown, nil) }
    let element = focused as! AXUIElement
    var owner: pid_t = 0
    guard AXUIElementGetPid(element, &owner) == .success, owner == pid else { return (.unknown, nil) }
    var role: CFTypeRef?, subrole: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &role) == .success, let role = role as? String else {
        return (.unknown, Int(CFHash(element)))
    }
    _ = AXUIElementCopyAttributeValue(element, kAXSubroleAttribute as CFString, &subrole)
    let identity = Int(CFHash(element))
    if (subrole as? String) == (kAXSecureTextFieldSubrole as String) { return (.secure, identity) }
    let textRoles: Set<String> = [kAXTextFieldRole as String, kAXTextAreaRole as String, kAXComboBoxRole as String]
    if textRoles.contains(role) { return (.text, identity) }
    // Any element with a text selection of its own is taken as a field (web content, custom views).
    var names: CFArray?
    if AXUIElementCopyAttributeNames(element, &names) == .success, let list = names as? [String],
       list.contains(kAXSelectedTextRangeAttribute as String) || list.contains(kAXInsertionPointLineNumberAttribute as String) {
        return (.text, identity)
    }
    return (.notText, identity)
}

// MARK: - The host's side, put together

/// One slot's inline composition: the socket, the switching of the input source, and the composer,
/// driven by the host every 100 ms.
final class InlineHost {
    let composer: InlineComposer
    let link: InlineLink
    let switcher: SourceSwitch
    let idle: Double
    var localIdle: () -> Double = { CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: .keyDown) }
    private let axQueue = DispatchQueue(label: "inline-ax")
    private var axAsked = false
    private var lastIdentity: Int?
    private var ticks = 0
    private var selectFailedAt: TimeInterval = -1000
    private var timer: DispatchSourceTimer?
    /// Events held after a commit, in order.
    var held: [Data] = []
    static let mostHeld = 256
    private(set) var heldDropped = 0
    /// The owner picked another input method by hand: Istesso's is not selected again until the
    /// next connection.
    private(set) var ownerChose = false
    private var wasConnected = false
    /// Stopped: nothing is selected again (seen on the Studio, 2026-11-19 21:41: a tick after the
    /// stop's restore selected Istesso's source again, and the program then ended).
    private(set) var stopped = false
    var pid: () -> pid_t? = { nil }

    init?(app: String, slot: Int, idle: Double, system: InputSourceSystem = TISInputSources(), path given: String? = nil) {
        guard let path = given ?? Wire.socketPath(slot: slot) else { return nil }
        composer = InlineComposer(app: app)
        link = InlineLink(path: path)
        switcher = SourceSwitch(folder: (path as NSString).deletingLastPathComponent, slot: slot, system: system)
        self.idle = idle
        link.onConnected = { [weak self] in note("inline: the input source connected"); self?.composer.connected() }
        link.onGone = { [weak self] in note("inline: the input source went away"); self?.composer.gone() }
        link.onMessage = { [weak self] in self?.composer.message($0) }
        link.onRefused = { why in note("inline: a program that is not the input source was refused (\(why))") }
        composer.send = { [weak self] in self?.link.send($0) ?? false }
        composer.note = { note($0) }
    }

    func start() -> Bool {
        guard link.open() else { return false }
        if let restored = switcher.recoverAtStart() { note("inline: start after a crash, restored: \(restored ? "yes" : "no")") }
        link.watch()
        return true
    }

    /// An event while a commit waits for "inserted": held in order (true), or, past the bound,
    /// dropped and counted (true as well: it never goes ahead of the queue).
    /// Composing text or a commit right after a posted key waits a moment (a mark must not
    /// overtake the key); keys and everything else go at once.
    private func waitsAfterKey(_ data: Data) -> Bool {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { return false }
        return composer.waitsAfterKey(kind: kind, text: object["text"] as? String)
    }

    /// The next held event that may go now (taken off the queue), or nil: something waits, or a
    /// mark right after a key just let go of from this same queue must wait its moment too.
    func nextToFlush() -> Data? {
        guard !stopped, !composer.holding, let first = held.first, !waitsAfterKey(first) else { return nil }
        return held.removeFirst()
    }

    /// Events a chord's hold lets go of are older than anything held here: they go in front, in
    /// their order. False when nothing is held (they then go their usual way).
    func holdInFront(_ events: [Data]) -> Bool {
        guard composer.holding || !held.isEmpty else { return false }
        held.insert(contentsOf: events, at: 0)
        if held.count > Self.mostHeld { heldDropped += held.count - Self.mostHeld; held.removeLast(held.count - Self.mostHeld) }
        return true
    }

    func hold(_ data: Data) -> Bool {
        if !composer.holding && held.isEmpty {
            guard waitsAfterKey(data) else { return false }
        }
        if held.count < Self.mostHeld { held.append(data) } else { heldDropped += 1 }
        return true
    }

    /// What is shown is cleared, and the answer waited for (100 ms at most), before the source is
    /// put back: switching first would make the input source commit it into the document. It
    /// blocks the main queue for that time (rare: a disconnect, a lock, a stop); answers read
    /// meanwhile may let held events go, through the same checks as always.
    func clearBeforeRestore() {
        guard let s = composer.clearNow() else { return }
        let deadline = ProcessInfo.processInfo.systemUptime + 0.1
        while composer.isPending(s), link.connected, ProcessInfo.processInfo.systemUptime < deadline {
            var poller = pollfd(fd: link.fd, events: Int16(POLLIN), revents: 0)
            _ = poll(&poller, 1, 10)
            link.poll()
        }
        note("inline: cleared before the source went back: \(composer.isPending(s) ? "no answer" : "yes")")
    }

    /// The viewer pressed a button: the focus may have moved; Accessibility is asked at once.
    func viewerPressed() {
        composer.buttonPressed()
        askField()
    }

    private func askField() {
        guard let pid = pid(), !axAsked else { return }
        axAsked = true
        axQueue.async {
            let (found, identity) = readFieldClass(pid)
            DispatchQueue.main.async {
                self.axAsked = false
                if identity != nil, identity != self.lastIdentity, self.lastIdentity != nil { self.composer.fieldChanged() }
                if identity != nil { self.lastIdentity = identity }
                self.composer.fieldClass = found
            }
        }
    }

    /// Every 100 ms (10 ms while a commit is held): which source, whether inline, what the field is.
    func tick(connected: Bool, viewers: Int = 1, locked: Bool, pid: pid_t?) {
        guard !stopped else { return }
        if connected && !wasConnected { ownerChose = false; switcher.forgetRestored() }
        wasConnected = connected
        let step = inlineSourceStep(holding: switcher.holding, connected: connected, viewers: viewers, locked: locked,
                                    localIdle: localIdle(), idle: idle)
        let wanted = step == .acquire
        let now = ProcessInfo.processInfo.systemUptime
        if switcher.holding && switcher.ownerChoseAnother() {
            // Picked by hand while Istesso's was selected: left as chosen.
            ownerChose = true
            _ = switcher.release()
            note("inline: the owner picked another input source: left as it is until the next connection")
        } else if wanted && !switcher.holding && !ownerChose && switcher.ownerChoseAnother() {
            ownerChose = true
            note("inline: the owner picked another input source: left as it is until the next connection")
        }
        if wanted && !switcher.holding && !ownerChose && now - selectFailedAt > 10 {
            let selected = switcher.acquire()
            note("inline: Istesso's source selected: \(selected ? "yes" : "no")")
            if !selected {
                selectFailedAt = now
                let restored = switcher.release()
                note("inline: restored: \(restored.map { $0 ? "yes" : "no" } ?? "another slot holds it")")
            }
        } else if case let .release(clearFirst) = step, switcher.holding {
            if clearFirst { clearBeforeRestore() }
            let restored = switcher.release()
            note("inline: \(connected ? (locked ? "locked" : (viewers == 0 ? "local typing, nobody shown" : "local typing")) : "nobody shown"): restored: \(restored.map { $0 ? "yes" : "no" } ?? "another slot holds it")")
        }
        composer.sourceOurs = switcher.holding && switcher.system.currentID() == SourceSwitch.ours
        ticks += 1
        if pid != nil, connected { askField() }
        if ticks % 5 == 0 { composer.probe() }
        composer.tick()
    }

    /// The end of this object (the program stops): what is shown is cleared, Istesso's source is
    /// let go of, the socket closed; nothing is selected again afterwards.
    func shutdown() {
        guard !stopped else { return }
        stopped = true
        // What waited is dropped, never typed at the stop (a held key-down would be posted and
        // never released; a held commit would land after the source went back).
        heldDropped += held.count
        held.removeAll()
        composer.dropWaits()
        clearBeforeRestore()
        if switcher.holding {
            let restored = switcher.release()
            note("inline: stop: restored: \(restored.map { $0 ? "yes" : "no" } ?? "another slot holds it")")
        }
        link.close()
    }
}

// MARK: - Self-test (no input source of the system's, no Accessibility: stand-ins over a socket)

final class FakeSources: InputSourceSystem {
    var current: String?
    var enabled: Set<String>
    var selects = 0
    init(current: String, enabled: Set<String>) { self.current = current; self.enabled = enabled }
    func currentID() -> String? { current }
    var onSelect: (String) -> Void = { _ in }
    func family(_ id: String) -> String { id.components(separatedBy: "#").first ?? id }
    func select(_ id: String) -> Bool {
        onSelect(id)
        selects += 1
        if enabled.contains(id) { current = id }
        return current == id
    }
}

/// The stand-in input source: the client end of the host's socket, answering as the real one does
/// (wire.swift's lines), driven step by step from the test.
final class StandInInputSource {
    let fd: Int32
    private let reader = Wire.LineReader()
    var received: [[String: Any]] = []
    init?(path: String) {
        guard let fd = Wire.connect(path) else { return nil }
        self.fd = fd
        _ = fcntl(fd, F_SETFL, O_NONBLOCK)
    }
    /// What the host sent since the last call.
    func take() -> [[String: Any]] {
        var buffer = [UInt8](repeating: 0, count: 8192)
        var got: [[String: Any]] = []
        for _ in 0..<50 {
            let n = read(fd, &buffer, buffer.count)
            if n > 0 { got += reader.feed(Data(buffer[0..<n])) ?? [] } else if !got.isEmpty || n == 0 { break } else { usleep(2000) }
        }
        received += got
        return got
    }
    func say(_ object: [String: Any]) { _ = Wire.writeAll(fd, Wire.encode(object)!) }
    func caret(_ op: String, seq: Any?, x: Double, top: Double, extra: [String: Any] = [:]) {
        var m: [String: Any] = ["op": op, "ok": true, "x": x, "y": 0, "w": 0, "h": 20, "top": top, "scale": 2.0]
        if let seq { m["seq"] = seq }
        for (k, v) in extra { m[k] = v }
        say(m)
    }
}

func inlineSelfTest() -> Int {
    var failed = 0
    func check(_ ok: Bool, _ what: String) {
        if ok { print("ok   inline: \(what)") } else { failed += 1; print("FAIL inline: \(what)") }
    }
    check(inlineSourceWanted(connected: true, locked: false, localIdle: 5, idle: 3), "Istesso's source while connected and the local keyboard idle")
    check(!inlineSourceWanted(connected: true, locked: false, localIdle: 0.4, idle: 3), "the local keyboard first: typing there puts the source back")
    check(!inlineSourceWanted(connected: false, locked: false, localIdle: 99, idle: 3), "not without a viewer")
    check(!inlineSourceWanted(connected: true, locked: true, localIdle: 99, idle: 3), "not while the screen is locked")
    check(inlineViewerShown(viewers: 0, restarting: true, sinceShown: 0.2) && inlineViewerShown(viewers: 0, restarting: false, sinceShown: 1.0),
          "a new stream after a resize is not a disconnect")
    check(!inlineViewerShown(viewers: 0, restarting: true, sinceShown: 999), "a resize with no viewer for long counts as nobody (the owner resizing at the Studio)")
    check(inlineSourceStep(holding: false, connected: true, viewers: 0, locked: false, localIdle: 99, idle: 3) == .none,
          "in the grace with nobody there: never selected")
    check(inlineSourceStep(holding: true, connected: true, viewers: 0, locked: false, localIdle: 99, idle: 3) == .keep,
          "in the grace: kept")
    check(inlineSourceStep(holding: true, connected: true, viewers: 0, locked: false, localIdle: 0.5, idle: 3) == .release(clearFirst: true),
          "local typing in the grace: cleared first, then put back")
    check(inlineSourceStep(holding: true, connected: true, viewers: 1, locked: false, localIdle: 0.5, idle: 3) == .release(clearFirst: false),
          "local typing with a viewer: the input source ends it (no clear)")
    check(inlineSourceStep(holding: true, connected: false, viewers: 0, locked: false, localIdle: 99, idle: 3) == .release(clearFirst: true)
          && inlineSourceStep(holding: true, connected: true, viewers: 1, locked: true, localIdle: 99, idle: 3) == .release(clearFirst: true),
          "gone or locked: cleared first")
    check(inlineSourceStep(holding: false, connected: true, viewers: 1, locked: false, localIdle: 99, idle: 3) == .acquire,
          "a viewer there, the local keyboard idle: selected")
    check(!inlineMayRekey(appInFront: true, localIdle: 0.2, idle: 3) && !inlineMayRekey(appInFront: false, localIdle: 99, idle: 3)
          && inlineMayRekey(appInFront: true, localIdle: 5, idle: 3), "the moved window is made key again only with the app in front and no local typing")
    check(inlineViewerShown(viewers: 0, restarting: false, sinceShown: 5) && !inlineViewerShown(viewers: 0, restarting: false, sinceShown: 10.1)
          && inlineViewerShown(viewers: 1, restarting: false, sinceShown: 99),
          "a start's gap (seconds) is not a disconnect either; nobody for 10 s is")
    // 2x virtual display: the window at (1000, 200) points, the picture 2 pixels a point.
    let px = caretPixels(CGRect(x: 1010, y: 210, width: 0, height: 20), origin: CGPoint(x: 1000, y: 200), pointsPerPixel: 0.5,
                         picture: CGSize(width: 800, height: 600))
    check(px == (20, 20, 0, 40), "caret in picture pixels on a 2x display (\(px))")
    let one = caretPixels(CGRect(x: 110, y: 150, width: 2, height: 17), origin: CGPoint(x: 100, y: 100), pointsPerPixel: 1,
                          picture: CGSize(width: 400, height: 300))
    check(one == (10, 50, 2, 17), "caret in picture pixels at 1x")
    let clamped = caretPixels(CGRect(x: -50, y: 9999, width: 0, height: 20), origin: .zero, pointsPerPixel: 1, picture: CGSize(width: 400, height: 300))
    check(clamped.0 == 0 && clamped.1 == 300, "a caret outside the picture is kept inside it")

    // ---- Switching, with stand-in input sources and a folder of the test's own ----
    guard let base = Wire.userTemporaryFolder() else { print("FAIL inline: no temporary folder"); return failed + 1 }
    let folder = (base as NSString).appendingPathComponent("istesso-daily-host-test-\(getpid())")
    mkdir(folder, 0o700)
    defer {
        for name in ["previous-source", "hold-slot-1", "hold-slot-2", "hold-slot-3", "switch.lock", "slot-1"] {
            unlink((folder as NSString).appendingPathComponent(name))
        }
        rmdir(folder)
    }
    let system = FakeSources(current: "test.source.before", enabled: ["test.source.before", "test.source.other", SourceSwitch.ours])
    let one1 = SourceSwitch(folder: folder, slot: 1, system: system)
    let two = SourceSwitch(folder: folder, slot: 2, system: system)
    check(one1.acquire() && system.current == SourceSwitch.ours, "slot 1 selects Istesso's source")
    check((try? String(contentsOfFile: one1.previousFile, encoding: .utf8)) == "test.source.before", "the source before it is written down")
    var info = stat()
    check(lstat(one1.previousFile, &info) == 0 && (info.st_mode & 0o077) == 0, "the file is this user's only")
    check(two.acquire(), "slot 2 holds it too")
    check((try? String(contentsOfFile: one1.previousFile, encoding: .utf8)) == "test.source.before", "and does not write Istesso's as the one before")
    check(one1.release() == nil && system.current == SourceSwitch.ours, "slot 1 lets go: slot 2 still holds, nothing put back")
    check(two.release() == true && system.current == "test.source.before" && access(one1.previousFile, F_OK) != 0,
          "the last one lets go: the source before is back, the file gone")
    // A crash: slot 1 held, its process gone; the next start puts the source back.
    _ = one1.acquire()
    let restarted = SourceSwitch(folder: folder, slot: 1, system: system)
    check(restarted.recoverAtStart() == true && system.current == "test.source.before", "start after a crash: put back")
    // Another slot's process that crashed does not hold forever.
    _ = two.acquire()
    let later = SourceSwitch(folder: folder, slot: 1, system: system)
    later.alive = { _ in false }
    check(later.recoverAtStart() == true && system.current == "test.source.before", "a crashed slot's hold is not counted")
    _ = two.release()
    // The owner chose another source by hand meanwhile: left as chosen.
    _ = one1.acquire()
    system.current = "test.source.other"
    check(one1.release() == true && system.current == "test.source.other", "a source the owner chose meanwhile is left as it is")
    // Istesso's not enabled: not selected, and put back at once.
    let without = FakeSources(current: "test.source.before", enabled: ["test.source.before"])
    let s3 = SourceSwitch(folder: folder, slot: 3, system: without)
    check(!s3.acquire() && s3.release() == true && without.current == "test.source.before", "not enabled: nothing changes")

    // ---- The socket and the peer check ----
    let path = (folder as NSString).appendingPathComponent("slot-1")
    let link = InlineLink(path: path)
    check(link.open(), "the socket is made")
    var folderInfo = stat()
    check(lstat(path, &info) == 0 && (info.st_mode & 0o777) == 0o600 && lstat(folder, &folderInfo) == 0 && (folderInfo.st_mode & 0o777) == 0o700,
          "the socket is 0600 in a 0700 folder")
    check(Wire.socketIsPrivate(path), "the input source's own check takes it as private")
    // The real check, both ways: this test program is signed ad hoc, so nobody is taken (fail closed).
    if let stranger = Wire.connect(path) {
        var refused: String?
        link.onRefused = { refused = $0 }
        link.poll()
        check(refused != nil && !link.connected, "the real signature check refuses a program that is not the signed input source")
        close(stranger)
    } else { check(false, "connect to the socket") }

    // From here the stand-in is taken by a check of the user only (the signature check is above).
    link.peerCheck = { fd in
        var uid: uid_t = 0, gid: gid_t = 0
        return getpeereid(fd, &uid, &gid) == 0 && uid == getuid() ? .ok("stand-in") : .refused("user")
    }
    let composer = InlineComposer(app: "com.example.relayed") // check-private: ok synthetic bundle identifier
    var clock: TimeInterval = 100
    composer.now = { clock }
    var fields: [FieldReport] = []
    var delivered: [String] = []
    var released = 0
    var notes: [String] = []
    composer.send = { link.send($0) }
    composer.setField = { fields.append($0) }
    composer.deliverText = { delivered.append($0) }
    composer.releaseHold = { released += 1 }
    composer.note = { notes.append($0) }
    composer.densityAtWindow = { 2 }
    link.onConnected = { composer.connected() }
    link.onGone = { composer.gone() }
    link.onMessage = { composer.message($0) }
    guard let im = StandInInputSource(path: path) else { print("FAIL inline: the stand-in does not connect"); return failed + 1 }
    link.poll()
    check(link.connected, "the stand-in input source is taken")
    let hello = im.take()
    check(hello.first?["op"] as? String == "hello" && hello.first?["app"] as? String == "com.example.relayed", // check-private: ok synthetic bundle identifier
          "the first message is hello, naming the relayed application")
    im.say(["op": "hello", "ok": true, "slot": 1, "seq": hello.first?["seq"] ?? 0])
    link.poll()
    check(composer.linked, "hello taken")

    // Geometry: the shown window on a 2x virtual display.
    composer.shownWindow = CGRect(x: 1000, y: 200, width: 400, height: 300)
    composer.origin = CGPoint(x: 1000, y: 200)
    composer.pointsPerPixel = 0.5
    composer.picture = CGSize(width: 800, height: 600)
    composer.fieldClass = .text
    composer.recompute()
    check(composer.report.state == 0, "a text field before the source is ours: the strip (0)")
    composer.preedit("にほ", cursor: [6, 6])
    check(im.take().isEmpty && composer.stripComposition, "a composition before inline is not marked: the strip")
    composer.keyPressed()
    composer.sourceOurs = true
    im.caret("active", seq: nil, x: 1010, top: 210)
    link.poll()
    check(composer.report.state == 1 && composer.report.x == 20 && composer.report.y == 20 && composer.report.height == 40,
          "after active, with the caret in the window: inline (1), the caret in picture pixels")
    composer.preedit("にほ", cursor: [0, 6])
    composer.preedit("にほん", cursor: [0, 9])
    let marks = im.take()
    check(marks.count == 2 && marks.allSatisfy { $0["op"] as? String == "mark" && $0["comp"] as? Int == 1 }
          && (marks.last?["sel"] as? [Int]) == [0, 9], "preedit becomes mark, with the host's serial and the UTF-8 selection")
    for m in marks { im.caret("mark", seq: m["seq"], x: 1030, top: 210, extra: ["comp": 1]) }
    link.poll()
    check(composer.report.x == 60, "the caret follows the composition")

    // Commit, and what comes after it waits for "inserted".
    let first = composer.commit("日本")
    let commitLine = im.take().first
    check(first && composer.holding && commitLine?["op"] as? String == "commit" && commitLine?["comp"] as? Int == 1,
          "text commits the composition through the input source, and holds what follows")
    im.caret("commit", seq: commitLine?["seq"], x: 1040, top: 210, extra: ["inserted": true, "comp": 1])
    link.poll()
    check(!composer.holding && released == 1, "inserted: what was held goes on")
    // No answer within 50 ms: it goes on anyway, counted.
    check(composer.commit("a"), "a commit with no composition before it goes inline too")
    let lone = im.take().first
    check(lone?["comp"] as? Int == 2, "with a new serial")
    clock += 0.06
    composer.tick()
    check(!composer.holding && released == 2 && composer.counts["inserted_timeout"] == 1, "no answer in 50 ms: let go, counted")
    // A refused commit goes by the text path; a stale one is dropped.
    _ = composer.commit("x")
    let refusedLine = im.take().first
    im.say(["op": "commit", "ok": false, "why": "relayed app not active", "seq": refusedLine?["seq"] ?? 0])
    link.poll()
    check(delivered == ["x"], "a refused commit is delivered by today's text path, once")
    _ = composer.commit("y")
    let staleLine = im.take().first
    im.say(["op": "commit", "ok": false, "why": "stale", "comp": staleLine?["comp"] ?? 0, "seq": staleLine?["seq"] ?? 0])
    link.poll()
    check(delivered == ["x"], "a stale commit is not delivered again")
    _ = im.take()

    // A refused mark: the composition goes on the strip, its commit by the text path.
    composer.preedit(String(repeating: "あ", count: 3), cursor: [9, 9])
    let longMark = im.take().first
    im.say(["op": "mark", "ok": false, "why": "text", "seq": longMark?["seq"] ?? 0])
    link.poll()
    let clearLine = im.take().first
    check(clearLine?["op"] as? String == "clear" && composer.report.state == 0, "a refused mark: cleared there, the strip here (0)")
    composer.preedit("", cursor: [0, 0])
    composer.preedit("あああい", cursor: [12, 12])
    check(im.take().isEmpty, "the rest of that composition is not marked")
    check(!composer.commit("ああああ") && delivered.last == "ああああ" && composer.report.state == 1,
          "its commit goes by the text path, then inline again")
    delivered.removeAll()

    // The application ended the composition: the viewer is told by a new serial; what crosses is dropped.
    composer.preedit("か", cursor: [3, 3])
    let kMark = im.take().first
    im.caret("mark", seq: kMark?["seq"], x: 1010, top: 210, extra: ["comp": kMark?["comp"] ?? 0])
    let serialBefore = composer.report.serial
    im.say(["op": "ended", "by": "application", "comp": kMark?["comp"] ?? 0])
    link.poll()
    check(composer.report.serial == (serialBefore + 1) % 256, "ended by the application: the field serial changes (the viewer ends its own)")
    composer.preedit("かな", cursor: [6, 6])
    check(!composer.commit("かな") && delivered.isEmpty && im.take().isEmpty, "what crossed the end (a mark, a commit) is dropped, never landing twice")
    composer.preedit("", cursor: [0, 0])
    composer.preedit("ね", cursor: [3, 3])
    let next = im.take().first
    check(next?["op"] as? String == "mark" && (next?["comp"] as? Int ?? 0) > (kMark?["comp"] as? Int ?? 0), "the next composition gets a new serial")
    im.caret("mark", seq: next?["seq"], x: 1010, top: 210)
    link.poll()

    // inactive, then the caret question says active again (seen on the Studio: inactive after active).
    im.say(["op": "inactive"])
    link.poll()
    check(composer.report.state == 0, "inactive: the strip (0)")
    composer.probe()
    let caretAsk = im.take().first
    check(caretAsk?["op"] as? String == "caret", "the host asks where the caret is")
    im.caret("caret", seq: caretAsk?["seq"], x: 1010, top: 210)
    link.poll()
    check(composer.report.state == 1, "a caret answer means active after all: inline again")

    // The caret outside the shown window: not inline.
    im.say(["op": "active", "ok": true, "x": 50, "y": 0, "w": 0, "h": 20, "top": 50, "scale": 2])
    link.poll()
    check(composer.report.state == 0, "the caret outside the shown window: the strip")
    im.caret("active", seq: nil, x: 1010, top: 210)
    link.poll()

    // Field states from Accessibility.
    composer.fieldClass = .secure
    composer.recompute()
    check(composer.report.state == 2, "a password field (AXSecureTextField): 2")
    composer.fieldClass = .unknown
    composer.recompute()
    check(composer.report.state == 0, "unknown by Accessibility, even with the input source active: the strip (0)")
    im.say(["op": "inactive"])
    link.poll()
    check(composer.report.state == 0, "unknown and not active: the strip (0)")
    composer.fieldClass = .notText
    composer.recompute()
    check(composer.report.state == 0, "not a text field: still 0 at first")
    clock += 0.11
    composer.recompute()
    check(composer.report.state == 3, "not a text field for 100 ms: 3")
    composer.fieldClass = .text
    im.caret("active", seq: nil, x: 1010, top: 210)
    link.poll()
    check(composer.report.state == 1, "back in a text field: inline")
    composer.preedit("", cursor: [0, 0])
    _ = im.take()
    let serialNow = composer.report.serial
    composer.fieldChanged()
    check(composer.report.serial == (serialNow + 1) % 256, "another focused element: a new serial")

    // A composition begun after the viewer's click that moved the focus is in the new field: the
    // late Accessibility answer neither ends it nor wipes it on the viewer (no new serial).
    _ = im.take()
    composer.buttonPressed()
    clock += 0.01
    composer.preedit("た", cursor: [3, 3])
    let newField = im.take().first
    im.caret("mark", seq: newField?["seq"], x: 1010, top: 210)
    link.poll()
    let serialKept = composer.report.serial
    composer.fieldChanged()
    check(composer.report.serial == serialKept && composer.comp != nil, "begun after the click: kept, no new serial")
    // Begun before the press: the focus moved away from it; it ends here, the viewer is told.
    clock += 0.01
    composer.buttonPressed()
    composer.fieldChanged()
    check(composer.comp == nil && composer.report.serial == (serialKept + 1) % 256, "begun before the click: ended, a new serial")
    composer.preedit("", cursor: [0, 0])
    clock += 2
    _ = im.take()

    // HIGH-2 of the review: the application ended a composition whose commit is on its way; the
    // refusal that follows is never typed again.
    composer.preedit("ち", cursor: [3, 3])
    let chiMark = im.take().first
    im.caret("mark", seq: chiMark?["seq"], x: 1010, top: 210)
    link.poll()
    _ = composer.commit("ち")
    let chiCommit = im.take().first
    im.say(["op": "ended", "by": "application", "comp": chiCommit?["comp"] ?? 0])
    im.say(["op": "commit", "ok": false, "why": "relayed app not active", "seq": chiCommit?["seq"] ?? 0])
    link.poll()
    check(delivered.isEmpty && composer.counts["commit_refused_after_end"] == 1 && !composer.holding,
          "a commit refused after its composition was ended by the application is not typed again")
    // A refusal without a reason: not known whether it went in; not typed again.
    _ = composer.commit("つ")
    let noWhy = im.take().first
    im.say(["op": "commit", "ok": false, "seq": noWhy?["seq"] ?? 0])
    link.poll()
    check(delivered.isEmpty && composer.counts["commit_unknown"] == 1, "a refusal without a reason is taken as unknown, not typed again")
    // A refusal that comes after the 50 ms wait ran out: the keys went on; not typed again.
    _ = composer.commit("て")
    let late = im.take().first
    clock += 0.06
    composer.tick()
    im.say(["op": "commit", "ok": false, "why": "relayed app not active", "seq": late?["seq"] ?? 0])
    link.poll()
    check(delivered.isEmpty && composer.counts["commit_refused_after_timeout"] == 1, "a refusal after the wait ran out is not typed after later keys")
    // The bound on questions waiting for answers never drops the commit waited on.
    _ = composer.commit("と")
    let waited = im.take().first
    for i in 0..<300 {
        composer.preedit(i % 2 == 0 ? "な" : "なに", cursor: [3, 3])
        if i % 20 == 0 { _ = im.take() }
    }
    _ = im.take()
    im.say(["op": "commit", "ok": false, "why": "text", "seq": waited?["seq"] ?? 0])
    link.poll()
    check(delivered == ["と"] && !composer.holding, "after 300 more questions the waited commit's answer is still known")
    delivered.removeAll()
    composer.preedit("", cursor: [0, 0])
    _ = im.take()

    // A mark or a commit right after a posted key waits 50 ms (it must not overtake the key);
    // keys and other events do not wait; after the time, what waited goes on.
    clock += 2
    let releasedBefore = released
    composer.keyPosted()
    clock += 0.01
    check(!composer.waitsAfterKey(kind: "key", text: nil) && !composer.waitsAfterKey(kind: "preedit", text: ""),
          "after a key: another key, or an empty preedit, does not wait")
    check(composer.waitsAfterKey(kind: "preedit", text: "で") && composer.holding, "after a key: a mark waits")
    clock += 0.02
    composer.tick()
    check(composer.holding && released == releasedBefore, "and still 30 ms after the key")
    clock += 0.03
    composer.tick()
    check(!composer.holding && released == releasedBefore + 1, "50 ms after the key it goes on")
    composer.keyPosted()
    clock += 0.06
    check(!composer.waitsAfterKey(kind: "text", text: "で"), "a commit 60 ms after a key does not wait")
    composer.keyPosted()
    check(composer.waitsAfterKey(kind: "text", text: "で"), "a commit right after a key waits")
    clock += 0.06
    composer.tick()
    check(!composer.holding, "and goes on")

    // An inline composition whose inline conditions go away mid-way (here the caret leaves the
    // window): cleared there before it goes on the strip, its commit typed once.
    clock += 2
    composer.preedit("ほ", cursor: [3, 3])
    let midMark = im.take().first
    im.caret("mark", seq: midMark?["seq"], x: 1010, top: 210)
    link.poll()
    im.say(["op": "active", "ok": true, "x": 50, "y": 0, "w": 0, "h": 20, "top": 50, "scale": 2])
    link.poll()
    composer.preedit("ほん", cursor: [6, 6])
    let midClear = im.take()
    check(midClear.count == 1 && midClear.first?["op"] as? String == "clear" && composer.comp == nil && composer.stripComposition,
          "inline, then not: cleared there first, the rest on the strip")
    check(!composer.commit("本") && delivered == ["本"], "and its commit typed once by the text path")
    delivered.removeAll()
    im.caret("active", seq: nil, x: 1010, top: 210)
    link.poll()

    // The answers of input source 0.4: ok means shown / in; ok:false always with a reason.
    clock += 2
    _ = composer.commit("に")
    let inAlready = im.take().first
    im.say(["op": "commit", "ok": true, "inserted": false, "ended": "application", "comp": inAlready?["comp"] ?? 0, "seq": inAlready?["seq"] ?? 0])
    link.poll()
    check(delivered.isEmpty && !composer.holding && composer.counts["commit_already_in"] == 1,
          "0.4: ok without inserted (ended there already): not again, not by the text path")
    _ = composer.commit("ぬ")
    let cleared = im.take().first
    im.say(["op": "commit", "ok": false, "why": "cleared", "comp": cleared?["comp"] ?? 0, "seq": cleared?["seq"] ?? 0])
    link.poll()
    check(delivered == ["ぬ"] && !composer.holding, "0.4: cleared there, never inserted: by the text path, once")
    delivered.removeAll()
    composer.preedit("の", cursor: [3, 3])
    let endedMark = im.take().first
    let serialBeforeMark = composer.report.serial
    im.say(["op": "mark", "ok": false, "why": "ended", "ended": "application", "comp": endedMark?["comp"] ?? 0, "seq": endedMark?["seq"] ?? 0])
    link.poll()
    check(composer.comp == nil && !composer.stripComposition && composer.report.serial == (serialBeforeMark + 1) % 256,
          "0.4: a mark refused as ended ends the composition here too (new serial), not the strip")
    composer.preedit("", cursor: [0, 0])
    clock += 2
    _ = im.take()
    let caretBefore = composer.report
    composer.probe()
    let noRect = im.take().first
    im.say(["op": "caret", "ok": false, "rect_ok": false, "x": 0, "y": 0, "w": 0, "h": 0, "top": 0, "seq": noRect?["seq"] ?? 0])
    link.poll()
    check(composer.active && composer.report == caretBefore, "0.4: a caret answer without a rectangle: still active, the caret kept")
    composer.preedit("は", cursor: [3, 3])
    let markNoRect = im.take().first
    im.say(["op": "mark", "ok": true, "rect_ok": false, "x": 0, "y": 0, "w": 0, "h": 0, "top": 0, "comp": markNoRect?["comp"] ?? 0, "seq": markNoRect?["seq"] ?? 0])
    link.poll()
    check(composer.report == caretBefore && !composer.stripComposition, "0.4: a mark shown without a rectangle: inline, the caret kept")
    composer.preedit("", cursor: [0, 0])
    _ = im.take()

    // The input source goes away during a composition: the rest on the strip, its commit by text.
    composer.preedit("さ", cursor: [3, 3])
    _ = im.take()
    close(im.fd)
    link.poll()
    check(!link.connected && composer.report.state == 0, "the input source gone: the strip")
    check(!composer.commit("さ") && delivered == ["さ"], "the composition it cleared is committed by the text path")

    // Nothing typed is in anything the host writes down.
    let written = notes.joined() + composer.counts.keys.joined()
    check(!["にほ", "日本", "か", "さ", "ああ"].contains { written.contains($0) }, "no text in the notes or counts")
    link.close()
    check(access(path, F_OK) != 0, "the socket is removed at the end")
    failed += inlineHostSelfTest(folder: folder)
    return failed
}

/// The host's side as a whole (switching, clearing before the source goes back, the owner's choice,
/// the held events), with a stand-in input source answering on a thread of its own.
func inlineHostSelfTest(folder: String) -> Int {
    var failed = 0
    func check(_ ok: Bool, _ what: String) {
        if ok { print("ok   inline: \(what)") } else { failed += 1; print("FAIL inline: \(what)") }
    }
    let path = (folder as NSString).appendingPathComponent("slot-1")
    let system = FakeSources(current: "test.ime#kana", enabled: ["test.ime#kana", "test.ime#roman", "test.layout", SourceSwitch.ours])
    guard let host = InlineHost(app: "com.example.relayed", slot: 1, idle: 3, system: system, path: path), // check-private: ok synthetic bundle identifier
          host.link.open() else { print("FAIL inline: host not made"); return 1 }
    defer { host.link.close() }
    host.link.peerCheck = { fd in
        var uid: uid_t = 0, gid: gid_t = 0
        return getpeereid(fd, &uid, &gid) == 0 && uid == getuid() ? .ok("stand-in") : .refused("user")
    }
    var idle = 99.0
    host.localIdle = { idle }
    let lock = NSLock()
    var seen: [String] = []
    var clearBeforeSelect: Bool?
    system.onSelect = { id in
        if id != SourceSwitch.ours { lock.lock(); clearBeforeSelect = seen.contains("clear"); lock.unlock() }
    }
    let stop = DispatchSemaphore(value: 0)
    let done = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        guard let im = StandInInputSource(path: path) else { done.signal(); return }
        im.say(["op": "active", "ok": true, "x": 1010, "y": 0, "w": 0, "h": 20, "top": 210, "scale": 2])
        while stop.wait(timeout: .now()) == .timedOut {
            for m in im.take() {
                let op = m["op"] as? String ?? ""
                lock.lock(); seen.append(op); lock.unlock()
                switch op {
                case "hello": im.say(["op": "hello", "ok": true, "slot": 1, "seq": m["seq"] ?? 0])
                case "mark", "clear", "caret": im.caret(op, seq: m["seq"], x: 1010, top: 210)
                default: break
                }
            }
        }
        close(im.fd)
        done.signal()
    }
    let composer = host.composer
    func pump(_ seconds: Double, until: () -> Bool = { false }) {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end && !until() { host.link.poll(); usleep(2000) }
    }
    pump(1) { composer.linked && composer.active }
    check(composer.linked && composer.active, "the host's link: hello taken, active")
    composer.shownWindow = CGRect(x: 1000, y: 200, width: 400, height: 300)
    composer.origin = CGPoint(x: 1000, y: 200)
    composer.pointsPerPixel = 0.5
    composer.picture = CGSize(width: 800, height: 600)
    composer.fieldClass = .text
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == SourceSwitch.ours && composer.sourceOurs, "connected, the local keyboard idle: Istesso's source")
    composer.recompute()
    composer.preedit("にほ", cursor: [6, 6])
    pump(0.3) { composer.comp != nil && !composer.isPending(1_000_000) && { lock.lock(); defer { lock.unlock() }; return seen.contains("mark") }() }
    pump(0.05)
    check(composer.comp != nil, "a composition is shown")
    // From the held queue too: a key let go of, then the mark behind it waits its moment.
    func event(_ object: [String: Any]) -> Data { try! JSONSerialization.data(withJSONObject: object) }
    host.held = [event(["kind": "key", "code": 28, "pressed": true]), event(["kind": "preedit", "text": "で", "cursor": [3, 3]])]
    let firstOut = host.nextToFlush()
    composer.keyPosted()  // as the injector does when it posts that key
    check(firstOut != nil && host.nextToFlush() == nil && host.held.count == 1, "held: the key goes, the mark behind it waits")
    usleep(60_000)
    composer.tick()
    check(host.nextToFlush() != nil && host.held.isEmpty, "held: and goes 50 ms after the key")
    host.tick(connected: false, locked: false, pid: nil)
    check(clearBeforeSelect == true && system.current == "test.ime#kana", "disconnected: cleared first, then the source before put back")
    // The owner's own choice.
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == SourceSwitch.ours, "connected again: Istesso's")
    idle = 0.5
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == "test.ime#kana" && clearBeforeSelect == true, "local typing: the source before is back")
    system.current = "test.ime#roman"
    idle = 99
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == SourceSwitch.ours, "another mode of the same input method is not a choice: Istesso's again")
    idle = 0.5
    host.tick(connected: true, locked: false, pid: nil)
    system.current = "test.layout"
    idle = 99
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == "test.layout" && host.ownerChose, "another input source picked by hand: left as chosen")
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == "test.layout", "and still after more idle time")
    host.tick(connected: false, locked: false, pid: nil)
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == SourceSwitch.ours && !host.ownerChose, "the next connection: Istesso's again")
    system.current = "test.layout"
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == "test.layout" && !host.switcher.holding, "picked by hand while Istesso's was selected: let go, left as chosen")
    // A stop puts the source back, and no later tick selects Istesso's again.
    host.tick(connected: true, locked: false, pid: nil)
    system.current = "test.ime#kana"
    host.tick(connected: false, locked: false, pid: nil)
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current == SourceSwitch.ours, "connected: Istesso's (before the stop)")
    host.held = [Data([1]), Data([2])]
    let droppedBefore = host.heldDropped
    host.shutdown()
    check(host.held.isEmpty && host.heldDropped == droppedBefore + 2 && host.nextToFlush() == nil && !composer.holding,
          "the stop drops what waited (never typed at the stop)")
    check(system.current != SourceSwitch.ours && !host.switcher.holding, "the stop puts the source back")
    host.tick(connected: true, locked: false, pid: nil)
    check(system.current != SourceSwitch.ours && !host.switcher.holding, "and a tick after the stop selects nothing")
    stop.signal()
    _ = done.wait(timeout: .now() + 2)
    pump(0.05)
    // Held events: in order while a commit waits; past the bound dropped and counted, never ahead.
    check(!host.hold(Data([1])), "nothing waits: an event is not held")
    host.held = [Data([0])]
    check(host.hold(Data([1])) && host.held == [Data([0]), Data([1])], "behind what is held, in order")
    host.held = Array(repeating: Data([2]), count: InlineHost.mostHeld)
    let boundBefore = host.heldDropped
    check(host.hold(Data([3])) && host.held.count == InlineHost.mostHeld && host.heldDropped == boundBefore + 1, "past the bound: dropped and counted, never ahead of the queue")
    host.held = [Data([9])]
    check(host.holdInFront([Data([7]), Data([8])]) && host.held == [Data([7]), Data([8]), Data([9])],
          "a chord's held events go in front of what waits here, in their order")
    host.held = []
    check(!host.holdInFront([Data([7])]), "nothing waits: a chord's events go their usual way")
    for name in ["previous-source", "hold-slot-1", "switch.lock"] { unlink((folder as NSString).appendingPathComponent(name)) }
    return failed
}
