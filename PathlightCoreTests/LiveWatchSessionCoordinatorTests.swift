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
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 301)])
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

    @Test("publishes the cursor after filtered changes")
    func publishesCursorAfterFilteredChanges() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "tiny.txt")
        let change = DiskActivityChange(
            kind: .modified,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 120)
        )
        let coordinator = LiveWatchSessionCoordinator(
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 302)])
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

        let cursorUpdate = await iterator.next()
        #expect(cursorUpdate?.events.isEmpty == true)
        #expect(cursorUpdate?.lastObservedEventID == 302)

        let finishedSession = await iterator.next()
        #expect(finishedSession == nil)
    }
}

private struct StaticDiskActivityMonitor: DiskActivityMonitoring {
    let events: [DiskActivityStreamEvent]

    nonisolated func events(for root: URL, since eventID: UInt64?, latency: TimeInterval) -> AsyncStream<DiskActivityStreamEvent> {
        AsyncStream { continuation in
            for event in events {
                continuation.yield(event)
            }
            continuation.finish()
        }
    }
}
