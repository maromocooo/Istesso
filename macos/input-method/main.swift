// Istesso Dev Input (com.maromoco.istesso-dev.inputmethod.trial): a trial input source that shows the
// remote client's composing text inline in the application in front on this Mac (ADR 0020, route
// d: the client's own input method composes; this one only shows and commits what it is given).
//
// - It never takes this Mac's own keyboard: recognizedEvents is empty and handle() declines, so
//   every key goes on to the application as if no input method were there.
// - Text comes only from the Mac host over the host's local socket (wire.swift): the input method
//   connects, checks the host's signature, and then sets marked text, commits or clears in the
//   client that is active now, and answers with the caret's rectangle.
// - Neither composing nor committed text is logged: lengths only.
//
// Trial only. It can put text into any application, so it is reviewed before it goes anywhere near
// main.
import AppKit
import Carbon
import CryptoKit
import InputMethodKit
import os

let log = Logger(subsystem: Wire.inputMethodIdentifier, category: "link")

/// What the input method does with a message from the host, kept apart from InputMethodKit so the
/// wire test can run it without a client.
enum Commands {
    struct Mark { let text: String; let selection: NSRange; let serial: Int }

    /// A "mark" message as text, a UTF-16 selection and the composition's serial, or the reason it
    /// is refused. The serial is the client's: one per composition, never used again.
    static func mark(_ message: [String: Any]) -> Result<Mark, Refusal> {
        guard let text = message["text"] as? String, Wire.acceptable(text, allowEmpty: true) else { return .failure(.init("text")) }
        guard let serial = message["comp"] as? Int else { return .failure(.init("comp")) }
        guard let cursor = message["sel"] as? [Int], cursor.count == 2, cursor[0] <= cursor[1],
              let start = Wire.utf16Offset(text, utf8: cursor[0]), let end = Wire.utf16Offset(text, utf8: cursor[1]) else {
            return .failure(.init("sel"))
        }
        return .success(Mark(text: text, selection: NSRange(location: start, length: end - start), serial: serial))
    }

    struct Commit { let text: String; let serial: Int }

    static func commit(_ message: [String: Any]) -> Result<Commit, Refusal> {
        guard let text = message["text"] as? String, Wire.acceptable(text, allowEmpty: false) else { return .failure(.init("text")) }
        guard let serial = message["comp"] as? Int else { return .failure(.init("comp")) }
        return .success(Commit(text: text, serial: serial))
    }

    struct Refusal: Error { let field: String; init(_ field: String) { self.field = field } }
}

/// A composition shown in a session, and the slot of the host it belongs to.
struct Shown { let mark: Commands.Mark; let slot: Int }

/// One host's connection: one slot (1-3), one relayed application.
///
/// The host names the application it relays ("hello") before anything else. From then on its
/// mark, commit, clear and caret act only while THAT application's field is active, and it hears
/// "active" / "inactive" only about that application. No other application's identifier is ever
/// sent or logged.
final class HostConnection {
    let slot: Int
    let fd: Int32
    var source: DispatchSourceRead?
    var reader = Wire.LineReader()
    var app: String?
    /// Compositions of this host that have ended, and how: "committed", "application" (the
    /// application ended it and it was committed), "cleared" (never inserted). Serials are the host's.
    var ended: [(serial: Int, reason: String)] = []
    var toldActive = false

    init(slot: Int, fd: Int32) { self.slot = slot; self.fd = fd }

    func end(_ serial: Int, _ reason: String) {
        ended.append((serial, reason))
        if ended.count > 64 { ended.removeFirst(ended.count - 64) }
    }

    func endedReason(_ serial: Int) -> String? { ended.last { $0.serial == serial }?.reason }
}

/// The connections to the hosts, and which controller shows which composition.
///
/// Each controller (one per text session of a client) keeps its own composition, owned by the slot
/// that sent it. A composition ends exactly once: committed by the host, committed by the
/// application (a click elsewhere, the focus moving), or cleared (the host gone). Its serial is then
/// remembered, so that a commit or a mark that crossed the end on the wire is dropped instead of
/// landing a second time.
final class Link {
    static let shared = Link()
    weak var active: IstessoDevInputController?
    /// The controller showing a composition now (it may no longer be the active one).
    weak var showing: IstessoDevInputController?
    private var hosts: [Int: HostConnection] = [:]
    private var timer: Timer?
    private let nowhere = NSRange(location: NSNotFound, length: NSNotFound)

    func start() {
        tryConnect()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in self?.tryConnect() }
    }

    private func tryConnect() {
        for slot in Wire.slots where hosts[slot] == nil {
            guard let path = Wire.socketPath(slot: slot), let fd = Wire.connect(path) else { continue }
            switch Wire.checkPeer(fd, identifiers: Wire.hostIdentifiers) {
            case .refused(let why):
                log.error("slot \(slot): host refused: \(why, privacy: .public)")
                close(fd)
                continue
            case .ok:
                log.info("slot \(slot): connected")
            }
            let host = HostConnection(slot: slot, fd: fd)
            let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
            source.setEventHandler { [weak self, weak host] in if let host { self?.readable(host) } }
            source.setCancelHandler { close(fd) }
            source.resume()
            host.source = source
            hosts[slot] = host
        }
    }

    /// The host went away (the viewer disconnected, the host stopped): what it was composing is
    /// cleared, never committed and never left behind in the application.
    private func disconnect(_ host: HostConnection) {
        if let controller = showing, let shown = controller.shown, shown.slot == host.slot {
            controller.client()?.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
            controller.shown = nil
            showing = nil
            log.info("slot \(host.slot): host gone, composing text cleared")
        }
        host.source?.cancel()
        host.source = nil
        hosts[host.slot] = nil
    }

    private func readable(_ host: HostConnection) {
        var buffer = [UInt8](repeating: 0, count: 4096)
        let n = read(host.fd, &buffer, buffer.count)
        if n <= 0 { log.info("slot \(host.slot): host closed"); disconnect(host); return }
        guard let messages = host.reader.feed(Data(buffer[0..<n])) else { log.error("line too long"); disconnect(host); return }
        for message in messages { handle(message, from: host) }
    }

    func send(_ object: [String: Any], to host: HostConnection, seq: Any? = nil) {
        var object = object
        if let seq = seq as? Int { object["seq"] = seq }
        guard host.source != nil, let data = Wire.encode(object) else { return }
        if !Wire.writeAll(host.fd, data) { disconnect(host) }
    }

    /// Tells each host whether ITS application's field is the active one now ("active" with the
    /// caret) or not ("inactive", once).
    func announce() {
        for host in hosts.values {
            guard let app = host.app else { continue }
            if let controller = active, controller.clientIdentifier == app, let client = controller.client() {
                var message = caret(client, controller)
                message["op"] = "active"
                message["ok"] = true
                send(message, to: host)
                host.toldActive = true
            } else if host.toldActive {
                send(["op": "inactive"], to: host)
                host.toldActive = false
            }
        }
    }

    private func handle(_ message: [String: Any], from host: HostConnection) {
        let op = message["op"] as? String ?? ""
        let seq = message["seq"]
        if op == "hello" {
            // The relayed application, once per connection, and not one another slot relays.
            guard host.app == nil, let app = message["app"] as? String, !app.isEmpty, app.count <= 255,
                  !hosts.values.contains(where: { $0 !== host && $0.app == app }) else {
                send(["op": "hello", "ok": false], to: host, seq: seq)
                return
            }
            host.app = app
            send(["op": "hello", "ok": true, "slot": host.slot], to: host, seq: seq)
            announce()
            return
        }
        guard let app = host.app else {
            send(["op": op, "ok": false, "why": "no hello"], to: host, seq: seq)
            return
        }
        // A commit or a clear of this host's composition goes where that composition is shown,
        // whether or not its session is still the active one (its deactivation may be on the way),
        // and is checked against the ended serials BEFORE anything else: an answer must never make
        // the host type again a text that already landed.
        if op == "commit" { commit(message, from: host, app: app, seq: seq); return }
        if op == "clear" { clear(from: host, seq: seq); return }
        if op == "mark", let serial = message["comp"] as? Int, let reason = host.endedReason(serial) {
            send(["op": "mark", "ok": false, "why": "ended", "ended": reason, "comp": serial], to: host, seq: seq)
            return
        }
        guard let controller = active, controller.clientIdentifier == app, let client = controller.client() else {
            send(["op": op, "ok": false, "why": "relayed app not active"], to: host, seq: seq)
            return
        }
        switch op {
        case "mark":
            // Secure event input is on (some application, not necessarily this field, holds it):
            // no composing text; the host uses the strip. Whether the field is a password field
            // is the host's to read through Accessibility.
            if IsSecureEventInputEnabled() {
                send(["op": "mark", "ok": false, "why": "secure-input"], to: host, seq: seq)
                return
            }
            switch Commands.mark(message) {
            case .failure(let refusal): send(["op": "mark", "ok": false, "why": refusal.field], to: host, seq: seq)
            case .success(let mark):
                // A composition still shown in another session (its deactivation not come yet), or
                // another host's, ends there first, so that it lands in its own application.
                if let other = showing, other !== controller || other.shown?.slot != host.slot { applicationEnded(other) }
                if mark.text.isEmpty {
                    client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
                    controller.shown = nil
                    showing = nil
                } else {
                    client.setMarkedText(controller.styled(mark), selectionRange: mark.selection, replacementRange: nowhere)
                    controller.shown = Shown(mark: mark, slot: host.slot)
                    showing = controller
                }
                log.info("mark: \(mark.text.utf16.count) units")
                // "ok" is whether the mark was shown, never whether a caret rectangle was found
                // ("rect_ok").
                var answer = caret(client, controller)
                answer["op"] = "mark"
                answer["ok"] = true
                answer["comp"] = mark.serial
                send(answer, to: host, seq: seq)
            }
        case "caret":
            send(caret(client, controller), to: host, seq: seq)
        #if ISTESSO_TRIAL_CHECK
        case "check":
            send(check(client), to: host, seq: seq)
        #endif
        default:
            send(["op": op, "ok": false, "why": "unknown"], to: host, seq: seq)
        }
    }

    /// commit: "ok" means it is in (or was already in); "ok": false only for a real refusal with a
    /// "why", after which the host delivers the text by its text path.
    private func commit(_ message: [String: Any], from host: HostConnection, app: String, seq: Any?) {
        let commit: Commands.Commit
        switch Commands.commit(message) {
        case .failure(let refusal):
            send(["op": "commit", "ok": false, "why": refusal.field], to: host, seq: seq)
            return
        case .success(let value): commit = value
        }
        if let reason = host.endedReason(commit.serial) {
            if reason == "cleared" {
                // Cleared, never inserted: a refusal, so the host delivers it by the text path.
                send(["op": "commit", "ok": false, "why": "cleared", "comp": commit.serial], to: host, seq: seq)
            } else {
                // Landed already (the application ended it, or this is a repeat): not again, and
                // not by the text path either.
                send(["op": "commit", "ok": true, "inserted": false, "ended": reason, "comp": commit.serial], to: host, seq: seq)
            }
            return
        }
        // Where this host's composition with this serial is shown (active or not), or else into
        // the relayed application's active field (a commit with no composition before it).
        let target: IstessoDevInputController
        if let shownIn = showing, let shown = shownIn.shown, shown.slot == host.slot, shown.mark.serial == commit.serial {
            target = shownIn
        } else {
            guard let controller = active, controller.clientIdentifier == app else {
                send(["op": "commit", "ok": false, "why": "relayed app not active", "comp": commit.serial], to: host, seq: seq)
                return
            }
            if controller.shown != nil {
                send(["op": "commit", "ok": false, "why": "other composition", "comp": commit.serial], to: host, seq: seq)
                return
            }
            target = controller
        }
        guard let client = target.client() else {
            send(["op": "commit", "ok": false, "why": "no client", "comp": commit.serial], to: host, seq: seq)
            return
        }
        let at = target.shown != nil ? client.markedRange().location : client.selectedRange().location
        client.insertText(IstessoDevInputController.japanese(commit.text), replacementRange: nowhere)
        target.shown = nil
        if showing === target { showing = nil }
        host.end(commit.serial, "committed")
        log.info("commit: \(commit.text.utf16.count) units")
        // "inserted": the answer is made after a call that returns a value from the client
        // (caret() asks for its ranges), which the client answers only after the insert before it.
        // Keys the host posts after a commit wait for this (bounded), so that text and keys never
        // change places.
        var answer = caret(client, target)
        answer["op"] = "commit"
        answer["ok"] = true
        answer["inserted"] = true
        answer["comp"] = commit.serial
        #if ISTESSO_TRIAL_CHECK
        answer["at"] = at == NSNotFound ? -1 : at
        #else
        _ = at
        #endif
        send(answer, to: host, seq: seq)
    }

    /// clear: this host's composition, wherever it is shown.
    private func clear(from host: HostConnection, seq: Any?) {
        if let shownIn = showing, let shown = shownIn.shown, shown.slot == host.slot {
            shownIn.client()?.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
            shownIn.shown = nil
            showing = nil
            host.end(shown.mark.serial, "cleared")
        }
        send(["op": "clear", "ok": true], to: host, seq: seq)
    }

    #if ISTESSO_TRIAL_CHECK
    /// PROTOTYPE ONLY, built only with --trial-check; the product input source never has it (it
    /// would be a part that can read an application's text). Reads back, through the client and not
    /// through Accessibility or the screen, the marked range, the selection, the document's length,
    /// and a SHA-256 of the MARKED text only (text the host itself sent). Never the text; the probe
    /// prints only whether the hashes match. Only in TextEdit.
    func check(_ client: IMKTextInput & NSObjectProtocol) -> [String: Any] {
        guard client.bundleIdentifier() == "com.apple.TextEdit" else { return ["op": "check", "ok": false, "why": "only in TextEdit"] }
        let markedRange = client.markedRange()
        let selected = client.selectedRange()
        var digest = ""
        if markedRange.location != NSNotFound, markedRange.length > 0 {
            var actual = NSRange(location: NSNotFound, length: 0)
            if let text = client.string(from: markedRange, actualRange: &actual) {
                digest = SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
            }
        }
        func pair(_ r: NSRange) -> [Int] { r.location == NSNotFound ? [-1, -1] : [r.location, r.length] }
        return ["op": "check", "ok": !digest.isEmpty, "marked": pair(markedRange), "selected": pair(selected),
                "length": client.length(), "sha256": digest]
    }
    #endif

    /// The application ended the composition itself (a click elsewhere, another window, the source
    /// switched back for local typing): what was shown is committed once into that same session, as
    /// an input method does, and its host is told so that the client ends its own composition
    /// without committing it again.
    func applicationEnded(_ controller: IstessoDevInputController) {
        guard let shown = controller.shown else { return }
        controller.shown = nil
        if showing === controller { showing = nil }
        controller.client()?.insertText(IstessoDevInputController.japanese(shown.mark.text), replacementRange: nowhere)
        if let host = hosts[shown.slot] {
            host.end(shown.mark.serial, "application")
            send(["op": "ended", "by": "application", "comp": shown.mark.serial], to: host)
        }
    }

    /// The caret's rectangle in global points, AppKit's way round (origin at the bottom left of the
    /// primary screen). The host turns it into picture pixels of the window it sends (ADR 0015,
    /// kind 5): y from the top = primary height - (y + h), less the window's origin, times the
    /// window's pixels per point.
    func caret(_ client: IMKTextInput & NSObjectProtocol, _ controller: IstessoDevInputController) -> [String: Any] {
        var rect = NSRect.zero
        var via = "none"
        let markedRange = client.markedRange()
        let selection = controller.shown?.mark.selection ?? NSRange(location: 0, length: 0)
        if markedRange.location != NSNotFound {
            var actual = NSRange(location: NSNotFound, length: 0)
            rect = client.firstRect(forCharacterRange: NSRange(location: markedRange.location + selection.location, length: selection.length),
                                    actualRange: &actual)
            via = "first-marked"
        } else {
            let selected = client.selectedRange()
            if selected.location != NSNotFound {
                var actual = NSRange(location: NSNotFound, length: 0)
                rect = client.firstRect(forCharacterRange: NSRange(location: selected.location, length: 0), actualRange: &actual)
                via = "first-selected"
            }
        }
        if rect == .zero {
            // IMKInputSession.h: "The index is relative to the inline session ... if there is no
            // inline session the value of index should be 0" -- so the position inside the
            // composition (0 without one), not the document position.
            var line = NSRect.zero
            _ = client.attributes(forCharacterIndex: controller.shown == nil ? 0 : selection.location, lineHeightRectangle: &line)
            rect = line
            via = "line"
        }
        let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
        let screen = NSScreen.screens.first { $0.frame.contains(rect.origin) } ?? NSScreen.screens.first
        return [
            "op": "caret", "ok": rect != .zero, "rect_ok": rect != .zero, "via": via,
            "x": rect.origin.x, "y": rect.origin.y, "w": rect.size.width, "h": rect.size.height,
            "top": primaryHeight - (rect.origin.y + rect.size.height),
            "scale": screen?.backingScaleFactor ?? 0,
            "marked_location": markedRange.location == NSNotFound ? -1 : markedRange.location,
            "secure_input": IsSecureEventInputEnabled(),
        ]
    }
}

@objc(IstessoDevInputController)
final class IstessoDevInputController: IMKInputController {
    var clientIdentifier: String { client()?.bundleIdentifier() ?? "?" }
    /// The composition this session shows now, and the slot of the host that sent it.
    var shown: Shown?

    /// No events: this Mac's own keys never come here.
    override func recognizedEvents(_ sender: Any!) -> Int { 0 }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool { false }

    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        // A composition another session still shows ends there before this one takes over.
        if let other = Link.shared.showing, other !== self { Link.shared.applicationEnded(other) }
        Link.shared.active = self
        Link.shared.announce()
    }

    override func deactivateServer(_ sender: Any!) {
        Link.shared.applicationEnded(self)
        if Link.shared.active === self {
            Link.shared.active = nil
            Link.shared.announce()
        }
        super.deactivateServer(sender)
    }

    override func commitComposition(_ sender: Any!) {
        Link.shared.applicationEnded(self)
    }

    /// The selected clause underlined thick, the rest thin, as the system's input methods draw them;
    /// raw input (an empty selection) all thin.
    func styled(_ mark: Commands.Mark) -> NSAttributedString {
        let whole = NSRange(location: 0, length: mark.text.utf16.count)
        let out = NSMutableAttributedString(attributedString: Self.japanese(mark.text))
        func attributes(_ style: Int, _ range: NSRange) -> [NSAttributedString.Key: Any] {
            var result: [NSAttributedString.Key: Any] = [:]
            for (key, value) in self.mark(forStyle: style, at: range) ?? [:] {
                if let key = key as? String { result[NSAttributedString.Key(key)] = value }
            }
            return result
        }
        if mark.selection.length == 0 {
            out.addAttributes(attributes(Int(kTSMHiliteRawText), whole), range: whole)
        } else {
            out.addAttributes(attributes(Int(kTSMHiliteConvertedText), whole), range: whole)
            out.addAttributes(attributes(Int(kTSMHiliteSelectedConvertedText), mark.selection), range: mark.selection)
        }
        return out
    }

    /// Text tagged Japanese, so that a client that keeps the attribute draws Han characters with
    /// Japanese glyphs (the input source itself declares no language).
    static func japanese(_ text: String) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTLanguageAttributeName as String): "ja"])
    }
}

// MARK: - Start

/// `--wire-test <socket>`: connects to the given socket, checks the peer, and answers mark and
/// commit with what it would have shown (lengths and UTF-16 selection only), without InputMethodKit.
func wireTest(_ path: String) -> Int32 {
    guard let fd = Wire.connect(path) else { print("wire-test: socket absent or not private"); return 3 }
    switch Wire.checkPeer(fd, identifiers: Wire.hostIdentifiers) {
    case .refused(let why): print("wire-test: host refused: \(why)"); return 4
    case .ok(let identifier): print("wire-test: host is \(identifier)")
    }
    let reader = Wire.LineReader()
    var buffer = [UInt8](repeating: 0, count: 4096)
    while true {
        let n = read(fd, &buffer, buffer.count)
        if n <= 0 { return 0 }
        guard let messages = reader.feed(Data(buffer[0..<n])) else { return 5 }
        for message in messages {
            var reply: [String: Any]
            switch message["op"] as? String {
            case "mark":
                switch Commands.mark(message) {
                case .success(let m): reply = ["op": "mark", "ok": true, "units": m.text.utf16.count, "sel16": [m.selection.location, m.selection.length]]
                case .failure(let r): reply = ["op": "mark", "ok": false, "why": r.field]
                }
            case "commit":
                switch Commands.commit(message) {
                case .success(let t): reply = ["op": "commit", "ok": true, "units": t.text.utf16.count, "comp": t.serial]
                case .failure(let r): reply = ["op": "commit", "ok": false, "why": r.field]
                }
            case "bye": return 0
            default: reply = ["op": "unknown", "ok": false]
            }
            if let data = Wire.encode(reply) { _ = Wire.writeAll(fd, data) }
        }
    }
}

let arguments = CommandLine.arguments
if arguments.count == 3 && arguments[1] == "--wire-test" {
    exit(wireTest(arguments[2]))
}

guard let connectionName = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String else {
    log.error("no InputMethodConnectionName")
    exit(1)
}
let server = IMKServer(name: connectionName, bundleIdentifier: Bundle.main.bundleIdentifier)
log.info("started")
Link.shared.start()
NSApplication.shared.run()
_ = server
