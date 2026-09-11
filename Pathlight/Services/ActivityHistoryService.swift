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

/// What a person narrowed the history to.
///
/// The Rust core spells the same rule in `core/src/history.rs` for the other
/// two hosts, and `core/fixtures/history-expectations.json` is what keeps the
/// two spellings answering alike.
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

    func matches(_ event: DiskActivityEvent) -> Bool {
        if let kind, event.kind != kind {
            return false
        }
        return text.isEmpty || event.path.path.localizedCaseInsensitiveContains(text)
    }

    /// Newest first, or biggest first with the newest order as the tiebreak
    /// either way, so two rows never swap places between two reads.
    func sorts(_ lhs: DiskActivityEvent, before rhs: DiskActivityEvent) -> Bool {
        guard largestFirst else {
            return ActivityHistoryService.recentEventSort(lhs: lhs, rhs: rhs)
        }
        let moved = (abs(lhs.byteDelta ?? 0), abs(rhs.byteDelta ?? 0))
        if moved.0 != moved.1 {
            return moved.0 > moved.1
        }
        return ActivityHistoryService.recentEventSort(lhs: lhs, rhs: rhs)
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

/// Accumulates a page in one pass, holding at most twice the page in memory so
/// a long journal cannot be read into RAM whole.
nonisolated struct ActivityEventPageBuilder {
    private let limit: Int
    private let skip: Int
    private let bucketInterval: TimeInterval
    private let query: ActivityHistoryQuery
    private var window: [DiskActivityEvent] = []
    private var totalEventCount = 0
    private var totalNetByteDelta: Int64 = 0
    private var unknownSizeEventCount = 0
    private var bucketTotals: [Date: (byteDelta: Int64, eventCount: Int, unknownSizeEventCount: Int)] = [:]

    init(
        limit: Int,
        bucketInterval: TimeInterval,
        query: ActivityHistoryQuery = .everything
    ) {
        self.limit = max(limit, 0)
        self.skip = max(query.skip, 0)
        self.bucketInterval = max(bucketInterval, 1)
        self.query = query
    }

    /// The rows the page may hold before it starts: what is listed, plus what
    /// is stepped over to get there.
    private var capacity: Int { limit + skip }

    mutating func add(_ event: DiskActivityEvent) {
        guard query.matches(event) else {
            return
        }
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

        // ponytail: a bounded buffer that is sorted and cut when it fills,
        // rather than a ring holding the last `limit` lines. "Biggest first"
        // is not answerable from the newest rows, and paging is not either;
        // memory stays at twice the page.
        guard capacity > 0 else { return }
        window.append(event)
        if window.count >= capacity * 2 {
            window.sort(by: query.sorts(_:before:))
            window = Array(window.prefix(capacity))
        }
    }

    func page() -> ActivityEventPage {
        ActivityEventPage(
            events: Array(
                window
                    .sorted(by: query.sorts(_:before:))
                    .dropFirst(skip)
                    .prefix(limit)
            ),
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
            isTruncated: page.totalEventCount > max(query.skip, 0) + page.events.count
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
