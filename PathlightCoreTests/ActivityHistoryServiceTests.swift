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

    /// The one place the two aggregations meet.
    ///
    /// `core/src/history.rs` folds rows for the Windows and Linux hosts and
    /// this file folds them for the app, because the journal is Swift's and
    /// does not cross the FFI — the same rule, written twice. The fixture and
    /// its expectations are shared with `core/tests/history.rs`: edit one
    /// implementation's bucket boundary, ordering or unknown-size rule and the
    /// other host's test fails, which is the only thing standing between two
    /// dashboards and two different answers for one folder.
    @Test("folds the shared fixture into the history the Rust core folds it into")
    func foldsTheSharedFixtureLikeTheCore() async throws {
        let (expected, rows) = try sharedFixture()
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: rows))
        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: expected.recentLimit,
            bucketInterval: TimeInterval(expected.bucketIntervalSecs),
            generatedAt: Date(timeIntervalSince1970: 0)
        )

        #expect(history.totalNetByteDelta == expected.totalNetByteDelta)
        #expect(history.eventCount == expected.eventCount)
        #expect(history.unknownSizeEventCount == expected.unknownSizeEventCount)
        #expect(history.isTruncated == expected.isTruncated)
        #expect(history.recentEvents.map(\.path.path) == expected.recentPaths)
        #expect(history.buckets == expected.buckets.map { bucket in
            ActivityHistoryBucket(
                startDate: Date(timeIntervalSince1970: TimeInterval(bucket.startSecs)),
                endDate: Date(
                    timeIntervalSince1970: TimeInterval(bucket.startSecs + expected.bucketIntervalSecs)
                ),
                byteDelta: bucket.byteDelta,
                eventCount: bucket.eventCount,
                unknownSizeEventCount: bucket.unknownSizeEventCount
            )
        })
    }

    /// The other half of the same contract: a narrowed read, which both hosts
    /// have to narrow, order and page identically. The totals are the part
    /// worth pinning — a number under a search box that described only the
    /// visible page would be a lie about the folder.
    @Test("narrows the shared fixture into the rows the Rust core narrows it to")
    func narrowsTheSharedFixtureLikeTheCore() async throws {
        let (expected, rows) = try sharedFixture()
        let search = expected.search
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: rows))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: search.limit,
            bucketInterval: TimeInterval(expected.bucketIntervalSecs),
            query: ActivityHistoryQuery(
                text: search.text,
                largestFirst: search.largestFirst,
                skip: search.skip
            ),
            generatedAt: Date(timeIntervalSince1970: 0)
        )

        #expect(history.eventCount == search.eventCount)
        #expect(history.totalNetByteDelta == search.totalNetByteDelta)
        #expect(history.unknownSizeEventCount == search.unknownSizeEventCount)
        #expect(history.isTruncated == search.isTruncated)
        #expect(history.recentEvents.map(\.path.path) == search.recentPaths)
    }

    /// Asking for one kind of change is asking about that kind: the totals
    /// must not keep counting the rows that were filtered out.
    @Test("one kind of change can be asked for on its own")
    func oneKindOfChangeCanBeAskedForOnItsOwn() async throws {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = [
            event(.created, root: root, name: "a.dmg", timestamp: 3_600, byteDelta: 2_000),
            event(.deleted, root: root, name: "old.zip", timestamp: 3_900, byteDelta: -500)
        ]
        let service = ActivityHistoryService(store: StaticActivityEventStore(events: events))

        let history = try await service.loadHistory(
            rootPath: root,
            eventLimit: 100,
            bucketInterval: 3_600,
            query: ActivityHistoryQuery(kind: .deleted),
            generatedAt: Date(timeIntervalSince1970: 10_000)
        )

        #expect(history.eventCount == 1)
        #expect(history.totalNetByteDelta == -500)
        #expect(history.recentEvents.map(\.path.lastPathComponent) == ["old.zip"])
    }

    private func sharedFixture() throws -> (SharedHistoryExpectations, [DiskActivityEvent]) {
        let fixtures = URL(filePath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appending(path: "core/fixtures", directoryHint: .isDirectory)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let expected = try decoder.decode(
            SharedHistoryExpectations.self,
            from: try Data(contentsOf: fixtures.appending(path: "history-expectations.json"))
        )
        let rows = try String(contentsOf: fixtures.appending(path: "swift-journal.jsonl"), encoding: .utf8)
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map { line in
                try decoder.decode(DiskActivityEvent.self, from: Data(line.utf8))
            }
        return (expected, rows)
    }
}

/// The numbers both hosts must reach, as `core/fixtures/history-expectations.json`
/// spells them.
private struct SharedHistoryExpectations: Decodable {
    struct Bucket: Decodable {
        let startSecs: Int
        let byteDelta: Int64
        let eventCount: Int
        let unknownSizeEventCount: Int
    }

    struct Search: Decodable {
        let text: String
        let largestFirst: Bool
        let skip: Int
        let limit: Int
        let eventCount: Int
        let totalNetByteDelta: Int64
        let unknownSizeEventCount: Int
        let isTruncated: Bool
        let recentPaths: [String]
    }

    let bucketIntervalSecs: Int
    let recentLimit: Int
    let search: Search
    let totalNetByteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
    let isTruncated: Bool
    let recentPaths: [String]
    let buckets: [Bucket]
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
