import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity history service")
struct ActivityHistoryServiceTests {
    @Test("aggregates events into time buckets")
    func aggregatesEventsIntoTimeBuckets() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = [
            event(.created, root: root, name: "a.dmg", timestamp: 3_600, byteDelta: 2_000),
            event(.deleted, root: root, name: "old.zip", timestamp: 3_900, byteDelta: -500),
            event(.modified, root: root, name: "cache.bin", timestamp: 7_200, byteDelta: 300)
        ]
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: events))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: 100,
            bucketInterval: 3_600,
            generatedAt: Date(timeIntervalSince1970: 10_000)
        )

        #expect(history.rootPath == root.standardizedFileURL)
        #expect(history.generatedAt == Date(timeIntervalSince1970: 10_000))
        #expect(history.totalNetByteDelta == 1_800)
        #expect(history.eventCount == 3)
        #expect(history.buckets == [
            ActivityHistoryBucket(
                startDate: Date(timeIntervalSince1970: 3_600),
                endDate: Date(timeIntervalSince1970: 7_200),
                byteDelta: 1_500,
                eventCount: 2,
                unknownSizeEventCount: 0
            ),
            ActivityHistoryBucket(
                startDate: Date(timeIntervalSince1970: 7_200),
                endDate: Date(timeIntervalSince1970: 10_800),
                byteDelta: 300,
                eventCount: 1,
                unknownSizeEventCount: 0
            )
        ])
    }

    /// The card used to report only what the list could hold, so 532 events
    /// and +154 MB rendered as "500 events • +2.1 MB".
    @Test("totals every retained event while listing only the newest page")
    func totalsEveryRetainedEventWhileListingOnePage() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = (0..<10).map { index in
            event(.created, root: root, name: "\(index).bin", timestamp: TimeInterval(index) * 3_600, byteDelta: 1_000)
        }
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: events))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: 3,
            bucketInterval: 3_600,
            generatedAt: Date(timeIntervalSince1970: 100_000)
        )

        #expect(history.eventCount == 10)
        #expect(history.totalNetByteDelta == 10_000)
        #expect(history.recentEvents.count == 3)
        #expect(history.isTruncated)

        let whole = try await service.loadHistory(
            rootPath: root,
            eventLimit: 100,
            bucketInterval: 3_600,
            generatedAt: Date(timeIntervalSince1970: 100_000)
        )
        #expect(!whole.isTruncated)
    }

    @Test("tracks unknown size events without changing net delta")
    func tracksUnknownSizeEvents() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = [
            event(.deleted, root: root, name: "missing.zip", timestamp: 3_600, byteDelta: nil),
            event(.created, root: root, name: "new.zip", timestamp: 3_700, byteDelta: 1_024)
        ]
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: events))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: 100,
            bucketInterval: 3_600,
            generatedAt: Date(timeIntervalSince1970: 10_000)
        )

        #expect(history.totalNetByteDelta == 1_024)
        #expect(history.unknownSizeEventCount == 1)
        #expect(history.buckets.first?.byteDelta == 1_024)
        #expect(history.buckets.first?.unknownSizeEventCount == 1)
    }

    @Test("keeps recent events in descending timestamp order")
    func keepsRecentEventsInDescendingTimestampOrder() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = [
            event(.created, root: root, name: "old.bin", timestamp: 100, byteDelta: 10),
            event(.created, root: root, name: "new.bin", timestamp: 200, byteDelta: 20)
        ]
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: events))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: 100,
            bucketInterval: 60,
            generatedAt: Date(timeIntervalSince1970: 300)
        )

        #expect(history.recentEvents.map(\.path.lastPathComponent) == ["new.bin", "old.bin"])
    }
}

private func event(
    _ kind: DiskActivityEventKind,
    root: URL,
    name: String,
    timestamp: TimeInterval,
    byteDelta: Int64?
) -> DiskActivityEvent {
    DiskActivityEvent(
        kind: kind,
        path: root.appending(path: name),
        rootPath: root,
        timestamp: Date(timeIntervalSince1970: timestamp),
        byteDelta: byteDelta,
        confidence: byteDelta == nil ? .unknown : .confirmed,
        previousPath: nil,
        affectedItemCount: 1
    )
}

private struct StaticActivityEventStore: ActivityEventStoring {
    let events: [DiskActivityEvent]

    func append(_ events: [DiskActivityEvent]) async throws {}

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] {
        Array(events
            .filter { $0.rootPath.standardizedFileURL == rootPath.standardizedFileURL }
            .sorted {
                if $0.timestamp == $1.timestamp {
                    return $0.path.path > $1.path.path
                }
                return $0.timestamp > $1.timestamp
            }
            .prefix(limit))
    }

    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws {}
}
