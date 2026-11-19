// The daily host's opt-in local indicator. All AppKit work runs on the main thread.
import AppKit

struct MenuSession: Codable, Equatable {
    let started: TimeInterval
    let app: String
    let devices: [String]
    var launcherSlot = false

    static func read(_ data: Data, started: TimeInterval) -> MenuSession? {
        guard let session = try? JSONDecoder().decode(Self.self, from: data),
              session.started == started else { return nil }
        return session
    }

    /// The most rows of viewers the menu shows; the rest are counted in one row.
    static let mostRows = 12

    static func lines(_ sessions: [Self]) -> [String] {
        let all = sessions.flatMap { session in
            session.devices.map { "いま映しています：" + label($0) + " — " + label(session.app) }
        }.sorted()
        guard all.count > mostRows else { return all }
        return Array(all.prefix(mostRows)) + ["ほか \(all.count - mostRows) 件"]
    }

    /// A name as the menu shows it: no control, format (bidi, zero-width), separator, private or
    /// unassigned characters; white space as single spaces; at most 40 characters.
    static func label(_ text: String) -> String {
        var kept = String.UnicodeScalarView()
        var gap = false
        for scalar in text.unicodeScalars {
            switch scalar.properties.generalCategory {
            case .control, .spaceSeparator, .lineSeparator, .paragraphSeparator:
                gap = !kept.isEmpty
            case .format, .privateUse, .surrogate, .unassigned:
                continue
            default:
                if kept.count >= 40 { return String(kept) + "…" }
                if gap { kept.append(" "); gap = false }
                kept.append(scalar)
            }
        }
        return kept.isEmpty ? "?" : String(kept)
    }

    // Only live, registered hosts take part. The PID breaks simultaneous starts.
    static func owner(_ sessions: [pid_t: Self]) -> pid_t? {
        sessions.min { ($0.value.started, $0.key) < ($1.value.started, $1.key) }?.key
    }
}

final class HostMenuBar: NSObject {
    private let folder: URL
    private let started: TimeInterval
    private let launcherSlot: Bool
    private let bundleID: String
    private let lockFD: Int32
    private var locked = false
    private var lastSession: MenuSession?
    private var item: NSStatusItem?
    private var previousLines: [String] = []

    init?(_ bundle: Bundle, pidFile: String?) {
        guard bundle.bundleURL.pathExtension == "app", let id = bundle.bundleIdentifier,
              let date = NSRunningApplication.current.launchDate, let base = Wire.userTemporaryFolder() else { return nil }
        let root = bundle.bundleURL.deletingLastPathComponent().appendingPathComponent("run", isDirectory: true)
        let receipt = pidFile.map { URL(fileURLWithPath: $0).standardizedFileURL }
        launcherSlot = receipt?.lastPathComponent == "host.number" &&
            receipt?.deletingLastPathComponent().deletingLastPathComponent().path == root.standardizedFileURL.path
        bundleID = id
        started = date.timeIntervalSinceReferenceDate
        folder = URL(fileURLWithPath: base).appendingPathComponent("istesso-daily-menu")
        guard mkdir(folder.path, 0o700) == 0 || errno == EEXIST else { return nil }
        var info = stat()
        guard lstat(folder.path, &info) == 0, info.st_mode & S_IFMT == S_IFDIR,
              info.st_uid == getuid(), info.st_mode & 0o077 == 0 else { return nil }
        lockFD = open(folder.appendingPathComponent("owner.lock").path, O_RDWR | O_CREAT | O_CLOEXEC | O_NOFOLLOW, 0o600)
        guard lockFD >= 0 else { return nil }
        super.init()
    }

    private func file(_ pid: pid_t) -> URL { folder.appendingPathComponent("\(pid).json") }

    // Look up live instances first. A file left by a crash or a reused PID cannot add a row.
    private func liveSessions() -> [(NSRunningApplication, MenuSession)] {
        var sessions: [(NSRunningApplication, MenuSession)] = []
        for host in NSRunningApplication.runningApplications(withBundleIdentifier: bundleID) {
            let pid = host.processIdentifier
            guard let date = host.launchDate, let data = try? Data(contentsOf: file(pid)),
                  let session = MenuSession.read(data, started: date.timeIntervalSinceReferenceDate) else { continue }
            sessions.append((host, session))
        }
        return sessions
    }

    func tick(app: String, devices: [String]) {
        let session = MenuSession(started: started, app: app, devices: devices.sorted(), launcherSlot: launcherSlot)
        if session != lastSession {
            do {
                try JSONEncoder().encode(session).write(to: file(getpid()), options: .atomic)
                lastSession = session
            } catch {
                note("cannot publish the menu state")
                return
            }
        }
        let sessions = Dictionary(uniqueKeysWithValues: liveSessions().map { ($0.0.processIdentifier, $0.1) })
        guard MenuSession.owner(sessions) == getpid() else {
            relinquish()
            return
        }
        // Snapshots in two processes may differ during a launch. Only one can display the item.
        if !locked { locked = flock(lockFD, LOCK_EX | LOCK_NB) == 0 }
        guard locked else { return }
        let lines = MenuSession.lines(Array(sessions.values))
        guard !lines.isEmpty else { removeItem(); return }
        if item != nil && lines == previousLines { return }
        let menu = NSMenu()
        menu.autoenablesItems = false
        for line in lines {
            menu.addItem(withTitle: line, action: nil, keyEquivalent: "").isEnabled = false
        }
        menu.addItem(.separator())
        menu.addItem(withTitle: "止める", action: #selector(stopAll), keyEquivalent: "").target = self
        if item == nil { item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength) }
        item?.button?.title = "映しています"
        item?.menu = menu
        previousLines = lines
    }

    @objc private func stopAll() {
        // The launcher keeps all slot and agent PID receipts beside the installed bundle.
        // An independent helper finishes the sweep even after this menu's owner exits.
        guard let script = Bundle.main.url(forResource: "stop-slots", withExtension: "sh") else {
            return note("cannot find the daily host stop helper")
        }
        let live = liveSessions()
        let bundles = Set(live.compactMap { $0.1.launcherSlot ? $0.0.bundleURL : nil }).union([Bundle.main.bundleURL])
        let helper = Process()
        helper.executableURL = URL(fileURLWithPath: "/bin/bash")
        helper.arguments = [script.path] + bundles.map { $0.deletingLastPathComponent().appendingPathComponent("run").path }
        do { try helper.run() } catch { return note("cannot start the daily host stop helper") }

        // Also cover an opted-in bundle launched without the script's PID receipt.
        for (host, session) in live where !session.launcherSlot {
            if !host.terminate() { note("could not request termination of a standalone daily host") }
        }
    }

    private func removeItem() {
        if let item { NSStatusBar.system.removeStatusItem(item) }
        item = nil
        previousLines = []
    }

    private func relinquish() {
        removeItem()
        if locked { flock(lockFD, LOCK_UN); locked = false }
    }

    deinit {
        relinquish()
        try? FileManager.default.removeItem(at: file(getpid()))
        close(lockFD)
    }
}

func menuBarSelfTest(_ check: (Bool, Bool, String) -> Void) {
    let first = MenuSession(started: 1, app: "Example App", devices: ["Example Tablet"]) // check-private: ok synthetic display labels
    let second = MenuSession(started: 2, app: "Example Editor", devices: ["Example Screen", "Example Pad"]) // check-private: ok synthetic display labels
    let idle = MenuSession(started: 2, app: first.app, devices: [])
    let bytes = (try? JSONEncoder().encode(first)) ?? Data()
    check(MenuSession.read(bytes, started: 1) == first, true, "menu: state round trip")
    check(MenuSession.read(bytes, started: 2) == nil, true, "menu: a reused PID is ignored")
    check(MenuSession.read(Data("{}".utf8), started: 1) == nil, true, "menu: incomplete state is ignored")
    check(MenuSession.lines([idle]).isEmpty, true, "menu: idle slots have no rows")
    let lines = [
        "いま映しています：Example Pad — Example Editor", // check-private: ok synthetic display labels
        "いま映しています：Example Screen — Example Editor", // check-private: ok synthetic display labels
        "いま映しています：Example Tablet — Example App", // check-private: ok synthetic display labels
    ]
    check(MenuSession.lines([first, second, idle]) == lines, true, "menu: one sorted row per viewer and app")
    check(MenuSession.owner([20: first, 10: second]) == 20, true, "menu: first live instance owns the item")
    check(MenuSession.owner([10: second, 30: idle]) == 10, true, "menu: deterministic handover after owner exit")
    check(MenuSession.owner([:]) == nil, true, "menu: no owner without hosts")
    check(MenuSession.label("A\u{202E}b\u{2066}c\u{200B}d") == "Abcd", true, "menu: bidi and zero-width characters are dropped")
    check(MenuSession.label(" Ex\u{0}\n\tample\u{2028} ") == "Ex ample", true, "menu: control characters and line breaks become one space")
    check(MenuSession.label("\u{202E}\u{7}") == "?", true, "menu: a name of nothing shown is a question mark")
    check(MenuSession.label(String(repeating: "x", count: 50)) == String(repeating: "x", count: 40) + "…", true, "menu: long names are cut")
    let many = MenuSession(started: 3, app: "Example App", devices: (0..<20).map { "Device \($0)" }) // check-private: ok synthetic display labels
    check(MenuSession.lines([many]).count == MenuSession.mostRows + 1, true, "menu: many viewers end in one counting row")
}
