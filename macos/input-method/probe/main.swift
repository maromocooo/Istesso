// istesso-ime-probe (com.maromoco.istesso-dev.ime-probe): the host's side of the link to Istesso Dev
// Input, for trying it before the Mac host takes it over (trial only; see build.sh).
//
//   istesso-ime-probe status              read only: is the input source installed, enabled, selected
//   istesso-ime-probe serve [script]      owns the socket, waits for the input method, runs commands
//                                         from the script (or standard input), prints its answers
//   istesso-ime-probe register            TISRegisterInputSource on the placed bundle (no enable,
//                                         no select)
//   istesso-ime-probe select              selects Istesso Dev Input (after the owner added it),
//                                         writing down the source selected before
//   istesso-ime-probe restore             puts that source back ("restored: yes/no")
//   istesso-ime-probe secure              is the focused element a password field (needs
//                                         Accessibility; says "unknown" without it), and is secure
//                                         event input on
//   istesso-ime-probe self-test <input method binary> [stand-in]
//                                         the link without InputMethodKit: peer checks both ways,
//                                         a refused stranger, a socket that is not private, the
//                                         byte-to-UTF-16 selection
//
// Commands: `mark <sel start byte> <sel end byte> <text>`, `commit <text>`, `clear`, `caret`,
// `wait <seconds>`, `connected <seconds> <idle seconds>`, `burst <n> ack|noack`, `click <dx> <dy>`, `drop`, `post-access`,
// `check at <location> <text>`, `check marked <text>` / `check before <text>` (TextEdit only: does the marked
// text, or the text just before the caret, hash the same as <text>). Text is never printed back: the answers carry lengths and rectangles only.
import AppKit
import ApplicationServices
import Carbon
import CryptoKit
import Foundation

func fail(_ text: String) -> Never {
    FileHandle.standardError.write(Data("istesso-ime-probe: \(text)\n".utf8))
    exit(2)
}

// MARK: - Input sources

func ourSources() -> [TISInputSource] {
    let filter = [kTISPropertyBundleID as String: Wire.inputMethodIdentifier] as CFDictionary
    return (TISCreateInputSourceList(filter, true)?.takeRetainedValue() as? [TISInputSource]) ?? []
}

func property<T>(_ source: TISInputSource, _ key: CFString) -> T? {
    guard let raw = TISGetInputSourceProperty(source, key) else { return nil }
    return Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? T
}

func currentSourceID() -> String {
    let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    return property(current, kTISPropertyInputSourceID) ?? "?"
}

func status() {
    let sources = ourSources()
    print("installed: \(!sources.isEmpty)")
    for source in sources {
        let id: String = property(source, kTISPropertyInputSourceID) ?? "?"
        let enabled: Bool = property(source, kTISPropertyInputSourceIsEnabled) ?? false
        let selected: Bool = property(source, kTISPropertyInputSourceIsSelected) ?? false
        let selectable: Bool = property(source, kTISPropertyInputSourceIsSelectCapable) ?? false
        let languages: [String] = property(source, kTISPropertyInputSourceLanguages) ?? []
        let ascii: Bool = property(source, kTISPropertyInputSourceIsASCIICapable) ?? false
        let name: String = property(source, kTISPropertyLocalizedName) ?? "?"
        let enableCapable: Bool = property(source, kTISPropertyInputSourceIsEnableCapable) ?? false
        let category: String = property(source, kTISPropertyInputSourceCategory) ?? "?"
        let type: String = property(source, kTISPropertyInputSourceType) ?? "?"
        print("source: \(id) name=\"\(name)\" enabled=\(enabled) selected=\(selected) select_capable=\(selectable) languages=\(languages) ascii_capable=\(ascii) enable_capable=\(enableCapable) category=\(category) type=\(type)")
    }
    print("current: \(currentSourceID())")
    print("secure_event_input: \(IsSecureEventInputEnabled())")
}

/// Tells the system about the bundle the owner allowed to be placed (instead of logging out). It
/// neither enables nor selects it.
func register() {
    let url = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Input Methods/Istesso Dev Input.app")
    let result = TISRegisterInputSource(url as CFURL)
    print("register: \(result == noErr ? "ok" : "error \(result)")")
}

func select() {
    guard let source = ourSources().first(where: { property($0, kTISPropertyInputSourceIsEnabled) ?? false }) else {
        fail("Istesso Dev Input is not enabled: the owner adds it in System Settings > Keyboard > Input Sources")
    }
    _ = source
    print("select: \(switchToIstesso() ? "ok" : "not effective")")
}

/// `restore`: puts back the source written down by `select` or a connection.
func restoreCommand() { restoreOriginal("asked") }

// MARK: - Switching while a viewer is connected (ADR 0023, 2.)

/// Where the source selected before Istesso's is kept while Istesso's is selected, so that the
/// next start after a crash puts it back. In the host's private socket folder (0700).
func previousSourceFile() -> String? {
    guard let path = Wire.defaultSocketPath() else { return nil }
    return ((path as NSString).deletingLastPathComponent as NSString).appendingPathComponent("previous-source")
}

func source(withID id: String) -> TISInputSource? {
    let filter = [kTISPropertyInputSourceID as String: id] as CFDictionary
    return (TISCreateInputSourceList(filter, false)?.takeRetainedValue() as? [TISInputSource])?.first
}

/// Selects Istesso's source, after writing down the one selected now. True when the system says
/// Istesso's is the current source afterwards (the menu-bar icon alone is not enough: the host goes
/// inline only after the input source itself says "active").
func switchToIstesso() -> Bool {
    guard let ours = source(withID: Wire.inputMethodIdentifier), let file = previousSourceFile() else { return false }
    let before = currentSourceID()
    if before != Wire.inputMethodIdentifier {
        guard (try? before.write(toFile: file, atomically: true, encoding: .utf8)) != nil else { return false }
        chmod(file, 0o600)
    }
    _ = TISSelectInputSource(ours)
    return currentSourceID() == Wire.inputMethodIdentifier
}

/// Puts back the source written down before, and forgets it. "restored" is read back from the
/// system; the identifier itself is not printed.
@discardableResult
func restoreOriginal(_ why: String) -> Bool {
    guard let file = previousSourceFile(), let id = try? String(contentsOfFile: file, encoding: .utf8) else { return true }
    var restored = false
    if let original = source(withID: id) {
        _ = TISSelectInputSource(original)
        restored = currentSourceID() == id
    }
    if restored { unlink(file) }
    print("restored (\(why)): \(restored ? "yes" : "no")")
    return restored
}

/// Seconds since this Mac's own keyboard was last pressed (the HID system's state: keys the host
/// posts to a process do not count).
func localKeyboardIdle() -> Double {
    CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: .keyDown)
}


/// The focused element's subrole, read through Accessibility: a password field must never get
/// composing text (the client then sends plain keys without its input method).
func secure() {
    print("secure_event_input: \(IsSecureEventInputEnabled())")
    guard AXIsProcessTrusted() else { print("focused_secure: unknown (no Accessibility)"); return }
    let system = AXUIElementCreateSystemWide()
    var focused: CFTypeRef?
    guard AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
          let element = focused, CFGetTypeID(element) == AXUIElementGetTypeID() else {
        print("focused_secure: unknown (no focused element)")
        return
    }
    var subrole: CFTypeRef?
    _ = AXUIElementCopyAttributeValue(element as! AXUIElement, kAXSubroleAttribute as CFString, &subrole)
    print("focused_secure: \((subrole as? String) == (kAXSecureTextFieldSubrole as String))")
}

// MARK: - The socket

/// Makes the private folder (0700, this user's, not a link) and a fresh socket in it (0600 from the
/// start: made under umask 0177).
func listen(at path: String) -> Int32 {
    let folder = (path as NSString).deletingLastPathComponent
    if mkdir(folder, 0o700) != 0 && errno != EEXIST { fail("folder not made") }
    var info = stat()
    guard lstat(folder, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR, info.st_uid == getuid(), (info.st_mode & 0o077) == 0 else {
        fail("socket folder is not private")
    }
    if lstat(path, &info) == 0 {
        guard (info.st_mode & S_IFMT) == S_IFSOCK else { fail("something that is not a socket is in the way") }
        unlink(path)
    }
    guard var address = Wire.socketAddress(path) else { fail("socket path too long") }
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { fail("no socket") }
    let old = umask(0o177)
    let bound = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    umask(old)
    guard bound == 0, chmod(path, 0o600) == 0, Darwin.listen(fd, 4) == 0 else { fail("socket not bound") }
    return fd
}

/// Waits for a connection from Istesso Dev Input; strangers are refused and closed.
func acceptInputMethod(_ listener: Int32, timeout seconds: Double) -> Int32? {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        var poller = pollfd(fd: listener, events: Int16(POLLIN), revents: 0)
        if poll(&poller, 1, 250) <= 0 { continue }
        let fd = accept(listener, nil, nil)
        if fd < 0 { continue }
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        switch Wire.checkPeer(fd, identifiers: [Wire.inputMethodIdentifier]) {
        case .ok(let identifier):
            print("peer: \(identifier)")
            return fd
        case .refused(let why):
            print("refused: \(why)")
            close(fd)
        }
    }
    return nil
}

/// Reads answers until one with the given op (or a timeout); prints every answer.
func answers(_ fd: Int32, _ reader: Wire.LineReader, until op: String?, timeout: Double) -> [[String: Any]] {
    var got: [[String: Any]] = []
    let deadline = Date().addingTimeInterval(timeout)
    var buffer = [UInt8](repeating: 0, count: 4096)
    while Date() < deadline {
        var poller = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        if poll(&poller, 1, 100) <= 0 { if op == nil && !got.isEmpty { break }; continue }
        let n = read(fd, &buffer, buffer.count)
        if n <= 0 { print("closed"); break }
        guard let messages = reader.feed(Data(buffer[0..<n])) else { print("line too long"); break }
        for message in messages {
            show(message)
            got.append(message)
        }
        if let op, got.contains(where: { $0["op"] as? String == op }) { break }
    }
    return got
}

func digest(_ text: String) -> String {
    SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
}

func command(_ line: String) -> [String: Any]? {
    let parts = line.split(separator: " ", maxSplits: 3, omittingEmptySubsequences: true).map(String.init)
    switch parts.first {
    case "mark" where parts.count == 4:
        guard let start = Int(parts[1]), let end = Int(parts[2]) else { return nil }
        return ["op": "mark", "text": parts[3], "sel": [start, end]]
    case "mark" where parts.count == 3 && parts[1] == "0" && parts[2] == "0":
        return ["op": "mark", "text": "", "sel": [0, 0]]
    case "commit" where parts.count >= 2:
        return ["op": "commit", "text": parts.dropFirst().joined(separator: " ")]
    case "clear": return ["op": "clear"]
    case "caret": return ["op": "caret"]
    case "check" where parts.count >= 3 && parts[1] == "marked":
        // The text is only hashed here; the input method answers with the hash of the marked text
        // it reads. Neither hash is ever printed: only whether they match.
        return ["op": "check", "expect": digest(parts.dropFirst(2).joined(separator: " "))]
    case "check" where parts.count == 2 && parts[1] == "none":
        return ["op": "check"]
    default: return nil
    }
}

/// One answer, printed without any hash.
func show(_ message: [String: Any]) {
    var message = message
    message.removeValue(forKey: "sha256")
    if let data = try? JSONSerialization.data(withJSONObject: message, options: [.sortedKeys]) {
        print("answer: \(String(decoding: data, as: UTF8.self))")
    }
}

final class Session {
    let listener: Int32
    var fd: Int32
    var reader = Wire.LineReader()
    var seq = 0
    /// The composition serial, as the client keeps it: one per composition, a new one after each
    /// commit, clear, or end by the application.
    var comp = 1
    /// The last caret answer (global points, top from the primary screen's top).
    var lastCaret: [String: Any] = [:]

    init(listener: Int32, fd: Int32) { self.listener = listener; self.fd = fd }

    func read(until: (([String: Any]) -> Bool)?, timeout: Double) -> [[String: Any]] {
        var got: [[String: Any]] = []
        let deadline = Date().addingTimeInterval(timeout)
        var buffer = [UInt8](repeating: 0, count: 4096)
        while Date() < deadline {
            var poller = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            if poll(&poller, 1, 20) <= 0 { if until == nil && !got.isEmpty { break }; continue }
            let n = Darwin.read(fd, &buffer, buffer.count)
            if n <= 0 { print("closed"); break }
            guard let messages = reader.feed(Data(buffer[0..<n])) else { print("line too long"); break }
            for message in messages {
                show(message)
                if ["caret", "active", "mark", "commit"].contains(message["op"] as? String ?? ""), message["x"] != nil { lastCaret = message }
                if message["op"] as? String == "ended" { comp += 1 }
                got.append(message)
            }
            if let until, got.contains(where: until) { break }
        }
        return got
    }

    /// Sends one message with a sequence number; waits (bounded) for the answer with that number.
    @discardableResult
    func ask(_ message: [String: Any], wait: Double = 2) -> [String: Any]? {
        seq += 1
        var message = message
        message["seq"] = seq
        let want = seq
        guard let data = Wire.encode(message), Wire.writeAll(fd, data) else { print("closed"); return nil }
        return read(until: { ($0["seq"] as? Int) == want }, timeout: wait).last { ($0["seq"] as? Int) == want }
    }

    func run(_ line: String) {
        let words = line.split(separator: " ").map(String.init)
        switch words.first {
        case "wait":
            let seconds = Double(words.dropFirst().first ?? "1") ?? 1
            print("wait \(seconds)")
            _ = read(until: nil, timeout: seconds)
        case "post-access":
            print("post_event_access: \(CGPreflightPostEventAccess())")
        case "burst":
            guard mayPost() else { print("burst: not reachable (no permission to post events; nothing asked)"); return }
            // `burst <n> ack|noack [keycode]`: n times a commit of one kana (the input-source path)
            // and at once a key (the CGEvent path, default "a") posted to TextEdit, with or without
            // holding the key until the commit's "inserted" answer (at most 50 ms; a timeout is sent
            // anyway and counted). In order, commit i lands at the first one's place + 2 i (one
            // unit of kana, one of the key's character): reordering shows in the places, no text is read.
            guard words.count >= 3, let n = Int(words[1]), let pid = textEditPid() else { print("burst: no TextEdit or bad arguments"); return }
            let withAck = words[2] == "ack"
            let keyCode = CGKeyCode(words.count == 4 ? Int(words[3]) ?? 0 : 0)
            let began = Date()
            var places: [Int: Int] = [:]
            var timeouts = 0
            var waits: [Double] = []
            for _ in 0..<n {
                seq += 1
                let want = seq
                _ = Wire.writeAll(fd, Wire.encode(["op": "commit", "text": "あ", "comp": comp, "seq": want])!)
                comp += 1
                if withAck {
                    let t0 = Date()
                    let got = read(until: { ($0["seq"] as? Int) == want }, timeout: 0.05)
                    for m in got { if let s = m["seq"] as? Int, let at = m["at"] as? Int { places[s] = at } }
                    if !got.contains(where: { ($0["seq"] as? Int) == want }) { timeouts += 1 }
                    waits.append(Date().timeIntervalSince(t0) * 1000)
                }
                postKey(pid, keyCode: keyCode)
            }
            for m in read(until: nil, timeout: 1.0) { if let s = m["seq"] as? Int, let at = m["at"] as? Int { places[s] = at } }
            let first = seq - n + 1
            var outOfOrder = 0
            if let base = places[first] {
                for i in 0..<n { if places[first + i] != base + 2 * i { outOfOrder += 1 } }
            } else { outOfOrder = -1 }
            waits.sort()
            let median = waits.isEmpty ? 0 : waits[waits.count / 2]
            print("burst: \(n) in \(Int(Date().timeIntervalSince(began) * 1000)) ms, places known \(places.count), out of order \(outOfOrder), ack timeouts \(timeouts), ack wait median \(String(format: "%.2f", median)) ms max \(String(format: "%.2f", waits.last ?? 0)) ms")
        case "click":
            guard mayPost() else { print("click: not reachable (no permission to post events; nothing asked)"); return }
            // `click <dx> <dy>`: a click at the last caret position moved by dx, dy points.
            guard words.count == 3, let dx = Double(words[1]), let dy = Double(words[2]), let pid = textEditPid(),
                  let x = lastCaret["x"] as? Double, let top = lastCaret["top"] as? Double, let h = lastCaret["h"] as? Double else {
                print("click: no caret yet or bad arguments"); return
            }
            postClick(pid, at: CGPoint(x: x + dx, y: top + h / 2 + dy))
            print("click: posted")
            _ = read(until: nil, timeout: 0.5)
        case "select-istesso":
            // Istesso's source, writing down the one before; then wait for the input source's own
            // "active" (the icon alone is not enough).
            let effective = switchToIstesso()
            print("select: system says current is Istesso's: \(effective)")
            let got = read(until: { $0["op"] as? String == "active" }, timeout: 3)
            print("active after select: \(got.contains { $0["op"] as? String == "active" })")
        case "keymark":
            guard mayPost() else { print("keymark: not reachable (no permission to post events; nothing asked)"); return }
            // `keymark <n> [keycode]`: a key (default "a"; 36 is Return) and at once a mark of one
            // kana, then its commit (waiting for "inserted"). In order, the commit lands one unit
            // after the key's character: start + 2 i + 1.
            guard words.count >= 2, let n = Int(words[1]), let pid = textEditPid() else { print("keymark: no TextEdit or bad arguments"); return }
            let keyCode = CGKeyCode(words.count == 3 ? Int(words[2]) ?? 0 : 0)
            guard let start = (ask(["op": "check"])?["selected"] as? [Int])?.first, start >= 0 else { print("keymark: no selection"); return }
            var outOfOrder = 0, unknown = 0
            for i in 0..<n {
                postKey(pid, keyCode: keyCode)
                _ = ask(["op": "mark", "text": "あ", "sel": [3, 3], "comp": comp], wait: 0.5)
                let answer = ask(["op": "commit", "text": "あ", "comp": comp], wait: 0.5)
                comp += 1
                guard let at = answer?["at"] as? Int else { unknown += 1; continue }
                if at != start + 2 * i + 1 { outOfOrder += 1 }
            }
            print("keymark: \(n) with key \(keyCode), out of order \(outOfOrder), unknown \(unknown)")
        case "longmark":
            // `longmark <n>`: a composition of n kana (a long live conversion); over the limit it is
            // refused and the composition goes to the strip.
            guard words.count == 2, let n = Int(words[1]) else { return }
            let text = String(repeating: "あ", count: n)
            let answer = ask(["op": "mark", "text": text, "sel": [text.utf8.count, text.utf8.count], "comp": comp])
            let ok = answer?["ok"] as? Bool ?? false
            print("longmark \(n): \(ok ? "inline" : "refused (\(answer?["why"] as? String ?? "?")) -> strip")")
            if ok { _ = ask(["op": "clear"]); comp += 1 }
        case "recommit":
            // A commit of the composition just ended (as if it crossed "ended" on the wire): must
            // be refused as stale.
            guard words.count >= 2 else { return }
            let answer = ask(["op": "commit", "text": words.dropFirst().joined(separator: " "), "comp": comp - 1])
            print("stale refused: \((answer?["why"] as? String) == "stale")")
        case "connected":
            // `connected <seconds> <idle seconds>`: the switching rule for one connection.
            // Istesso's source while the viewer is connected; as soon as this Mac's own keyboard
            // types, the original source back and the strip; Istesso's again only after the local
            // keyboard has been idle <idle seconds>. Inline only after "active" names TextEdit.
            guard words.count == 3, let seconds = Double(words[1]), let idle = Double(words[2]) else { print("connected: bad arguments"); return }
            let began = Date()
            var ours = currentSourceID() == Wire.inputMethodIdentifier
            var inline = false
            func say(_ text: String) { print(String(format: "%7.2f s ", Date().timeIntervalSince(began)) + text) }
            say("connected: start, Istesso's selected: \(ours), local idle \(String(format: "%.1f", localKeyboardIdle())) s")
            while Date().timeIntervalSince(began) < seconds {
                let localIdle = localKeyboardIdle()
                if localIdle < idle {
                    if ours {
                        restoreOriginal("local typing")
                        ours = false
                        inline = false
                        say("local typing: original source, strip")
                    }
                } else if !ours {
                    ours = switchToIstesso()
                    say("local idle \(String(format: "%.1f", localIdle)) s: Istesso's selected: \(ours); strip until active")
                }
                for m in read(until: nil, timeout: 0.1) {
                    if m["op"] as? String == "active" {
                        let was = inline
                        // "active" comes only about the relayed application.
                        inline = ours && (m["ok"] as? Bool) == true
                        if inline != was { say(inline ? "active in TextEdit: inline" : "active elsewhere: strip") }
                    }
                    if m["op"] as? String == "inactive", inline { inline = false; say("inactive: strip") }
                }
            }
            restoreOriginal("disconnect")
        case "drop":
            // The host goes away during a composition, and the input method comes back.
            close(fd)
            print("drop: closed; waiting for the input method to connect again")
            guard let again = acceptInputMethod(listener, timeout: 5) else { print("drop: not back in 5 s"); return }
            fd = again
            reader = Wire.LineReader()
            let app = ProcessInfo.processInfo.environment["ISTESSO_IME_APP"] ?? "com.apple.TextEdit"
            print("hello again: \((ask(["op": "hello", "app": app])?["ok"] as? Bool) == true)")
            _ = read(until: nil, timeout: 1.0)
        default:
            guard var message = command(line) else { print("not a command"); return }
            let expected = message.removeValue(forKey: "expect") as? String
            let op = message["op"] as! String
            if op == "mark" || op == "commit" { message["comp"] = comp }
            if op == "commit" || op == "clear" { comp += 1 }
            print("send: \(op) (\((message["text"] as? String)?.utf8.count ?? 0) bytes)")
            let answer = ask(message)
            if op == "commit", let answer, answer["ok"] as? Bool == false, answer["why"] as? String != "stale",
               let text = message["text"] as? String, let pid = textEditPid() {
                // Never dropped: a refused commit goes by today's text path (a key event carrying
                // the text, as the Mac host's strip does).
                postText(pid, text)
                print(mayPost() ? "fallback: text path" : "fallback: text path not reachable here (no permission to post events)")
                print("fallback reason (\(answer["why"] as? String ?? "?"))")
            }
            if op == "mark", let answer, answer["ok"] as? Bool == false {
                print("fallback: strip for this composition (\(answer["why"] as? String ?? "?"))")
            }
            if let expected { print("match: \((answer?["sha256"] as? String) == expected)") }
        }
    }
}

func textEditPid() -> pid_t? {
    let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit")
    return running.count == 1 ? running[0].processIdentifier : nil
}

/// Posting events to another process without the permission makes macOS ask the owner for
/// Accessibility (seen on the host, 2026-11-19). The probe must never ask: it posts only when the
/// non-prompting preflight says it may, and otherwise the test says "not reachable".
func mayPost() -> Bool { CGPreflightPostEventAccess() }

func postKey(_ pid: pid_t, keyCode: CGKeyCode) {
    guard mayPost() else { return }
    let source = CGEventSource(stateID: .privateState)
    for down in [true, false] {
        CGEvent(keyboardEventSource: source, virtualKey: keyCode, keyDown: down)?.postToPid(pid)
    }
}

func postText(_ pid: pid_t, _ text: String) {
    guard mayPost() else { return }
    let source = CGEventSource(stateID: .privateState)
    let units = Array(text.utf16)
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
        if down { event?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units) }
        event?.postToPid(pid)
    }
}

func postClick(_ pid: pid_t, at point: CGPoint) {
    guard mayPost() else { return }
    let source = CGEventSource(stateID: .privateState)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.postToPid(pid)
    }
}

func serve(script: String?, selectFirst: Bool = false) {
    guard let path = Wire.defaultSocketPath() else { fail("no temporary folder") }
    let listener = listen(at: path)
    defer { close(listener); unlink(path) }
    // A source written down and not put back means the last run ended without restoring.
    if let file = previousSourceFile(), access(file, F_OK) == 0 { restoreOriginal("start after a crash") }
    if selectFirst {
        // The input source's process starts only when its source is selected.
        print("select before waiting: system says current is Istesso's: \(switchToIstesso())")
    }
    print("waiting for Istesso Dev Input (it connects once a second while it runs)")
    guard let fd = acceptInputMethod(listener, timeout: 60) else { fail("Istesso Dev Input did not connect in 60 s") }
    let session = Session(listener: listener, fd: fd)
    _ = session.read(until: nil, timeout: 0.5)
    // The relayed application first: the input source acts only while its field is active.
    let app = ProcessInfo.processInfo.environment["ISTESSO_IME_APP"] ?? "com.apple.TextEdit"
    print("hello: \((session.ask(["op": "hello", "app": app])?["ok"] as? Bool) == true)")
    _ = session.read(until: nil, timeout: 1.0)
    let lines: [String]
    if let script {
        guard let text = try? String(contentsOfFile: script, encoding: .utf8) else { fail("script not read") }
        lines = text.components(separatedBy: "\n")
    } else {
        lines = AnyIterator { readLine() }.map { $0 }
    }
    for line in lines where !line.isEmpty && !line.hasPrefix("#") {
        session.run(line)
    }
    close(session.fd)
    restoreOriginal("end")
}

// MARK: - Self-test

func selfTest(_ binary: String, standin: String? = nil) {
    var failures = 0
    func check(_ ok: Bool, _ what: String) {
        print("\(ok ? "ok  " : "FAIL") \(what)")
        if !ok { failures += 1 }
    }
    check(Wire.utf16Offset("にほん", utf8: 3) == 1, "UTF-8 offset 3 is UTF-16 offset 1 in kana")
    check(Wire.utf16Offset("にほん", utf8: 4) == nil, "an offset inside a character is refused")
    check(Wire.utf16Offset("a😀b", utf8: 5) == 3, "an emoji counts two UTF-16 units")
    check(!Wire.acceptable("a\u{7}", allowEmpty: false), "control characters are refused")
    check(!Wire.acceptable(String(repeating: "あ", count: 300), allowEmpty: false), "too long is refused")
    check(Wire.acceptable("👨\u{200D}👩\u{200D}👧 🇯🇵", allowEmpty: false), "joined emoji and flags pass (only Cc is refused)")
    check(!Wire.acceptable("a\u{0}b", allowEmpty: false), "NUL is refused")
    check(Wire.acceptable(String(repeating: "a", count: 256), allowEmpty: false) && !Wire.acceptable(String(repeating: "a", count: 257), allowEmpty: false),
          "the limit is 256 scalars, as the agent counts")
    check(Wire.ownLeafHash() != nil, "this program is signed with a certificate")

    guard let base = Wire.userTemporaryFolder() else { fail("no temporary folder") }
    let folder = (base as NSString).appendingPathComponent("istesso-dev-input-test-\(getpid())")
    let path = (folder as NSString).appendingPathComponent("socket")
    let listener = listen(at: path)
    defer { close(listener); unlink(path); rmdir(folder) }

    // A properly signed stranger: this program, signed with the same certificate, but not with the
    // input method's identifier, so the real requirement refuses it.
    if let stranger = Wire.connect(path) {
        check(acceptInputMethod(listener, timeout: 1) == nil, "a peer with the same certificate and another identifier is refused")
        close(stranger)
    } else { check(false, "connect to own socket") }

    // The input method's identifier and certificate, but running without the hardened runtime.
    if let standin {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: standin)
        process.arguments = ["--wire-test", path]
        process.standardOutput = FileHandle.nullDevice
        if (try? process.run()) != nil {
            check(acceptInputMethod(listener, timeout: 3) == nil, "the input method's identifier without the hardened runtime is refused")
            process.waitUntilExit()
        } else { check(false, "stand-in started") }
    } else { print("skip: no stand-in given (the hardened-runtime refusal is not tested)") }

    // A socket that is not private: the input method does not connect.
    chmod(path, 0o666)
    let open = Process()
    open.executableURL = URL(fileURLWithPath: binary)
    open.arguments = ["--wire-test", path]
    try? open.run()
    open.waitUntilExit()
    check(open.terminationStatus == 3, "the input method does not connect to a socket others can open")
    chmod(path, 0o600)

    let child = Process()
    child.executableURL = URL(fileURLWithPath: binary)
    child.arguments = ["--wire-test", path]
    do { try child.run() } catch { fail("input method binary not started") }
    guard let fd = acceptInputMethod(listener, timeout: 5) else {
        check(false, "the input method connects and is accepted")
        close(listener); unlink(path); rmdir(folder)
        exit(1)
    }
    check(true, "the input method connects and is accepted")
    let reader = Wire.LineReader()
    func ask(_ message: [String: Any], _ op: String) -> [String: Any] {
        _ = Wire.writeAll(fd, Wire.encode(message)!)
        return answers(fd, reader, until: op, timeout: 2).last ?? [:]
    }
    let mark = ask(["op": "mark", "text": "にほんご", "sel": [3, 9], "comp": 1], "mark")
    check(mark["ok"] as? Bool == true && (mark["sel16"] as? [Int]) == [1, 2], "a mark's selection is turned into UTF-16")
    let bad = ask(["op": "mark", "text": "にほんご", "sel": [1, 3], "comp": 1], "mark")
    check(bad["ok"] as? Bool == false, "a selection inside a character is refused")
    let empty = ask(["op": "commit", "text": "", "comp": 1], "commit")
    check(empty["ok"] as? Bool == false, "an empty commit is refused")
    _ = Wire.writeAll(fd, Wire.encode(["op": "bye"])!)
    child.waitUntilExit()
    check(child.terminationStatus == 0, "the input method checked this program and ended cleanly")
    close(fd)
    print(failures == 0 ? "self-test: passed" : "self-test: \(failures) failed")
    // exit() runs no defer: the socket and its folder are removed here.
    close(listener); unlink(path); rmdir(folder)
    exit(failures == 0 ? 0 : 1)
}

// Line by line, so that a log read while a run goes on is current.
setlinebuf(stdout)
let arguments = CommandLine.arguments
switch arguments.dropFirst().first {
case "status": status()
case "select": select()
case "register": register()
case "restore": restoreCommand()
case "secure": secure()
case "serve": serve(script: arguments.count >= 3 ? arguments[2] : nil, selectFirst: arguments.count == 4 && arguments[3] == "--select")
case "self-test" where arguments.count >= 3: selfTest(arguments[2], standin: arguments.count == 4 ? arguments[3] : nil)
default: fail("usage: status | register | serve [script] | select | restore | secure | self-test <input method binary> [stand-in]")
}
