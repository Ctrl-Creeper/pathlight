import Foundation
import XCTest
@testable import PathlightCore

/// End-to-end: a created+modified burst on one file reaches the journal as a
/// single row with the summed delta, written once the coalescing window closes.
@MainActor
final class AppModelJournalTests: XCTestCase {
    func testBurstIsJournaledOnceAfterCoalescing() async throws {
        let root = URL(filePath: "/tmp/pathlight-journal-tests/\(UUID().uuidString)", directoryHint: .isDirectory)
        let file = root.appending(path: "copy.txt")
        let start = Date()
        let monitor = ScriptedDiskActivityMonitor(events: [
            .change(DiskActivityChange(kind: .created, path: file, rootPath: root, timestamp: start), eventID: 1),
            .change(DiskActivityChange(kind: .modified, path: file, rootPath: root, timestamp: start.addingTimeInterval(0.2)), eventID: 2),
            .change(DiskActivityChange(kind: .created, path: root.appending(path: "other.txt"), rootPath: root, timestamp: start.addingTimeInterval(0.3)), eventID: 3)
        ])
        let store = InMemoryActivityEventStore()
        let sizes = SizeTable(sizes: [file.path: [100, 400], root.appending(path: "other.txt").path: [7]])
        let model = AppModel(
            dependencies: AppDependencies(
                systemActions: .inert,
                activityMonitor: monitor,
                activityAttribution: stubActivityAttribution,
                activitySizeProviders: { _ in
                    ActivitySizeProviders(
                        size: { sizes.next(for: $0) },
                        known: { sizes.known(for: $0) }
                    )
                },
                activityEventStore: store,
                longTermWatchTargets: LongTermWatchTargetStore(
                    persistence: UserDefaultsLongTermWatchTargetPersistence(
                        defaults: UserDefaults(suiteName: "AppModelJournalTests.\(UUID().uuidString)")!
                    )
                ),
                activityStoragePreferences: FixedActivityStoragePreferencesStore(encryptNewData: false),
                launchAtLoginService: NoopLaunchAtLoginService()
            )
        )
        defer { model.cleanup() }

        model.startShortTermWatch(rootPath: root)

        try await Task.sleep(for: .milliseconds(400))
        let earlyCalls = await store.appendCallCount()
        XCTAssertEqual(earlyCalls, 0, "rows must wait for the coalescing window")

        try await Task.sleep(for: .milliseconds(1_400))
        let rows = await store.allEvents()
        let calls = await store.appendCallCount()
        XCTAssertEqual(calls, 1)
        XCTAssertEqual(rows.count, 2)
        let copyRow = rows.first { $0.path.lastPathComponent == "copy.txt" }
        XCTAssertEqual(copyRow?.kind, .created)
        XCTAssertEqual(copyRow?.byteDelta, 400, "created 100 then grew by 300")
        XCTAssertEqual(rows.first { $0.path.lastPathComponent == "other.txt" }?.byteDelta, 7)
    }

    func testMergedVersionAfterEarlyCommitWritesOnlyItsNewDelta() {
        let root = URL(filePath: "/tmp/pathlight-journal-tests", directoryHint: .isDirectory)
        let path = root.appending(path: "continuous-copy.bin")
        let first = DiskActivityEvent(
            kind: .created,
            path: path,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100),
            byteDelta: 100,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
        let laterMergedVersion = DiskActivityEvent(
            kind: .created,
            path: path,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100.5),
            byteDelta: 400,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )

        let incremental = AppModel.incrementalJournalEvent(laterMergedVersion, after: first)

        XCTAssertEqual(incremental?.kind, .modified)
        XCTAssertEqual(incremental?.byteDelta, 300)
        XCTAssertEqual((first.byteDelta ?? 0) + (incremental?.byteDelta ?? 0), 400)
    }
}

/// Returns scripted sizes in order and remembers the last one as "known".
private final class SizeTable: @unchecked Sendable {
    private let lock = NSLock()
    private var sizes: [String: [Int64]]
    private var knownSizes: [String: Int64] = [:]

    init(sizes: [String: [Int64]]) {
        self.sizes = sizes
    }

    func next(for url: URL) -> Int64? {
        lock.lock()
        defer { lock.unlock() }
        guard var queue = sizes[url.path], !queue.isEmpty else {
            return knownSizes[url.path]
        }
        let size = queue.removeFirst()
        sizes[url.path] = queue
        knownSizes[url.path] = size
        return size
    }

    func known(for url: URL) -> Int64? {
        lock.lock()
        defer { lock.unlock() }
        return knownSizes[url.path]
    }
}

private final class ScriptedDiskActivityMonitor: DiskActivityMonitoring, @unchecked Sendable {
    private let events: [DiskActivityStreamEvent]

    init(events: [DiskActivityStreamEvent]) {
        self.events = events
    }

    nonisolated func events(for root: URL, since eventID: UInt64?, latency: TimeInterval) -> AsyncStream<DiskActivityStreamEvent> {
        let events = events
        return AsyncStream { continuation in
            for event in events {
                continuation.yield(event)
            }
            // Keep the stream open like a real watcher; the task is cancelled by the model.
        }
    }
}

private actor InMemoryActivityEventStore: ActivityEventStoring {
    private var events: [DiskActivityEvent] = []
    private var appendCalls = 0

    func append(_ newEvents: [DiskActivityEvent]) async throws {
        appendCalls += 1
        events.append(contentsOf: newEvents)
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] {
        Array(events.filter { $0.rootPath.standardizedFileURL == rootPath.standardizedFileURL }.prefix(limit))
    }

    func enforceStoragePolicy(_ preferences: ActivityStoragePreferences, eventJournalLimitBytes: Int64, now: Date) async throws {}

    func allEvents() -> [DiskActivityEvent] { events }
    func appendCallCount() -> Int { appendCalls }
}

private final class NoopLaunchAtLoginService: LaunchAtLoginControlling {
    func currentStatus() -> LaunchAtLoginStatus { .disabled }
    func setEnabled(_ enabled: Bool) throws {}
    func openLoginItemsSettings() {}
}
