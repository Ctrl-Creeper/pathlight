import Foundation

/// Answers "which process has files open under this folder right now?".
nonisolated protocol ProcessHinting: Sendable {
    /// Open file paths under `root` mapped to the owning process name.
    func openPaths(under root: URL) async -> [String: String]
}

/// Snapshots open files with `lsof`. One call costs roughly 100–300 ms of CPU,
/// so callers rate-limit it; FSEvents itself never tells us the writer.
// ponytail: lsof snapshot heuristic. Exact attribution needs Endpoint Security
// (entitlement + system extension) on macOS, fanotify on Linux.
nonisolated struct LsofProcessHintService: ProcessHinting {
    func openPaths(under root: URL) async -> [String: String] {
        let rootPrefix = root.standardizedFileURL.path + "/"
        return await Task.detached(priority: .utility) {
            Self.snapshot(rootPrefix: rootPrefix)
        }.value
    }

    private static func snapshot(rootPrefix: String) -> [String: String] {
        let process = Process()
        process.executableURL = URL(filePath: "/usr/sbin/lsof")
        // -F: machine-readable fields (pid, command, fd, name); -n -l -P skip name lookups; -w drops warnings.
        process.arguments = ["-Fpcfn", "-n", "-l", "-P", "-w"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return [:]
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return parse(String(decoding: data, as: UTF8.self), rootPrefix: rootPrefix)
    }

    /// Parses `lsof -F` output: `p<pid>`, `c<command>`, then `f<fd>`/`n<path>`
    /// pairs. Only numeric descriptors count: `cwd`, `txt`, and `mem` entries mean
    /// a process merely sits in or maps the folder, not that it writes there.
    nonisolated static func parse(_ output: String, rootPrefix: String) -> [String: String] {
        var command = ""
        var isDataDescriptor = true
        var result: [String: String] = [:]
        for line in output.split(separator: "\n", omittingEmptySubsequences: true) {
            guard let tag = line.first else { continue }
            let value = String(line.dropFirst())
            switch tag {
            case "c":
                command = value
            case "f":
                isDataDescriptor = value.allSatisfy(\.isNumber)
            case "n":
                if isDataDescriptor, value.hasPrefix(rootPrefix), !command.isEmpty {
                    result[value] = command
                }
            default:
                continue
            }
        }
        return result
    }
}

enum ProcessHintMatcher {
    /// Attaches a process name to each event whose path, or an ancestor below
    /// the root, is open. When exactly one process has anything open under the
    /// root, unmatched events fall back to it.
    nonisolated static func annotate(
        _ events: [DiskActivityEvent],
        hints: [String: String],
        rootPath: URL
    ) -> [DiskActivityEvent] {
        guard !hints.isEmpty else {
            return events
        }
        let root = rootPath.standardizedFileURL.path
        let distinctProcesses = Set(hints.values)
        let soleProcess = distinctProcesses.count == 1 ? distinctProcesses.first : nil

        return events.map { event in
            guard event.processName == nil else {
                return event
            }
            var candidate = event.path.standardizedFileURL.path
            while candidate.hasPrefix(root), candidate != root {
                if let process = hints[candidate] {
                    return event.withProcessName(process)
                }
                let parent = (candidate as NSString).deletingLastPathComponent
                guard parent != candidate else { break }
                candidate = parent
            }
            return soleProcess.map(event.withProcessName) ?? event
        }
    }
}
