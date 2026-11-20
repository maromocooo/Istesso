import Foundation

public struct ProcessRunner: LauncherRunner, Sendable {
    private let executable: URL
    private let trial: String
    private let appSeconds: Int
    private let projectSeconds: Int

    public init(istesso: String, trial: String, appSeconds: Int = 10, projectSeconds: Int = 20) {
        executable = URL(fileURLWithPath: istesso)
        self.trial = trial
        self.appSeconds = appSeconds
        self.projectSeconds = projectSeconds
    }

    public func listApps() async -> Result<AppList, LauncherFailure> {
        await list(["launcher", "apps", "--json", "--trial", trial], seconds: appSeconds, name: "app")
    }

    public func listProjects() async -> Result<[ProjectRow], LauncherFailure> {
        await list(["projects", "--json"], seconds: projectSeconds, name: "project")
    }

    private func process(_ arguments: [String]) -> Process {
        let child = Process()
        child.executableURL = executable
        child.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "DAILY_OWNER")
        environment.removeValue(forKey: "DAILY_OPEN_HANDLER")
        child.environment = environment
        child.standardInput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        return child
    }

    public func open(appID: String) async -> Int32 {
        await withCheckedContinuation { continuation in
            let child = process(["launcher", "open", appID, "--trial", trial])
            child.standardOutput = FileHandle.nullDevice
            child.terminationHandler = { continuation.resume(returning: $0.terminationStatus) }
            do {
                try child.run()
            } catch {
                continuation.resume(returning: 127)
            }
        }
    }

    private func list<T: Decodable>(_ arguments: [String], seconds: Int, name: String)
        async -> Result<T, LauncherFailure> {
        await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let result: Result<T, LauncherFailure> = readList(arguments, seconds: seconds, name: name)
                continuation.resume(returning: result)
            }
        }
    }

    private func readList<T: Decodable>(_ arguments: [String], seconds: Int, name: String)
        -> Result<T, LauncherFailure> {
        let failure = LauncherFailure("could not read the \(name) list")
        let child = process(arguments)
        let pipe = Pipe()
        child.standardOutput = pipe
        let exited = DispatchSemaphore(value: 0)
        child.terminationHandler = { _ in exited.signal() }
        let deadline = DispatchTime.now() + .seconds(seconds)
        do {
            try child.run()
            let output = BoundedOutput(pipe.fileHandleForReading)
            defer { output.cancel() }
            let data = try output.collect(until: deadline, failure: failure)
            guard exited.wait(timeout: deadline) == .success else {
                throw LauncherFailure("did not answer in time")
            }
            guard child.terminationStatus == 0 else { throw failure }
            return .success(try JSONDecoder().decode(T.self, from: data))
        } catch {
            if child.isRunning { child.terminate() }
            return .failure(error as? LauncherFailure ?? failure)
        }
    }
}

// Wait for pipe EOF and child exit separately. A stalled writer (including an
// inherited write end) must not keep the caller waiting beyond its deadline.
private final class BoundedOutput {
    private let queue = DispatchQueue(label: "istesso.launcher.stdout")
    private let source: DispatchSourceRead
    private let handle: FileHandle
    private let finished = DispatchSemaphore(value: 0)
    private var data = Data()
    private var complete = false

    init(_ handle: FileHandle) {
        self.handle = handle
        source = DispatchSource.makeReadSource(fileDescriptor: handle.fileDescriptor, queue: queue)
        source.setEventHandler { [weak self] in self?.readAvailable() }
        source.setCancelHandler { try? handle.close() }
        source.resume()
    }

    private func readAvailable() {
        let available = source.data
        let remaining = 4 * 1024 * 1024 - data.count
        if available == 0 {
            complete = true
        } else if available <= UInt(remaining) {
            do {
                if let chunk = try handle.read(upToCount: min(Int(available), 64 * 1024)),
                   !chunk.isEmpty {
                    data.append(chunk)
                    return
                }
                complete = true
            } catch {
                complete = false
            }
        }
        source.cancel()
        finished.signal()
    }

    func collect(until deadline: DispatchTime, failure: LauncherFailure) throws -> Data {
        guard finished.wait(timeout: deadline) == .success else {
            throw LauncherFailure("did not answer in time")
        }
        guard complete else { throw failure }
        return data
    }

    func cancel() { source.cancel() }
}
