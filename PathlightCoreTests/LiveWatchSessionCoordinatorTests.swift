import Foundation
import Testing
@testable import PathlightCore

@Suite("Live watch session coordinator")
struct LiveWatchSessionCoordinatorTests {
    @Test("publishes initial session and attributed updates")
    func publishesInitialSessionAndAttributedUpdates() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "installer.dmg")
        let startedAt = Date(timeIntervalSince1970: 100)
        let change = DiskActivityChange(
            kind: .created,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 110)
        )
        let coordinator = LiveWatchSessionCoordinator(
            monitor: StaticDiskActivityMonitor(changes: [change])
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: startedAt,
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProvider: { url in
                url.path == file.path ? 2_048 : nil
            }
        ).makeAsyncIterator()

        let initialSession = await iterator.next()
        #expect(initialSession?.rootPath == root.standardizedFileURL)
        #expect(initialSession?.startedAt == startedAt)
        #expect(initialSession?.events.isEmpty == true)

        let updatedSession = await iterator.next()
        #expect(updatedSession?.events == [
            DiskActivityEvent(
                kind: .created,
                path: file,
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 110),
                byteDelta: 2_048,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        ])

        let finishedSession = await iterator.next()
        #expect(finishedSession == nil)
    }

    @Test("does not publish update snapshots for filtered changes")
    func skipsFilteredChanges() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "tiny.txt")
        let change = DiskActivityChange(
            kind: .modified,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 120)
        )
        let coordinator = LiveWatchSessionCoordinator(
            monitor: StaticDiskActivityMonitor(changes: [change])
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100),
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 1_024,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProvider: { _ in 512 }
        ).makeAsyncIterator()

        let initialSession = await iterator.next()
        #expect(initialSession?.events.isEmpty == true)

        let finishedSession = await iterator.next()
        #expect(finishedSession == nil)
    }
}

private struct StaticDiskActivityMonitor: DiskActivityMonitoring {
    let changes: [DiskActivityChange]

    nonisolated func changes(for root: URL) -> AsyncStream<DiskActivityChange> {
        AsyncStream { continuation in
            for change in changes where change.rootPath.standardizedFileURL == root.standardizedFileURL {
                continuation.yield(change)
            }
            continuation.finish()
        }
    }
}
