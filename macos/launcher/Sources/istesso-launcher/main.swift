import AppKit
import LauncherCore
import LauncherUI

struct PasteboardClipboard: Clipboard {
    func copy(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

func usage() -> Never {
    FileHandle.standardError.write(Data("usage: istesso-launcher --istesso PATH --trial DIR\n".utf8))
    exit(2)
}

var arguments = CommandLine.arguments.dropFirst().makeIterator()
var options: [String: String] = [:]
while let flag = arguments.next() {
    guard ["--istesso", "--trial"].contains(flag), options[flag] == nil,
          let value = arguments.next(), !value.isEmpty, !value.hasPrefix("--") else { usage() }
    options[flag] = value
}
guard let path = options["--istesso"], let trial = options["--trial"] else { usage() }

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)

@MainActor
func setUpLauncher(app: NSApplication, path: String, trial: String) -> NSWindow {
    let menu = NSMenu()
    let appItem = NSMenuItem()
    menu.addItem(appItem)
    let commands = NSMenu(title: "Istesso Launcher")
    commands.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
    commands.addItem(withTitle: "Quit Istesso Launcher", action: #selector(NSApplication.terminate(_:)),
                     keyEquivalent: "q").target = app
    appItem.submenu = commands
    app.mainMenu = menu

    let model = LauncherModel(runner: ProcessRunner(istesso: path, trial: trial), clipboard: PasteboardClipboard())
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 420),
                          styleMask: [.titled, .closable, .resizable, .miniaturizable],
                          backing: .buffered, defer: false)
    window.title = "Istesso Launcher"
    window.contentViewController = LauncherViewController(model: model)
    window.isReleasedWhenClosed = false
    window.center()
    window.makeKeyAndOrderFront(nil)
    app.activate()
    model.start()
    return window
}

let window = MainActor.assumeIsolated { setUpLauncher(app: app, path: path, trial: trial) }
app.run()
