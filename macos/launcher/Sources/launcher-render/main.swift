import AppKit
import LauncherCore
import LauncherUI

private struct PictureRunner: LauncherRunner {
    let apps: AppList
    let projects: [ProjectRow]

    func listApps() async -> Result<AppList, LauncherFailure> { .success(apps) }
    func listProjects() async -> Result<[ProjectRow], LauncherFailure> { .success(projects) }
    func open(appID: String) async -> Int32 { 127 }
}

private struct PictureClipboard: Clipboard {
    func copy(_ text: String) {}
}

func stop(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

let usage = "usage: launcher-render --out FILE.png [--dark] [--query TEXT] [--status TEXT]"
var arguments = CommandLine.arguments.dropFirst().makeIterator()
var options: [String: String] = [:]
var dark = false
while let flag = arguments.next() {
    if flag == "--dark" && !dark {
        dark = true
    } else {
        guard ["--out", "--query", "--status"].contains(flag), options[flag] == nil,
              let value = arguments.next() else { stop(usage, code: 2) }
        options[flag] = value
    }
}
guard let output = options["--out"], !output.isEmpty else { stop(usage, code: 2) }

do {
    let runner = try PictureRunner(apps: LauncherFixtures.apps(), projects: LauncherFixtures.projects())
    let model = LauncherModel(runner: runner, clipboard: PictureClipboard())
    await model.start().value
    model.query = options["--query"] ?? ""
    if let status = options["--status"] { model.status = status }
    let controller = LauncherViewController(model: model)
    let view = controller.view
    view.frame = NSRect(x: 0, y: 0, width: 560, height: 420)
    guard let appearance = NSAppearance(named: dark ? .darkAqua : .aqua) else {
        stop("Could not load the appearance.", code: 1)
    }
    view.appearance = appearance
    var png: Data?
    appearance.performAsCurrentDrawingAppearance {
        controller.layoutForRendering()
        if let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: bitmap)
            png = bitmap.representation(using: .png, properties: [:])
        }
    }
    guard let png else { stop("Could not draw the launcher.", code: 1) }
    try png.write(to: URL(fileURLWithPath: output), options: .atomic)
} catch {
    stop("Could not render the launcher image.", code: 1)
}
