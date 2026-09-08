import Foundation

nonisolated struct ActivityHistoryBucket: Equatable, Sendable {
    let startDate: Date
    let endDate: Date
    let byteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
}

nonisolated struct ActivityHistorySnapshot: Equatable, Sendable {
    let rootPath: URL
    let generatedAt: Date
    /// Totals cover every event still retained for this root, not only the
    /// ones in `recentEvents`.
    let totalNetByteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
    let buckets: [ActivityHistoryBucket]
    /// Newest events, capped for display.
    let recentEvents: [DiskActivityEvent]
    /// True when retained history holds more events than `recentEvents` shows.
    let isTruncated: Bool

    init(
        rootPath: URL,
        generatedAt: Date,
        totalNetByteDelta: Int64,
        eventCount: Int,
        unknownSizeEventCount: Int,
        buckets: [ActivityHistoryBucket],
        recentEvents: [DiskActivityEvent],
        isTruncated: Bool = false
    ) {
        self.rootPath = rootPath
        self.generatedAt = generatedAt
        self.totalNetByteDelta = totalNetByteDelta
        self.eventCount = eventCount
        self.unknownSizeEventCount = unknownSizeEventCount
        self.buckets = buckets
        self.recentEvents = recentEvents
        self.isTruncated = isTruncated
    }
}

/// One read of a root's retained history: the newest events for display plus
/// totals over everything retained.
///
/// The two are separate because they used to be the same thing — the dashboard
/// summed the capped page, so a folder with more events than the cap reported a
/// fraction of its real net change and said nothing about the rest.
nonisolated struct ActivityEventPage: Equatable, Sendable {
    let events: [DiskActivityEvent]
    let totalEventCount: Int
    let totalNetByteDelta: Int64
    let unknownSizeEventCount: Int
    let buckets: [ActivityHistoryBucket]

    static let empty = ActivityEventPage(
        events: [],
        totalEventCount: 0,
        totalNetByteDelta: 0,
        unknownSizeEventCount: 0,
        buckets: []
    )
}

/// Accumulates a page in one pass, holding at most `limit` events in memory so
/// a long journal cannot be read into RAM whole.
nonisolated struct ActivityEventPageBuilder {
    private let limit: Int
    private let bucketInterval: TimeInterval
    private var window: [DiskActivityEvent] = []
    private var nextSlot = 0
    private var totalEventCount = 0
    private var totalNetByteDelta: Int64 = 0
    private var unknownSizeEventCount = 0
    private var bucketTotals: [Date: (byteDelta: Int64, eventCount: Int, unknownSizeEventCount: Int)] = [:]

    init(limit: Int, bucketInterval: TimeInterval) {
        self.limit = max(limit, 0)
        self.bucketInterval = max(bucketInterval, 1)
    }

    mutating func add(_ event: DiskActivityEvent) {
        totalEventCount += 1
        if let byteDelta = event.byteDelta {
            totalNetByteDelta += byteDelta
        } else {
            unknownSizeEventCount += 1
        }

        let start = ActivityHistoryService.bucketStart(for: event.timestamp, interval: bucketInterval)
        var totals = bucketTotals[start] ?? (0, 0, 0)
        totals.byteDelta += event.byteDelta ?? 0
        totals.eventCount += 1
        totals.unknownSizeEventCount += event.byteDelta == nil ? 1 : 0
        bucketTotals[start] = totals

        // ponytail: the journal is append-ordered by time, so keeping the last
        // `limit` lines keeps the newest events without buffering the rest.
        guard limit > 0 else { return }
        if window.count < limit {
            window.append(event)
        } else {
            window[nextSlot] = event
            nextSlot = (nextSlot + 1) % limit
        }
    }

    func page() -> ActivityEventPage {
        ActivityEventPage(
            events: window.sorted(by: ActivityHistoryService.recentEventSort).prefix(limit).map { $0 },
            totalEventCount: totalEventCount,
            totalNetByteDelta: totalNetByteDelta,
            unknownSizeEventCount: unknownSizeEventCount,
            buckets: bucketTotals
                .map { start, totals in
                    ActivityHistoryBucket(
                        startDate: start,
                        endDate: start.addingTimeInterval(bucketInterval),
                        byteDelta: totals.byteDelta,
                        eventCount: totals.eventCount,
                        unknownSizeEventCount: totals.unknownSizeEventCount
                    )
                }
                .sorted { $0.startDate < $1.startDate }
        )
    }
}

struct ActivityHistoryService: Sendable {
    private let store: any ActivityEventStoring

    init(store: any ActivityEventStoring) {
        self.store = store
    }

    func loadHistory(
        rootPath: URL,
        eventLimit: Int = 500,
        bucketInterval: TimeInterval,
        generatedAt: Date = Date()
    ) async throws -> ActivityHistorySnapshot {
        let page = try await store.loadEventPage(
            rootPath: rootPath,
            limit: eventLimit,
            bucketInterval: max(bucketInterval, 1)
        )

        return ActivityHistorySnapshot(
            rootPath: rootPath.standardizedFileURL,
            generatedAt: generatedAt,
            totalNetByteDelta: page.totalNetByteDelta,
            eventCount: page.totalEventCount,
            unknownSizeEventCount: page.unknownSizeEventCount,
            buckets: page.buckets,
            recentEvents: page.events,
            isTruncated: page.totalEventCount > page.events.count
        )
    }

    nonisolated static func bucketStart(for date: Date, interval: TimeInterval) -> Date {
        let timestamp = date.timeIntervalSince1970
        return Date(timeIntervalSince1970: floor(timestamp / interval) * interval)
    }

    nonisolated static func recentEventSort(lhs: DiskActivityEvent, rhs: DiskActivityEvent) -> Bool {
        if lhs.timestamp == rhs.timestamp {
            return lhs.path.path > rhs.path.path
        }
        return lhs.timestamp > rhs.timestamp
    }
}
