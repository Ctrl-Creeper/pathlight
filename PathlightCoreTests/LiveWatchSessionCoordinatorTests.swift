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
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 301)]),
            attribution: stubActivityAttribution
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: startedAt,
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProviders: ActivitySizeProviders(size: { url in
                url.path == file.path ? 2_048 : nil
            })
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

    @Test("excluded changes advance the cursor without producing events")
    func excludedChangesAdvanceCursorWithoutEvents() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let noise = root.appending(path: ".DS_Store")
        let change = DiskActivityChange(
            kind: .modified,
            path: noise,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 110)
        )
        let attribution = StubActivityAttribution { change in
            [stubActivityEvent(for: change, byteDelta: 4_096)]
        }
        let coordinator = LiveWatchSessionCoordinator(
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 305)]),
            attribution: { _, _ in attribution }
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100),
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 1,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            exclusionFilter: ActivityExclusionFilter(
                patterns: [".DS_Store"],
                rootPath: root
            ),
            sizeProviders: ActivitySizeProviders(size: { _ in 4_096 })
        ).makeAsyncIterator()

        _ = await iterator.next()
        let updatedSession = await iterator.next()
        #expect(updatedSession?.events.isEmpty == true)
        #expect(updatedSession?.lastObservedEventID == 305)
        #expect(attribution.observedChanges.isEmpty, "an excluded change was measured anyway")
    }

    /// Whether a change is worth recording is attribution's call, and the
    /// answer can be nothing at all. The cursor still has to move past it, or
    /// the next reconnect replays a change that was already judged.
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
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 302)]),
            attribution: { _, _ in StubActivityAttribution() }
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100),
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 1_024,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProviders: ActivitySizeProviders(size: { _ in 512 }, known: { _ in 0 })
        ).makeAsyncIterator()

        let initialSession = await iterator.next()
        #expect(initialSession?.events.isEmpty == true)

        let cursorUpdate = await iterator.next()
        #expect(cursorUpdate?.events.isEmpty == true)
        #expect(cursorUpdate?.lastObservedEventID == 302)

        let finishedSession = await iterator.next()
        #expect(finishedSession == nil)
    }

    /// Recording an event writes the journal, and writing the journal is a
    /// change: on a whole-disk watch that loop feeds itself forever. The
    /// exclusion filter cannot be trusted with it — the user can empty it.
    @Test("never records its own journal, even with no exclusion filter")
    func isolatesItsOwnStorageFromAnyWatch() async {
        let root = URL(filePath: "/", directoryHint: .isDirectory)
        // FSEvents reports resolved paths, which is the spelling the guard
        // holds; `defaultJournalURL()` has not been through `realpath`.
        let journal = URL(filePath: ActivityStorageIsolation.directoryPath, directoryHint: .isDirectory)
            .appending(path: "activity-events.jsonl")
        let change = DiskActivityChange(
            kind: .modified,
            path: journal,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 130)
        )
        let attribution = StubActivityAttribution { change in
            [stubActivityEvent(for: change, byteDelta: 4_096)]
        }
        let coordinator = LiveWatchSessionCoordinator(
            monitor: StaticDiskActivityMonitor(events: [.change(change, eventID: 306)]),
            attribution: { _, _ in attribution }
        )

        var iterator = coordinator.sessions(
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100),
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 0,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProviders: ActivitySizeProviders(size: { _ in 4_096 })
        ).makeAsyncIterator()

        _ = await iterator.next()
        let updatedSession = await iterator.next()
        #expect(updatedSession?.events.isEmpty == true)
        #expect(updatedSession?.lastObservedEventID == 306)
        #expect(
            attribution.observedChanges.isEmpty,
            "measuring the journal is what appends to the size index, which is the next event"
        )
    }

    /// A folder whose name merely starts the same way belongs to someone else.
    @Test("isolates the storage folder itself, not its name prefix")
    func matchesTheStorageFolderOnPathBoundaries() {
        let storage = "/Users/example/Library/Application Support/Pathlight"

        #expect(ActivityStorageIsolation.excludes(storage, storageDirectory: storage))
        #expect(ActivityStorageIsolation.excludes("\(storage)/activity-events.jsonl", storageDirectory: storage))
        #expect(!ActivityStorageIsolation.excludes("\(storage)-backup/notes.txt", storageDirectory: storage))
        #expect(!ActivityStorageIsolation.excludes("/Users/example/Library", storageDirectory: storage))
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
