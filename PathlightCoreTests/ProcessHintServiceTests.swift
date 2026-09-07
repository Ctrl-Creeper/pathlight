import Foundation
import Testing
@testable import PathlightCore

@Suite("Process hints")
struct ProcessHintServiceTests {
    private let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

    @Test("parses lsof field output into path to process map")
    func parsesLsofOutput() {
        let output = """
        p123
        cSafari
        n/Users/example/Downloads/movie.mp4
        n/Applications/Safari.app/Contents/MacOS/Safari
        p456
        cFinder
        n/Users/example/Downloads/Archive
        """
        let hints = LsofProcessHintService.parse(output, rootPrefix: "/Users/example/Downloads/")

        #expect(hints == [
            "/Users/example/Downloads/movie.mp4": "Safari",
            "/Users/example/Downloads/Archive": "Finder"
        ])
    }

    @Test("annotates events by exact path, ancestor, or sole process")
    func annotatesEvents() {
        let exact = event("movie.mp4")
        let nested = event("Archive/part1.zip")
        let unrelated = event("notes.txt")

        let twoProcesses = ProcessHintMatcher.annotate(
            [exact, nested, unrelated],
            hints: [
                root.appending(path: "movie.mp4").path: "Safari",
                root.appending(path: "Archive").path: "Finder"
            ],
            rootPath: root
        )
        #expect(twoProcesses.map(\.processName) == ["Safari", "Finder", nil])

        let soleProcess = ProcessHintMatcher.annotate(
            [unrelated],
            hints: [root.appending(path: "movie.mp4").path: "Safari"],
            rootPath: root
        )
        #expect(soleProcess.first?.processName == "Safari")
        #expect(ProcessHintMatcher.annotate([unrelated], hints: [:], rootPath: root) == [unrelated])
    }

    @Test("snapshots are rate limited and gated by size for long-term watches")
    func snapshotGating() {
        let now = Date(timeIntervalSince1970: 1_000)
        let small = [event("a", byteDelta: 10)]
        let big = [event("b", byteDelta: 5_000_000)]

        #expect(LiveWatchSessionCoordinator.shouldSnapshotProcesses(for: small, options: .shortTermDefault, lastSnapshotAt: nil, now: now))
        #expect(!LiveWatchSessionCoordinator.shouldSnapshotProcesses(for: small, options: .shortTermDefault, lastSnapshotAt: now.addingTimeInterval(-1), now: now))
        #expect(!LiveWatchSessionCoordinator.shouldSnapshotProcesses(for: small, options: .default, lastSnapshotAt: nil, now: now))
        #expect(LiveWatchSessionCoordinator.shouldSnapshotProcesses(for: big, options: .default, lastSnapshotAt: nil, now: now))
    }

    private func event(_ name: String, byteDelta: Int64 = 1) -> DiskActivityEvent {
        DiskActivityEvent(
            kind: .modified,
            path: root.appending(path: name),
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100),
            byteDelta: byteDelta,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
    }
}
