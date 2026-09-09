import Foundation
import Testing

@testable import PathlightCore

@Suite("Command line tool installer")
struct CommandLineToolInstallerTests {
    private let tool = URL(filePath: "/Applications/Pathlight.app/Contents/MacOS/pathlight-monitor")

    /// The app deliberately owns none of the policy: the command decides where
    /// it goes on each platform, and its report is what the user needs, down to
    /// the line they have to paste into a shell profile.
    @Test("asks the bundled command to install itself and reports what it said")
    func delegatesToTheCommand() throws {
        let report = """
            Installed /Users/tester/.local/bin/pathlight-monitor.
            /Users/tester/.local/bin is not in PATH yet. Add it:
              export PATH="/Users/tester/.local/bin:$PATH"   # in your shell profile
            """
        let invocations = Invocations()
        let installer = CommandLineToolInstaller(tool: tool) { tool, arguments in
            invocations.record(tool, arguments)
            return report
        }

        #expect(try installer.install() == report)
        #expect(invocations.calls == [(tool.path, ["install-cli"])])
    }

    /// An Xcode build from source carries no command, because the release
    /// pipeline is what copies one in. That is a sentence to show, not a crash,
    /// and nothing may be launched.
    @Test("a build without the command says so instead of launching anything")
    func reportsAMissingCommand() {
        let invocations = Invocations()
        let installer = CommandLineToolInstaller(tool: nil) { tool, arguments in
            invocations.record(tool, arguments)
            return ""
        }

        #expect(throws: CommandLineToolError.notBundled) { try installer.install() }
        #expect(invocations.calls.isEmpty)
    }

    @Test("a command that fails surfaces its own complaint")
    func surfacesFailure() {
        let installer = CommandLineToolInstaller(tool: tool) { _, _ in
            throw CommandLineToolError.failed("pathlight-monitor: permission denied")
        }

        #expect(throws: CommandLineToolError.failed("pathlight-monitor: permission denied")) {
            try installer.install()
        }
    }

    /// The message a user reads when something they cannot see went wrong.
    @Test("an empty complaint still says something")
    func neverShowsAnEmptyError() {
        #expect(CommandLineToolError.failed("").errorDescription?.isEmpty == false)
    }

    private final class Invocations: @unchecked Sendable {
        private let lock = NSLock()
        private var recorded: [(String, [String])] = []

        var calls: [(String, [String])] {
            lock.withLock { recorded }
        }

        func record(_ tool: URL, _ arguments: [String]) {
            lock.withLock { recorded.append((tool.path, arguments)) }
        }
    }
}

private func == (lhs: [(String, [String])], rhs: [(String, [String])]) -> Bool {
    lhs.count == rhs.count && zip(lhs, rhs).allSatisfy { $0.0 == $1.0 && $0.1 == $1.1 }
}
