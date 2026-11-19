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

/// The connection to the host, and which controller shows which composition.
///
/// Each controller (one per text session of a client) keeps its own composition. A composition
/// ends exactly once: committed by the host, committed by the application (a click elsewhere, the
/// focus moving), or cleared (the host gone). Its serial is then remembered, so that a commit or a
/// mark that crossed the end on the wire is dropped instead of landing a second time.
final class Link {
    static let shared = Link()
    weak var active: IstessoDevInputController?
    /// The controller showing a composition now (it may no longer be the active one).
    weak var showing: IstessoDevInputController?
    private var ended: [Int] = []
    private var fd: Int32 = -1
    private var source: DispatchSourceRead?
    private var reader = Wire.LineReader()
    private var timer: Timer?
    private let nowhere = NSRange(location: NSNotFound, length: NSNotFound)

    func start() {
        tryConnect()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            guard let self, self.fd < 0 else { return }
            self.tryConnect()
        }
    }

    private func tryConnect() {
        guard let path = Wire.defaultSocketPath(), let fd = Wire.connect(path) else { return }
        switch Wire.checkPeer(fd, identifiers: Wire.hostIdentifiers) {
        case .refused(let why):
            log.error("host refused: \(why, privacy: .public)")
            close(fd)
            return
        case .ok(let identifier):
            log.info("connected to \(identifier, privacy: .public)")
        }
        self.fd = fd
        reader = Wire.LineReader()
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in self?.readable() }
        source.setCancelHandler { close(fd) }
        source.resume()
        self.source = source
        if let active { send(active.activeMessage()) }
    }

    private func end(_ serial: Int) {
        ended.append(serial)
        if ended.count > 64 { ended.removeFirst(ended.count - 64) }
    }

    /// The host went away (the viewer disconnected, the host stopped): what is composing is
    /// cleared, never committed and never left behind in the application.
    private func disconnect() {
        if let controller = showing, let shown = controller.shown {
            controller.client()?.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
            controller.shown = nil
            end(shown.serial)
            log.info("host gone: composing text cleared")
        }
        showing = nil
        source?.cancel()
        source = nil
        fd = -1
    }

    private func readable() {
        var buffer = [UInt8](repeating: 0, count: 4096)
        let n = read(fd, &buffer, buffer.count)
        if n <= 0 { log.info("host closed"); disconnect(); return }
        guard let messages = reader.feed(Data(buffer[0..<n])) else { log.error("line too long"); disconnect(); return }
        for message in messages { handle(message) }
    }

    func send(_ object: [String: Any], seq: Any? = nil) {
        var object = object
        if let seq = seq as? Int { object["seq"] = seq }
        guard fd >= 0, let data = Wire.encode(object) else { return }
        if !Wire.writeAll(fd, data) { disconnect() }
    }

    private func handle(_ message: [String: Any]) {
        let op = message["op"] as? String ?? ""
        let seq = message["seq"]
        guard let controller = active, let client = controller.client() else {
            send(["op": op, "ok": false, "why": "no active client"], seq: seq)
            return
        }
        switch op {
        case "mark":
            // A password field (secure event input on): never show composing text there. The host
            // tells the client, which then sends plain keys without its input method.
            if IsSecureEventInputEnabled() {
                send(["op": "mark", "ok": false, "why": "secure", "secure": true], seq: seq)
                return
            }
            switch Commands.mark(message) {
            case .failure(let refusal): send(["op": "mark", "ok": false, "why": refusal.field], seq: seq)
            case .success(let mark):
                if ended.contains(mark.serial) {
                    send(["op": "mark", "ok": false, "why": "stale", "comp": mark.serial], seq: seq)
                    return
                }
                // A composition still shown in another session (its deactivation not come yet)
                // ends there first, so that it lands in its own application.
                if let other = showing, other !== controller { applicationEnded(other) }
                if mark.text.isEmpty {
                    client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
                    controller.shown = nil
                    showing = nil
                } else {
                    client.setMarkedText(controller.styled(mark), selectionRange: mark.selection, replacementRange: nowhere)
                    controller.shown = mark
                    showing = controller
                }
                log.info("mark: \(mark.text.utf16.count) units")
                send(caret(client, controller), seq: seq)
            }
        case "commit":
            switch Commands.commit(message) {
            case .failure(let refusal): send(["op": "commit", "ok": false, "why": refusal.field], seq: seq)
            case .success(let commit):
                // Ended already (committed by the application, or cleared): it landed or was
                // dropped once; never again.
                if ended.contains(commit.serial) {
                    send(["op": "commit", "ok": false, "why": "stale", "comp": commit.serial], seq: seq)
                    return
                }
                // Where the composition is shown (the session it was typed into), or else where the
                // caret is now (a commit with no composition before it).
                let target = (showing?.shown?.serial == commit.serial ? showing : nil) ?? controller
                guard let targetClient = target.client() else {
                    send(["op": "commit", "ok": false, "why": "no client"], seq: seq)
                    return
                }
                if let shown = target.shown, shown.serial != commit.serial {
                    send(["op": "commit", "ok": false, "why": "other composition", "comp": commit.serial], seq: seq)
                    return
                }
                let at = target.shown != nil ? targetClient.markedRange().location : targetClient.selectedRange().location
                targetClient.insertText(IstessoDevInputController.japanese(commit.text), replacementRange: nowhere)
                target.shown = nil
                if showing === target { showing = nil }
                end(commit.serial)
                log.info("commit: \(commit.text.utf16.count) units")
                // "inserted": the answer is made after a call that returns a value from the client
                // (caret() asks for its ranges), which the client answers only after the insert
                // before it. Keys the host posts after a commit wait for this (bounded), so that text
                // and keys never change places.
                var answer = caret(targetClient, target)
                answer["op"] = "commit"
                answer["inserted"] = true
                answer["comp"] = commit.serial
                #if ISTESSO_TRIAL_CHECK
                answer["at"] = at == NSNotFound ? -1 : at
                #else
                _ = at
                #endif
                send(answer, seq: seq)
            }
        case "clear":
            if let shown = controller.shown { end(shown.serial) }
            client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
            controller.shown = nil
            if showing === controller { showing = nil }
            send(caret(client, controller), seq: seq)
        case "caret":
            send(caret(client, controller), seq: seq)
        #if ISTESSO_TRIAL_CHECK
        case "check":
            send(check(client), seq: seq)
        #endif
        default:
            send(["op": op, "ok": false, "why": "unknown"], seq: seq)
        }
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

    /// The application ended the composition itself (a click elsewhere, another window): what was
    /// shown is committed once into that same session, as an input method does, and the host is
    /// told so that the client ends its own composition without committing it again.
    func applicationEnded(_ controller: IstessoDevInputController) {
        guard let shown = controller.shown else { return }
        controller.shown = nil
        if showing === controller { showing = nil }
        end(shown.serial)
        controller.client()?.insertText(IstessoDevInputController.japanese(shown.text), replacementRange: nowhere)
        send(["op": "ended", "by": "application", "comp": shown.serial])
    }

    /// The caret's rectangle in global points, AppKit's way round (origin at the bottom left of the
    /// primary screen). The host turns it into picture pixels of the window it sends (ADR 0015,
    /// kind 5): y from the top = primary height - (y + h), less the window's origin, times the
    /// window's pixels per point.
    func caret(_ client: IMKTextInput & NSObjectProtocol, _ controller: IstessoDevInputController) -> [String: Any] {
        var rect = NSRect.zero
        var via = "none"
        let markedRange = client.markedRange()
        let selection = controller.shown?.selection ?? NSRange(location: 0, length: 0)
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
            var line = NSRect.zero
            _ = client.attributes(forCharacterIndex: selection.location, lineHeightRectangle: &line)
            rect = line
            via = "line"
        }
        let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
        let screen = NSScreen.screens.first { $0.frame.contains(rect.origin) } ?? NSScreen.screens.first
        return [
            "op": "caret", "ok": rect != .zero, "via": via,
            "x": rect.origin.x, "y": rect.origin.y, "w": rect.size.width, "h": rect.size.height,
            "top": primaryHeight - (rect.origin.y + rect.size.height),
            "scale": screen?.backingScaleFactor ?? 0,
            "marked_location": markedRange.location == NSNotFound ? -1 : markedRange.location,
            "secure": IsSecureEventInputEnabled(),
        ]
    }
}

@objc(IstessoDevInputController)
final class IstessoDevInputController: IMKInputController {
    var clientIdentifier: String { client()?.bundleIdentifier() ?? "?" }
    /// The composition this session shows now.
    var shown: Commands.Mark?

    /// "active" with the client's application and where its caret is: the host goes inline only
    /// when this names the application it shows and the caret lies inside the shown window.
    func activeMessage() -> [String: Any] {
        var message: [String: Any] = ["client": clientIdentifier]
        if let client = client() {
            for (key, value) in Link.shared.caret(client, self) where key != "op" { message[key] = value }
        }
        message["op"] = "active"
        return message
    }

    /// No events: this Mac's own keys never come here.
    override func recognizedEvents(_ sender: Any!) -> Int { 0 }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool { false }

    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        // A composition another session still shows ends there before this one takes over.
        if let other = Link.shared.showing, other !== self { Link.shared.applicationEnded(other) }
        Link.shared.active = self
        log.info("active: \(self.clientIdentifier, privacy: .public)")
        Link.shared.send(activeMessage())
    }

    override func deactivateServer(_ sender: Any!) {
        Link.shared.applicationEnded(self)
        if Link.shared.active === self {
            Link.shared.active = nil
            Link.shared.send(["op": "inactive"])
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
