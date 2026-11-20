import Foundation
import LauncherCore

// Each answer can arrive before or after the model asks for it.
private struct Answers<Value> {
    let stream: AsyncStream<Value>
    let sender: AsyncStream<Value>.Continuation

    init() {
        (stream, sender) = AsyncStream.makeStream(of: Value.self)
    }

    func next() async -> Value {
        var iterator = stream.makeAsyncIterator()
        guard let value = await iterator.next() else { preconditionFailure("Missing fake answer") }
        return value
    }
}

private actor FakeRunner: LauncherRunner {
    nonisolated let apps = Answers<Result<AppList, LauncherFailure>>()
    nonisolated let projects = Answers<Result<[ProjectRow], LauncherFailure>>()
    nonisolated let exits = Answers<Int32>()
    private(set) var openCalls: [String] = []
    private(set) var listCalls = 0

    func listApps() async -> Result<AppList, LauncherFailure> {
        listCalls += 1
        return await apps.next()
    }

    func listProjects() async -> Result<[ProjectRow], LauncherFailure> {
        listCalls += 1
        return await projects.next()
    }

    func open(appID: String) async -> Int32 {
        openCalls.append(appID)
        return await exits.next()
    }
}

private final class FakeClipboard: Clipboard {
    var copies: [String] = []
    func copy(_ text: String) { copies.append(text) }
}

private func project(name: String, machine: String, kinds: [String]) throws -> ProjectRow {
    let json: [String: Any] = [
        "id": "00000000000000000000000000000005", "name": name, "development_machine": machine,
        "reachable": true, "clash": true, "kinds": kinds, "capabilities": ["gpu": false, "tools": []],
    ]
    let data = try JSONSerialization.data(withJSONObject: json)
    return try JSONDecoder().decode(ProjectRow.self, from: data)
}

@MainActor
private final class Checks {
    var passed = 0
    var failures: [String] = []

    func check(_ condition: Bool, _ description: String) {
        if condition { passed += 1 } else { failures.append(description) }
    }

    func eventually(_ description: String, _ condition: () async -> Bool) async {
        let clock = ContinuousClock()
        let deadline = clock.now + .seconds(2)
        while clock.now < deadline {
            if await condition() {
                check(true, description)
                return
            }
            try? await Task.sleep(for: .milliseconds(1))
        }
        check(false, description)
    }

    func run() async throws {
        let apps = try LauncherFixtures.apps()
        let projects = try LauncherFixtures.projects()
        let appEntries = apps.apps.map(Entry.init(app:))
        let projectEntries = projects.map(Entry.init(project:))
        check(apps.apps.count == 7 && apps.problems.isEmpty, "decode the app document with an extra key")
        check(projects.count == 5 && projects[0].capabilities.gpu &&
              projects[0].capabilities.tools == ["cargo"], "decode the project array with an extra key")
        check(appEntries[0].id == "ubuntu/firefox" && appEntries[0].section == .apps &&
              appEntries[0].action == .open("ubuntu/firefox") && appEntries[0].mark == nil, "app identity and action")
        check(appEntries.map(\.detail) == Array(repeating: "on Ubuntu", count: 4) +
              ["on Studio", "on this Mac", "on this Mac"], "app detail by host") // check-private: ok (invented machine labels)
        check(projectEntries[0].id == "project/\(projects[0].id)@Ubuntu" &&
              projectEntries[0].section == .projects, "project identity") // check-private: ok (invented machine label)
        check(projectEntries[3].id != projectEntries[4].id, "separate same-id projects by machine")
        check(projectEntries.map(\.mark) == [nil, "!", nil, "?", "!?"], "all reachability and clash marks")
        check(projectEntries.map(\.detail) == ["on Ubuntu · rust", "on Ubuntu · blender", "on Studio · apple",
              "on Ubuntu", "on Example A"], "project kinds and unknown alone") // check-private: ok (invented machine labels)
        check(projectEntries.map(\.action) == [.copy("ish alpha"), .copy("ish scene"), .copy("ish notes-app"),
              .copy("ish Ubuntu:shared"), .copy("ish 'Example A:shared'")], "plain and qualified commands") // check-private: ok (invented machine labels)

        let quoted = try project(name: "shared", machine: "Example A's", kinds: ["node", "future"]) // check-private: ok (invented machine label)
        let escaped = Entry(project: quoted)
        check(escaped.action == .copy("ish 'Example A'\\''s:shared'"), "escape an embedded single quote") // check-private: ok (invented machine label)
        check(escaped.detail == "on Example A's · node, future", "multiple kinds including a future kind") // check-private: ok (invented machine label)
        let invalid = try project(name: "two words", machine: "Studio", kinds: []) // check-private: ok (invented machine label)
        let disabled = Entry(project: invalid)
        check(disabled.action == .none && disabled.detail == "on Studio · cannot be opened by name", "invalid name detail") // check-private: ok (invented machine label)
        for name in ["", "-option", "a\n", "café", String(repeating: "a", count: 65)] {
            let row = try project(name: name, machine: "Studio", kinds: []) // check-private: ok (invented machine label)
            check(Entry(project: row).action == .none, "reject invalid name of length \(name.utf8.count)")
        }
        let longest = "0._-" + String(repeating: "a", count: 60)
        let valid = try project(name: longest, machine: "Example.1_A-B", kinds: []) // check-private: ok (invented machine label)
        check(Entry(project: valid).action == .copy("ish Example.1_A-B:" + longest), "valid boundary and punctuation") // check-private: ok (invented machine label)

        let runner = FakeRunner()
        let clipboard = FakeClipboard()
        let model = LauncherModel(runner: runner, clipboard: clipboard)
        let loading: [Row] = [.header("Apps"), .note("Looking for apps…"),
                              .header("Projects"), .note("Looking for projects…")]
        check(model.rows == loading && model.selectedRow == nil, "both sections initially loading")
        check(model.status == "Type to search. Return opens.", "initial status")

        var changes: [[Row]] = []
        model.onChange = { changes.append(model.rows) }
        let load = model.start()
        await eventually("list requests are concurrent") { await runner.listCalls == 2 }
        runner.projects.sender.yield(.success(projects))
        let projectRows = projectEntries.map(Row.entry)
        let partial = Array(loading.prefix(3)) + projectRows
        await eventually("publish projects while apps are pending") { changes.contains(partial) }
        runner.apps.sender.yield(.success(apps))
        await load.value
        check(model.rows == [.header("Apps")] + appEntries.map(Row.entry) + [.header("Projects")] + projectRows,
              "loaded sections in source order")
        check(model.selectedRow == 1, "select first loaded entry")
        model.onChange = nil

        model.query = "  ÉDITOR\tubUNtu\n"
        check(entries(model).map(\.title) == ["Text Editor"], "match all words across title and machine")
        model.query = "editor studio" // check-private: ok (invented machine label)
        check(model.selectedRow == nil && model.rows == [.header("Apps"), .note("No matches"),
              .header("Projects"), .note("No matches")], "no matches in either section")
        model.moveSelection(1)
        model.activateSelected()
        check(model.selectedRow == nil && model.status == "Type to search. Return opens.", "empty selection is inert")
        model.query = "s"
        check(entries(model).filter { $0.section == .projects }.map(\.id) ==
              [1, 3, 4, 2].map { projectEntries[$0].id }, "stable title-prefix ordering")

        model.query = " "
        model.moveSelection(Int.min)
        check(model.selectedRow == 1, "clamp at first entry")
        model.moveSelection(Int.max)
        check(model.selectedRow == model.rows.count - 1, "clamp at last entry")
        model.select(row: 7)
        model.moveSelection(1)
        check(model.selectedRow == 9, "skip section header moving down")
        model.moveSelection(-1)
        check(model.selectedRow == 7, "skip section header moving up")
        model.select(row: 8)
        model.select(row: -1)
        check(model.selectedRow == 7, "ignore header and invalid selections")
        model.query = "fire"
        check(model.selectedRow == 1 && entries(model).first?.id == "ubuntu/firefox", "query resets selection")

        var expectedCalls: [String] = []
        let outcomes: [(Int32, String)] = [
            (0, "Opened Firefox on Ubuntu."), // check-private: ok (invented machine label)
            (3, "A latency measurement is running. Try again when it is over."),
            (1, "Could not open Firefox on Ubuntu (exit 1)."), // check-private: ok (invented machine label)
        ]
        for (code, status) in outcomes {
            model.activateSelected()
            model.activateSelected()
            check(model.status == "Opening Firefox on Ubuntu…", "opening status before exit \(code)") // check-private: ok (invented machine label)
            expectedCalls.append("ubuntu/firefox")
            await eventually("one open in flight for exit \(code)") { await runner.openCalls == expectedCalls }
            runner.exits.sender.yield(code)
            await eventually("completion status for exit \(code)") { model.status == status }
        }
        check(await runner.openCalls == expectedCalls, "exactly one call per completed activation")
        model.query = "alpha"
        model.activateSelected()
        check(clipboard.copies == ["ish alpha"] && model.status == "Copied “ish alpha”. Paste it in a terminal.",
              "copy a reachable project")
        model.query = "scene"
        model.activateSelected()
        check(clipboard.copies == ["ish alpha", "ish scene"] &&
              model.status == "Copied “ish scene”. Paste it in a terminal. Ubuntu is not answering.", "copy an offline project") // check-private: ok (invented machine label)

        try await checkSectionStates(app: apps.apps[0], invalid: invalid)
        do {
            try await checkProcessRunner()
        } catch {
            check(false, "create and read the process runner fixture")
        }
    }

    func entries(_ model: LauncherModel) -> [Entry] {
        model.rows.compactMap { if case .entry(let entry) = $0 { return entry }; return nil }
    }

    func checkProcessRunner() async throws {
        let files = FileManager.default
        let dir = files.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try files.createDirectory(at: dir, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: dir) }

        let fake = dir.appendingPathComponent("fake-istesso")
        let script = #"""
        #!/bin/sh
        {
            echo call
            for a in "$@"; do printf 'arg=%s\n' "$a"; done
            echo "DAILY_OWNER=${DAILY_OWNER-unset}"
            echo "DAILY_OPEN_HANDLER=${DAILY_OPEN_HANDLER-unset}"
            echo "LAUNCHER_CHECK_KEEP=${LAUNCHER_CHECK_KEEP-unset}"
            echo end
        } >> "$(dirname "$0")/record"
        case "$#:$1:$2" in
            5:launcher:apps)
                [ "$3" = --json ] && [ "$4" = --trial ] || exit 9
                case "$5" in
                    */slow) exec /bin/sleep 30 ;;
                    */fail) exit 4 ;;
                esac
                echo '{"apps":[{"id":"local/com.example.fake","name":"Fake","machine":"This Mac","host":"local"}],"problems":[]}' # check-private: ok (invented app and machine label)
                ;;
            2:projects:--json) echo '[]' ;;
            5:launcher:open)
                [ "$4" = --trial ] || exit 9
                case "$3" in
                    */exit-3) exit 3 ;;
                    */exit-1) exit 1 ;;
                esac
                ;;
            *) exit 9 ;;
        esac
        exit 0
        """#
        try script.write(to: fake, atomically: true, encoding: .utf8)
        try files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: fake.path)
        setenv("DAILY_OWNER", "1", 1)
        setenv("DAILY_OPEN_HANDLER", "/opt/fixture-one/handler", 1)
        setenv("LAUNCHER_CHECK_KEEP", "kept", 1)
        defer {
            unsetenv("DAILY_OWNER")
            unsetenv("DAILY_OPEN_HANDLER")
            unsetenv("LAUNCHER_CHECK_KEEP")
        }

        let trial = dir.appendingPathComponent("trial").path
        let runner = ProcessRunner(istesso: fake.path, trial: trial)
        let apps = await runner.listApps()
        check((try? apps.get().apps.map(\.id)) == ["local/com.example.fake"], "process decodes the app ID")
        let projects = await runner.listProjects()
        check((try? projects.get().isEmpty) == true, "process decodes an empty project list")
        let literalID = "local/Fake App;$(x)"
        check(await runner.open(appID: literalID) == 0, "open returns exit 0")
        check(await runner.open(appID: "local/exit-1") == 1, "open returns exit 1")
        check(await runner.open(appID: "local/exit-3") == 3, "open returns exit 3")

        func failureMessage(_ result: Result<AppList, LauncherFailure>) -> String? {
            if case .failure(let error) = result { return error.message }
            return nil
        }
        let fail = dir.appendingPathComponent("fail").path
        let failing = ProcessRunner(istesso: fake.path, trial: fail)
        check(failureMessage(await failing.listApps()) == "could not read the app list", "report a nonzero list exit")
        let missing = ProcessRunner(istesso: dir.appendingPathComponent("missing").path, trial: trial)
        check(await missing.open(appID: literalID) == 127, "missing executable returns exit 127")
        check(failureMessage(await missing.listApps()) != nil, "missing executable fails the app list")

        let slow = dir.appendingPathComponent("slow").path
        let timed = ProcessRunner(istesso: fake.path, trial: slow, appSeconds: 1)
        let clock = ContinuousClock()
        let start = clock.now
        let result = await timed.listApps()
        let elapsed = start.duration(to: clock.now)
        check(failureMessage(result) == "did not answer in time", "report the app list timeout")
        check(elapsed >= .milliseconds(900) && elapsed < .seconds(5), "honor the one-second app list deadline")

        let record = try String(contentsOf: dir.appendingPathComponent("record"), encoding: .utf8)
        let calls = record.split(separator: "\n").map(String.init).split(separator: "end")
        let arguments = [
            ["launcher", "apps", "--json", "--trial", trial],
            ["projects", "--json"],
            ["launcher", "open", literalID, "--trial", trial],
            ["launcher", "open", "local/exit-1", "--trial", trial],
            ["launcher", "open", "local/exit-3", "--trial", trial],
            ["launcher", "apps", "--json", "--trial", fail],
            ["launcher", "apps", "--json", "--trial", slow],
        ]
        let expectedCalls = arguments.map { ["call"] + $0.map { "arg=" + $0 } }
        check(calls.map { Array($0.dropLast(3)) } == expectedCalls, "pass each argument literally and in order")
        let environment = ["DAILY_OWNER=unset", "DAILY_OPEN_HANDLER=unset", "LAUNCHER_CHECK_KEEP=kept"]
        check(calls.allSatisfy { $0.suffix(3).elementsEqual(environment) }, "strip both owner variables and keep the rest")
    }

    func checkSectionStates(app: AppRow, invalid: ProjectRow) async throws {
        let clipboard = FakeClipboard()
        let failure = LauncherFailure("did not answer in time")
        let failed = FakeRunner()
        failed.apps.sender.yield(.failure(failure))
        failed.projects.sender.yield(.failure(failure))
        let failedModel = LauncherModel(runner: failed, clipboard: clipboard)
        await failedModel.start().value
        check(failedModel.rows == [.header("Apps"), .note("Could not read the app list."),
              .header("Projects"), .note("Could not read the project list.")] && failedModel.selectedRow == nil,
              "list failure notes")

        let empty = FakeRunner()
        let list = try JSONDecoder().decode(AppList.self, from: Data(#"{"apps":[],"problems":[]}"#.utf8))
        empty.apps.sender.yield(.success(list))
        empty.projects.sender.yield(.success([]))
        let emptyModel = LauncherModel(runner: empty, clipboard: clipboard)
        await emptyModel.start().value
        check(emptyModel.rows == [.header("Apps"), .header("Projects"),
              .note("No projects yet. Register one with: istesso projects add NAME FOLDER")] &&
              emptyModel.selectedRow == nil, "empty successful lists")

        let runner = FakeRunner()
        let model = LauncherModel(runner: runner, clipboard: clipboard)
        let json = #"{"apps":[{"id":"ubuntu/firefox","name":"Firefox","machine":"Ubuntu","host":"ubuntu"}],"problems":["example problem","another problem"]}"# // check-private: ok (invented machine label)
        runner.apps.sender.yield(.success(try JSONDecoder().decode(AppList.self, from: Data(json.utf8))))
        let load = model.start()
        let appRows: [Row] = [.header("Apps"), .entry(Entry(app: app)), .note("example problem"),
                              .note("another problem"), .header("Projects")]
        await eventually("publish apps and problems while projects are pending") {
            model.rows == appRows + [.note("Looking for projects…")]
        }
        runner.projects.sender.yield(.success([invalid]))
        await load.value
        model.select(row: 2)
        check(model.selectedRow == 1, "ignore a problem note")
        model.moveSelection(1)
        check(model.selectedRow == 5, "skip multiple problem notes")
        model.activateSelected()
        check(clipboard.copies.isEmpty && model.status == "This project cannot be opened by name.", "disabled action")
        model.query = "absent"
        check(model.rows == [.header("Apps"), .note("No matches"), .note("example problem"),
              .note("another problem"), .header("Projects"), .note("No matches")], "keep problems when filtering")
    }
}

private let checks = Checks()
do {
    try await checks.run()
} catch {
    checks.failures.append("Could not decode the check fixtures.")
}
if checks.failures.isEmpty {
    print("launcher-check: \(checks.passed) checks passed")
} else {
    for failure in checks.failures {
        FileHandle.standardError.write(Data(("launcher-check: " + failure + "\n").utf8))
    }
    exit(1)
}
