import Foundation

public struct AppList: Decodable {
    public let apps: [AppRow]
    public let problems: [String]
}

public struct AppRow: Decodable {
    public enum Host: String, Decodable { case ubuntu, mac, local }

    public let id: String
    public let name: String
    public let machine: String
    public let host: Host
}

public struct ProjectRow: Decodable {
    public struct Capabilities: Decodable {
        public let gpu: Bool
        public let tools: [String]
    }

    public let id: String
    public let name: String
    public let kinds: [String]
    public let developmentMachine: String
    public let reachable: Bool
    public let clash: Bool
    public let capabilities: Capabilities

    enum CodingKeys: String, CodingKey {
        case id, name, kinds, reachable, clash, capabilities
        case developmentMachine = "development_machine"
    }
}

public struct Entry: Equatable {
    public enum Section { case apps, projects }
    public enum Action: Equatable { case open(String), copy(String), none }

    public let id: String
    public let section: Section
    public let title: String
    public let machine: String
    public let detail: String
    public let mark: String?
    public let action: Action

    public init(app: AppRow) {
        id = app.id
        section = .apps
        title = app.name
        machine = app.host == .local ? "this Mac" : app.machine
        detail = "on " + machine
        mark = nil
        action = .open(app.id)
    }

    public init(project: ProjectRow) {
        machine = project.developmentMachine
        id = "project/\(project.id)@\(machine)"
        section = .projects
        title = project.name
        let flags = (project.reachable ? "" : "!") + (project.clash ? "?" : "")
        mark = flags.isEmpty ? nil : flags
        var description = "on " + machine
        if !project.kinds.isEmpty && project.kinds != ["unknown"] {
            description += " · " + project.kinds.joined(separator: ", ")
        }
        let bytes = title.utf8
        if let first = bytes.first, Self.isAlphanumeric(first), bytes.count <= 64,
           bytes.allSatisfy(Self.isWordByte) {
            var word = title
            if project.clash {
                word = machine + ":" + title
                if !machine.utf8.allSatisfy(Self.isWordByte) {
                    word = "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
                }
            }
            action = .copy("ish " + word)
        } else {
            action = .none
            description += " · cannot be opened by name"
        }
        detail = description
    }

    private static func isAlphanumeric(_ byte: UInt8) -> Bool {
        (65...90).contains(byte) || (97...122).contains(byte) || (48...57).contains(byte)
    }

    private static func isWordByte(_ byte: UInt8) -> Bool {
        isAlphanumeric(byte) || byte == 46 || byte == 95 || byte == 45
    }
}

public enum Row: Equatable {
    case header(String), entry(Entry), note(String)
}

public struct LauncherFailure: Error {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }
}

public protocol LauncherRunner {
    func listApps() async -> Result<AppList, LauncherFailure>
    func listProjects() async -> Result<[ProjectRow], LauncherFailure>
    func open(appID: String) async -> Int32
}

public protocol Clipboard {
    func copy(_ text: String)
}
