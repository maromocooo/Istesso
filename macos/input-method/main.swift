// Istesso Dev Input (com.maromoco.istesso-dev.inputmethod): a trial input source that shows the
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
import InputMethodKit
import os

let log = Logger(subsystem: Wire.inputMethodIdentifier, category: "link")

/// What the input method does with a message from the host, kept apart from InputMethodKit so the
/// wire test can run it without a client.
enum Commands {
    struct Mark { let text: String; let selection: NSRange }

    /// A "mark" message as text and a UTF-16 selection, or the reason it is refused.
    static func mark(_ message: [String: Any]) -> Result<Mark, Refusal> {
        guard let text = message["text"] as? String, Wire.acceptable(text, allowEmpty: true) else { return .failure(.init("text")) }
        guard let cursor = message["sel"] as? [Int], cursor.count == 2, cursor[0] <= cursor[1],
              let start = Wire.utf16Offset(text, utf8: cursor[0]), let end = Wire.utf16Offset(text, utf8: cursor[1]) else {
            return .failure(.init("sel"))
        }
        return .success(Mark(text: text, selection: NSRange(location: start, length: end - start)))
    }

    static func commit(_ message: [String: Any]) -> Result<String, Refusal> {
        guard let text = message["text"] as? String, Wire.acceptable(text, allowEmpty: false) else { return .failure(.init("text")) }
        return .success(text)
    }

    struct Refusal: Error { let field: String; init(_ field: String) { self.field = field } }
}

/// The connection to the host, and the controller of the client that is active now.
final class Link {
    static let shared = Link()
    weak var active: IstessoDevInputController?
    private var fd: Int32 = -1
    private var source: DispatchSourceRead?
    private var reader = Wire.LineReader()
    private var timer: Timer?
    /// The marked text shown now (to commit it when the application ends the composition) and its
    /// selection in UTF-16.
    private(set) var marked: Commands.Mark?

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
        if let active { send(["op": "active", "client": active.clientIdentifier]) }
    }

    private func disconnect() {
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

    func send(_ object: [String: Any]) {
        guard fd >= 0, let data = Wire.encode(object) else { return }
        if !Wire.writeAll(fd, data) { disconnect() }
    }

    private func handle(_ message: [String: Any]) {
        let op = message["op"] as? String ?? ""
        guard let controller = active, let client = controller.client() else {
            send(["op": op, "ok": false, "why": "no active client"])
            return
        }
        let nowhere = NSRange(location: NSNotFound, length: NSNotFound)
        switch op {
        case "mark":
            // A password field (secure event input on): never show composing text there. The host
            // tells the client, which then sends plain keys without its input method.
            if IsSecureEventInputEnabled() {
                send(["op": "mark", "ok": false, "why": "secure", "secure": true])
                return
            }
            switch Commands.mark(message) {
            case .failure(let refusal): send(["op": "mark", "ok": false, "why": refusal.field])
            case .success(let mark):
                if mark.text.isEmpty {
                    client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
                    marked = nil
                } else {
                    client.setMarkedText(controller.styled(mark), selectionRange: mark.selection, replacementRange: nowhere)
                    marked = mark
                }
                log.info("mark: \(mark.text.utf16.count) units")
                send(caret(client))
            }
        case "commit":
            switch Commands.commit(message) {
            case .failure(let refusal): send(["op": "commit", "ok": false, "why": refusal.field])
            case .success(let text):
                client.insertText(text, replacementRange: nowhere)
                marked = nil
                log.info("commit: \(text.utf16.count) units")
                send(caret(client))
            }
        case "clear":
            client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: nowhere)
            marked = nil
            send(caret(client))
        case "caret":
            send(caret(client))
        default:
            send(["op": op, "ok": false, "why": "unknown"])
        }
    }

    /// The application ended the composition itself (a click elsewhere, another window): what was
    /// shown is committed, as an input method does, and the host is told so that the client can end
    /// its own composition too.
    func applicationEnded(_ controller: IstessoDevInputController) {
        guard controller === active, let mark = marked, let client = controller.client() else { return }
        client.insertText(mark.text, replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
        marked = nil
        send(["op": "ended", "by": "application"])
    }

    /// The caret's rectangle in global points, AppKit's way round (origin at the bottom left of the
    /// primary screen). The host turns it into picture pixels of the window it sends (ADR 0015,
    /// kind 5): y from the top = primary height - (y + h), less the window's origin, times the
    /// window's pixels per point.
    func caret(_ client: IMKTextInput & NSObjectProtocol) -> [String: Any] {
        var rect = NSRect.zero
        var via = "none"
        let markedRange = client.markedRange()
        let selection = marked?.selection ?? NSRange(location: 0, length: 0)
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

    /// No events: this Mac's own keys never come here.
    override func recognizedEvents(_ sender: Any!) -> Int { 0 }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool { false }

    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        Link.shared.active = self
        log.info("active: \(self.clientIdentifier, privacy: .public)")
        Link.shared.send(["op": "active", "client": clientIdentifier])
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
        let out = NSMutableAttributedString(string: mark.text)
        func attributes(_ style: Int, _ range: NSRange) -> [NSAttributedString.Key: Any] {
            var result: [NSAttributedString.Key: Any] = [:]
            for (key, value) in self.mark(forStyle: style, at: range) ?? [:] {
                if let key = key as? String { result[NSAttributedString.Key(key)] = value }
            }
            return result
        }
        if mark.selection.length == 0 {
            out.setAttributes(attributes(Int(kTSMHiliteRawText), whole), range: whole)
        } else {
            out.setAttributes(attributes(Int(kTSMHiliteConvertedText), whole), range: whole)
            out.setAttributes(attributes(Int(kTSMHiliteSelectedConvertedText), mark.selection), range: mark.selection)
        }
        return out
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
                case .success(let t): reply = ["op": "commit", "ok": true, "units": t.utf16.count]
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
