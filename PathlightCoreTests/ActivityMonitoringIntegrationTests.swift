import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity monitoring integration")
struct ActivityMonitoringIntegrationTests {
    @Test("resumes from a checkpoint and persists attributed events through the store")
    func resumesFromCheckpointAndPersistsEvents() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let createdFile = root.appending(path: "installer.dmg")
        let deletedFile = root.appending(path: "stale.zip")
        let monitor = RecordingDiskActivityMonitor(events: [
            .historyCaughtUp(eventID: 402),
            .change(
                DiskActivityChange(
                    kind: .created,
                    path: createdFile,
                    rootPath: root,
                    timestamp: Date(timeIntervalSince1970: 210)
                ),
                eventID: 403
            ),
            .change(
                DiskActivityChange(
                    kind: .deleted,
                    path: deletedFile,
                    rootPath: root,
                    timestamp: Date(timeIntervalSince1970: 220)
                ),
                eventID: 404
            )
        ])
        let coordinator = LiveWatchSessionCoordinator(monitor: monitor)

        var observedHistoryStates: [WatchSessionHistoryState] = []
        var lastSession: WatchSessionModel?
        for await session in coordinator.sessions(
            rootPath: root,
            sinceEventID: 400,
            startedAt: Date(timeIntervalSince1970: 200),
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 1,
                aggregationWindow: 0,
                longTermRecordsFileNames: true
            ),
            sizeProvider: { url in
                url.path == createdFile.path ? 4_096 : nil
            },
            priorSizeProvider: { url in
                url.path == deletedFile.path ? 2_048 : nil
            }
        ) {
            observedHistoryStates.append(session.historyState)
            lastSession = session
        }

        #expect(monitor.observedSinceEventID() == 400)
        #expect(observedHistoryStates.first == .catchingUp)
        #expect(observedHistoryStates.last == .live)
        let session = try #require(lastSession)
        #expect(session.lastObservedEventID == 404)
        #expect(session.events.map(\.byteDelta) == [4_096, -2_048])
        #expect(session.events.map(\.confidence) == [.confirmed, .estimated])

        let store = JSONLActivityEventStore(journalURL: journalURL)
        try await store.append(session.events)
        try await store.enforceStoragePolicy(
            ActivityStoragePreferences(
                detailedRetentionDays: 7,
                aggregateRetentionDays: 30,
                storageLimitBytes: 1_024 * 1_024,
                encryptNewData: false
            ),
            eventJournalLimitBytes: 1_024 * 1_024,
            now: Date(timeIntervalSince1970: 300)
        )
        let reloaded = try await store.loadEvents(rootPath: root, limit: 10)

        #expect(reloaded == Array(session.events.reversed()))
    }

    @Test("flags a history gap when the monitor requires a rescan")
    func flagsHistoryGapWhenMonitorRequiresRescan() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let monitor = RecordingDiskActivityMonitor(events: [.requiresRescan(eventID: 501)])
        let coordinator = LiveWatchSessionCoordinator(monitor: monitor)

        var lastSession: WatchSessionModel?
        for await session in coordinator.sessions(
            rootPath: root,
            sinceEventID: 500,
            options: .default,
            sizeProvider: { _ in nil }
        ) {
            lastSession = session
        }

        #expect(lastSession?.historyState == .gapDetected)
        #expect(lastSession?.lastObservedEventID == 501)
    }
}

private final class RecordingDiskActivityMonitor: DiskActivityMonitoring, @unchecked Sendable {
    private let lock = NSLock()
    private let events: [DiskActivityStreamEvent]
    private var sinceEventIDs: [UInt64?] = []

    init(events: [DiskActivityStreamEvent]) {
        self.events = events
    }

    func observedSinceEventID() -> UInt64? {
        lock.lock()
        defer {
            lock.unlock()
        }
        return sinceEventIDs.first ?? nil
    }

    nonisolated func events(for root: URL, since eventID: UInt64?, latency: TimeInterval) -> AsyncStream<DiskActivityStreamEvent> {
        lock.lock()
        sinceEventIDs.append(eventID)
        lock.unlock()
        return AsyncStream { continuation in
            for event in events {
                continuation.yield(event)
            }
            continuation.finish()
        }
    }
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appending(path: "PathlightIntegrationTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}
