import Foundation

@MainActor
public final class LauncherModel {
    private let runner: LauncherRunner
    private let clipboard: Clipboard
    private var apps: Result<AppList, LauncherFailure>?
    private var projects: Result<[ProjectRow], LauncherFailure>?
    private var loading: Task<Void, Never>?
    private var opening: Set<String> = []
    private var chosenID: String?

    public var onChange: (() -> Void)?
    public var query = "" {
        didSet {
            chosenID = nil
            rebuild()
        }
    }
    public private(set) var rows: [Row] = []
    public private(set) var selectedRow: Int?
    public var status = "Type to search. Return opens." {
        didSet { onChange?() }
    }

    public init(runner: LauncherRunner, clipboard: Clipboard) {
        self.runner = runner
        self.clipboard = clipboard
        rebuild()
    }

    @discardableResult
    public func start() -> Task<Void, Never> {
        if let loading { return loading }
        let task = Task {
            async let appLoad: Void = loadApps()
            async let projectLoad: Void = loadProjects()
            _ = await (appLoad, projectLoad)
        }
        loading = task
        return task
    }

    private func loadApps() async {
        apps = await runner.listApps()
        rebuild()
    }

    private func loadProjects() async {
        projects = await runner.listProjects()
        rebuild()
    }

    private func rebuild() {
        let words = query.split(whereSeparator: \.isWhitespace).map(String.init)
        var result: [Row] = [.header("Apps")]
        switch apps {
        case nil: result.append(.note("Looking for apps…"))
        case .failure: result.append(.note("Could not read the app list."))
        case .success(let list):
            result += matching(list.apps.map(Entry.init(app:)), words: words)
            result += list.problems.map(Row.note)
        }
        result.append(.header("Projects"))
        switch projects {
        case nil: result.append(.note("Looking for projects…"))
        case .failure: result.append(.note("Could not read the project list."))
        case .success(let list) where list.isEmpty:
            result.append(.note("No projects yet. Register one with: istesso projects add NAME FOLDER"))
        case .success(let list):
            result += matching(list.map(Entry.init(project:)), words: words)
        }
        rows = result
        let indices = entryIndices
        selectedRow = indices.first { index in
            if case .entry(let entry) = rows[index] { return entry.id == chosenID }
            return false
        } ?? indices.first
        onChange?()
    }

    private func matching(_ entries: [Entry], words: [String]) -> [Row] {
        guard let first = words.first else { return entries.map(Row.entry) }
        let options: String.CompareOptions = [.caseInsensitive, .diacriticInsensitive]
        var prefixes: [Row] = []
        var others: [Row] = []
        for entry in entries where words.allSatisfy({ word in
            entry.title.range(of: word, options: options) != nil ||
                entry.machine.range(of: word, options: options) != nil
        }) {
            if entry.title.range(of: first, options: options.union(.anchored)) != nil {
                prefixes.append(.entry(entry))
            } else {
                others.append(.entry(entry))
            }
        }
        if prefixes.isEmpty && others.isEmpty && !entries.isEmpty { return [.note("No matches")] }
        return prefixes + others
    }

    private var entryIndices: [Int] {
        rows.indices.filter { index in
            if case .entry = rows[index] { return true }
            return false
        }
    }

    public func moveSelection(_ step: Int) {
        let indices = entryIndices
        guard let selectedRow, let position = indices.firstIndex(of: selectedRow) else { return }
        let offset = min(max(step, -position), indices.count - 1 - position)
        select(row: indices[position + offset])
    }

    public func select(row: Int) {
        guard rows.indices.contains(row), case .entry(let entry) = rows[row] else { return }
        chosenID = entry.id
        selectedRow = row
        onChange?()
    }

    public func activateSelected() {
        guard let selectedRow, case .entry(let entry) = rows[selectedRow] else { return }
        switch entry.action {
        case .open(let id):
            guard opening.insert(id).inserted else { return }
            let target = "\(entry.title) on \(entry.machine)"
            status = "Opening \(target)…"
            Task {
                let code = await runner.open(appID: id)
                opening.remove(id)
                switch code {
                case 0: status = "Opened \(target)."
                case 3: status = "A latency measurement is running. Try again when it is over."
                default: status = "Could not open \(target) (exit \(code))."
                }
            }
        case .copy(let command):
            clipboard.copy(command)
            let offline = entry.mark?.contains("!") == true
            status = "Copied “\(command)”. Paste it in a terminal." +
                (offline ? " \(entry.machine) is not answering." : "")
        case .none:
            status = "This project cannot be opened by name."
        }
    }
}
