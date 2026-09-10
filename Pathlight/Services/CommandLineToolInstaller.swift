import Foundation

/// Installs the command line tool that ships inside the app bundle.
///
/// Where the command goes, whether it is linked or copied, and what to tell
/// somebody whose `PATH` does not include it are all decided by the command
/// itself — `install-cli` in `core/src/bin/pathlight-monitor.rs`, which every
/// platform shares. This asks it to install itself and shows what it said.
/// Two implementations of "where does a command belong" eventually disagree,
/// and the one that loses is somebody's `PATH`.
nonisolated struct CommandLineToolInstaller: Sendable {
    /// Runs the tool and returns everything it printed, throwing with what it
    /// printed on the error stream instead.
    typealias Launcher = @Sendable (URL, [String]) throws -> String

    /// The bundled command, or `nil` in a build that carries none: the release
    /// pipeline is what copies it into `Contents/MacOS`, so an Xcode build
    /// from source has nothing to install.
    var tool: URL?
    var launch: Launcher

    static let live = CommandLineToolInstaller(
        tool: Bundle.main.url(forAuxiliaryExecutable: "pathlight-monitor"),
        launch: runTool
    )

    /// The command's own report, meant to be shown verbatim: when the
    /// directory it chose is not on `PATH`, the last line is the exact line to
    /// paste into a shell profile.
    func install() throws -> String {
        guard let tool else { throw CommandLineToolError.notBundled }
        return try launch(tool, ["install-cli"])
    }
}

nonisolated enum CommandLineToolError: LocalizedError, Equatable {
    /// A build without the bundled command. Nothing is wrong with it; it just
    /// has nothing to install.
    case notBundled
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .notBundled:
            return "This build of Pathlight does not include the command line tool."
        case .failed(let message):
            return message.isEmpty ? "The command line tool could not be installed." : message
        }
    }
}

// `nonisolated` because the launcher it is assigned to is: the app target
// isolates file-scope functions to the main actor by default, and running a
// child process and draining its pipes is the last thing that belongs there.
private nonisolated func runTool(_ tool: URL, _ arguments: [String]) throws -> String {
    let process = Process()
    process.executableURL = tool
    process.arguments = arguments
    let output = Pipe()
    let errors = Pipe()
    process.standardOutput = output
    process.standardError = errors
    try process.run()
    // Read before waiting: a child that fills a pipe nobody drains never exits.
    let printed = output.fileHandleForReading.readDataToEndOfFile()
    let complained = errors.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw CommandLineToolError.failed(text(complained))
    }
    return text(printed)
}

private nonisolated func text(_ data: Data) -> String {
    String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
}
