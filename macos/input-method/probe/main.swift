// istesso-ime-probe (com.maromoco.istesso-dev.ime-probe): the host's side of the link to Istesso Dev
// Input, for trying it before the Mac host takes it over (trial only; see build.sh).
//
//   istesso-ime-probe status              read only: is the input source installed, enabled, selected
//   istesso-ime-probe serve [script]      owns the socket, waits for the input method, runs commands
//                                         from the script (or standard input), prints its answers
//   istesso-ime-probe select              selects Istesso Dev Input (after the owner added it);
//                                         prints the source that was selected before
//   istesso-ime-probe restore <id>        selects that source again
//   istesso-ime-probe secure              is the focused element a password field (needs
//                                         Accessibility; says "unknown" without it), and is secure
//                                         event input on
//   istesso-ime-probe self-test <input method binary>
//                                         the link without InputMethodKit: peer checks both ways,
//                                         a refused stranger, a socket that is not private, the
//                                         byte-to-UTF-16 selection
//
// Commands: `mark <sel start byte> <sel end byte> <text>`, `commit <text>`, `clear`, `caret`,
// `wait <seconds>`. Text is never printed back: the answers carry lengths and rectangles only.
import AppKit
import ApplicationServices
import Carbon
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
        print("source: \(id) enabled=\(enabled) selected=\(selected) select_capable=\(selectable)")
    }
    print("current: \(currentSourceID())")
    print("secure_event_input: \(IsSecureEventInputEnabled())")
}

func select() {
    guard let source = ourSources().first(where: { property($0, kTISPropertyInputSourceIsSelectCapable) ?? false }) else {
        fail("Istesso Dev Input is not installed or cannot be selected")
    }
    guard property(source, kTISPropertyInputSourceIsEnabled) ?? false else {
        fail("Istesso Dev Input is not enabled: the owner adds it in System Settings > Keyboard > Input Sources")
    }
    let before = currentSourceID()
    let result = TISSelectInputSource(source)
    print("before: \(before)")
    print("select: \(result == noErr ? "ok" : "error \(result)")")
}

func restore(_ id: String) {
    let filter = [kTISPropertyInputSourceID as String: id] as CFDictionary
    guard let source = (TISCreateInputSourceList(filter, false)?.takeRetainedValue() as? [TISInputSource])?.first else {
        fail("no enabled source with that identifier")
    }
    let result = TISSelectInputSource(source)
    print("restore: \(result == noErr ? "ok" : "error \(result)")")
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
            if let data = try? JSONSerialization.data(withJSONObject: message, options: [.sortedKeys]) {
                print("answer: \(String(decoding: data, as: UTF8.self))")
            }
            got.append(message)
        }
        if let op, got.contains(where: { $0["op"] as? String == op }) { break }
    }
    return got
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
    default: return nil
    }
}

func serve(script: String?) {
    guard let path = Wire.defaultSocketPath() else { fail("no temporary folder") }
    let listener = listen(at: path)
    defer { close(listener); unlink(path) }
    print("waiting for Istesso Dev Input (it connects once a second while it runs)")
    guard let fd = acceptInputMethod(listener, timeout: 60) else { fail("Istesso Dev Input did not connect in 60 s") }
    let reader = Wire.LineReader()
    _ = answers(fd, reader, until: nil, timeout: 1.5)
    let lines: [String]
    if let script {
        guard let text = try? String(contentsOfFile: script, encoding: .utf8) else { fail("script not read") }
        lines = text.components(separatedBy: "\n")
    } else {
        lines = AnyIterator { readLine() }.map { $0 }
    }
    for line in lines where !line.isEmpty && !line.hasPrefix("#") {
        if line.hasPrefix("wait "), let seconds = Double(line.dropFirst(5)) {
            print("wait \(seconds)")
            _ = answers(fd, reader, until: nil, timeout: seconds)
            continue
        }
        guard let message = command(line), let data = Wire.encode(message) else { print("not a command"); continue }
        let op = message["op"] as! String
        let length = (message["text"] as? String)?.utf8.count ?? 0
        print("send: \(op) (\(length) bytes)")
        guard Wire.writeAll(fd, data) else { print("closed"); break }
        _ = answers(fd, reader, until: "caret", timeout: 2)
    }
    close(fd)
}

// MARK: - Self-test

func selfTest(_ binary: String) {
    var failures = 0
    func check(_ ok: Bool, _ what: String) {
        print("\(ok ? "ok  " : "FAIL") \(what)")
        if !ok { failures += 1 }
    }
    check(Wire.utf16Offset("にほん", utf8: 3) == 1, "UTF-8 offset 3 is UTF-16 offset 1 in kana")
    check(Wire.utf16Offset("にほん", utf8: 4) == nil, "an offset inside a character is refused")
    check(Wire.utf16Offset("a😀b", utf8: 5) == 3, "an emoji counts two UTF-16 units")
    check(!Wire.acceptable("a\u{7}", allowEmpty: false), "control characters are refused")
    check(!Wire.acceptable(String(repeating: "あ", count: 400), allowEmpty: false), "too long is refused")
    check(Wire.ownLeafHash() != nil, "this program is signed with a certificate")

    guard let base = Wire.userTemporaryFolder() else { fail("no temporary folder") }
    let folder = (base as NSString).appendingPathComponent("istesso-dev-input-test-\(getpid())")
    let path = (folder as NSString).appendingPathComponent("socket")
    let listener = listen(at: path)
    defer { close(listener); unlink(path); rmdir(folder) }

    // A stranger: this program itself is not the input method, so it is refused.
    if let stranger = Wire.connect(path) {
        check(acceptInputMethod(listener, timeout: 1) == nil, "a peer that is not the input method is refused")
        close(stranger)
    } else { check(false, "connect to own socket") }

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
    guard let fd = acceptInputMethod(listener, timeout: 5) else { check(false, "the input method connects and is accepted"); exit(1) }
    check(true, "the input method connects and is accepted")
    let reader = Wire.LineReader()
    func ask(_ message: [String: Any], _ op: String) -> [String: Any] {
        _ = Wire.writeAll(fd, Wire.encode(message)!)
        return answers(fd, reader, until: op, timeout: 2).last ?? [:]
    }
    let mark = ask(["op": "mark", "text": "にほんご", "sel": [3, 9]], "mark")
    check(mark["ok"] as? Bool == true && (mark["sel16"] as? [Int]) == [1, 2], "a mark's selection is turned into UTF-16")
    let bad = ask(["op": "mark", "text": "にほんご", "sel": [1, 3]], "mark")
    check(bad["ok"] as? Bool == false, "a selection inside a character is refused")
    let empty = ask(["op": "commit", "text": ""], "commit")
    check(empty["ok"] as? Bool == false, "an empty commit is refused")
    _ = Wire.writeAll(fd, Wire.encode(["op": "bye"])!)
    child.waitUntilExit()
    check(child.terminationStatus == 0, "the input method checked this program and ended cleanly")
    close(fd)
    print(failures == 0 ? "self-test: passed" : "self-test: \(failures) failed")
    exit(failures == 0 ? 0 : 1)
}

let arguments = CommandLine.arguments
switch arguments.dropFirst().first {
case "status": status()
case "select": select()
case "restore" where arguments.count == 3: restore(arguments[2])
case "secure": secure()
case "serve": serve(script: arguments.count == 3 ? arguments[2] : nil)
case "self-test" where arguments.count == 3: selfTest(arguments[2])
default: fail("usage: status | serve [script] | select | restore <id> | secure | self-test <input method binary>")
}
