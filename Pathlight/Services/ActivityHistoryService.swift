import Foundation
import PathlightRustCore

nonisolated struct ActivityHistoryBucket: Equatable, Sendable {
    let startDate: Date
    let endDate: Date
    let byteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
}

/// Net change of one immediate child of the watch root, over every retained row.
nonisolated struct ActivityHistoryChildTotal: Equatable, Sendable {
    var byteDelta: Int64 = 0
    var eventCount = 0
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
    /// When this folder last changed, across everything retained for it — not
    /// only what the current filter, sort and page happen to show.
    let latestEventAt: Date?
    /// Keyed by the name of the child; covers every retained row like the
    /// totals do, which is why it is not derived from `recentEvents`.
    let childTotals: [String: ActivityHistoryChildTotal]

    init(
        rootPath: URL,
        generatedAt: Date,
        totalNetByteDelta: Int64,
        eventCount: Int,
        unknownSizeEventCount: Int,
        buckets: [ActivityHistoryBucket],
        recentEvents: [DiskActivityEvent],
        isTruncated: Bool = false,
        latestEventAt: Date? = nil,
        childTotals: [String: ActivityHistoryChildTotal] = [:]
    ) {
        self.rootPath = rootPath
        self.generatedAt = generatedAt
        self.totalNetByteDelta = totalNetByteDelta
        self.eventCount = eventCount
        self.unknownSizeEventCount = unknownSizeEventCount
        self.buckets = buckets
        self.recentEvents = recentEvents
        self.isTruncated = isTruncated
        self.latestEventAt = latestEventAt
        self.childTotals = childTotals
    }
}

/// What a person narrowed the history to.
///
/// Matching, ordering and paging are the Rust core's (`core/src/history.rs`),
/// the same for every host.
nonisolated struct ActivityHistoryQuery: Equatable, Sendable {
    /// Part of a path, matched without regard to case. Empty matches every row.
    var text: String = ""
    /// One kind of change, or every kind.
    var kind: DiskActivityEventKind?
    /// Biggest change first rather than newest first. The sign is ignored: the
    /// question is which change moved the most bytes, and a deletion moves as
    /// many as the write that made the file.
    var largestFirst = false
    /// Matching rows to step over before the listed page starts.
    var skip = 0

    static let everything = ActivityHistoryQuery()

    var isEverything: Bool { self == .everything }

    var isNarrowed: Bool { !text.isEmpty || kind != nil }

    /// The same question as the core asks it, which is where it is answered.
    var coreQuery: Query {
        Query(
            text: text,
            kind: kind?.core,
            largestFirst: largestFirst,
            skip: UInt32(clamping: max(skip, 0))
        )
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
    /// The newest event for this root, whatever the query asked for.
    let latestEventAt: Date?
    let childTotals: [String: ActivityHistoryChildTotal]

    static let empty = ActivityEventPage(
        events: [],
        totalEventCount: 0,
        totalNetByteDelta: 0,
        unknownSizeEventCount: 0,
        buckets: [],
        latestEventAt: nil,
        childTotals: [:]
    )
}

nonisolated extension ActivityEventPage {
    /// What the core folded the journal into, in this app's types.
    init(_ history: HistorySnapshot) {
        self.init(
            events: history.recentEvents.map(DiskActivityEvent.init),
            totalEventCount: Int(history.eventCount),
            totalNetByteDelta: history.totalNetByteDelta,
            unknownSizeEventCount: Int(history.unknownSizeEventCount),
            buckets: history.buckets.map {
                ActivityHistoryBucket(
                    startDate: $0.start,
                    endDate: $0.end,
                    byteDelta: $0.byteDelta,
                    eventCount: Int($0.eventCount),
                    unknownSizeEventCount: Int($0.unknownSizeEventCount)
                )
            },
            latestEventAt: history.latestEventAt,
            childTotals: history.childTotals.mapValues {
                ActivityHistoryChildTotal(byteDelta: $0.byteDelta, eventCount: Int($0.eventCount))
            }
        )
    }
}


struct ActivityHistoryService: Sendable {
    /// How many rows one read lists. Also how far a page button moves.
    static let pageSize = 500

    private let store: any ActivityEventStoring

    init(store: any ActivityEventStoring) {
        self.store = store
    }

    func loadHistory(
        rootPath: URL,
        eventLimit: Int = pageSize,
        bucketInterval: TimeInterval,
        query: ActivityHistoryQuery = .everything,
        generatedAt: Date = Date()
    ) async throws -> ActivityHistorySnapshot {
        let page = try await store.loadEventPage(
            rootPath: rootPath,
            limit: eventLimit,
            bucketInterval: max(bucketInterval, 1),
            query: query
        )

        return ActivityHistorySnapshot(
            rootPath: rootPath.standardizedFileURL,
            generatedAt: generatedAt,
            totalNetByteDelta: page.totalNetByteDelta,
            eventCount: page.totalEventCount,
            unknownSizeEventCount: page.unknownSizeEventCount,
            buckets: page.buckets,
            recentEvents: page.events,
            isTruncated: page.totalEventCount > max(query.skip, 0) + page.events.count,
            latestEventAt: page.latestEventAt,
            childTotals: page.childTotals
        )
    }

    nonisolated static func bucketStart(for date: Date, interval: TimeInterval) -> Date {
        let timestamp = date.timeIntervalSince1970
        return Date(timeIntervalSince1970: floor(timestamp / interval) * interval)
    }
}
