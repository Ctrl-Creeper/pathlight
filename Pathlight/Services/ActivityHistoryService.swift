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
    let totalNetByteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
    let buckets: [ActivityHistoryBucket]
    let recentEvents: [DiskActivityEvent]
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
        let events = try await store.loadEvents(rootPath: rootPath, limit: eventLimit)
        let sortedRecentEvents = events.sorted(by: Self.recentEventSort)
        let totalNetByteDelta = sortedRecentEvents.compactMap(\.byteDelta).reduce(Int64(0), +)
        let unknownSizeEventCount = sortedRecentEvents.filter { $0.byteDelta == nil }.count

        return ActivityHistorySnapshot(
            rootPath: rootPath.standardizedFileURL,
            generatedAt: generatedAt,
            totalNetByteDelta: totalNetByteDelta,
            eventCount: sortedRecentEvents.count,
            unknownSizeEventCount: unknownSizeEventCount,
            buckets: Self.buckets(for: sortedRecentEvents, interval: max(bucketInterval, 1)),
            recentEvents: sortedRecentEvents
        )
    }

    private static func buckets(
        for events: [DiskActivityEvent],
        interval: TimeInterval
    ) -> [ActivityHistoryBucket] {
        let groups = Dictionary(grouping: events) { event in
            bucketStart(for: event.timestamp, interval: interval)
        }

        return groups
            .map { startDate, bucketEvents in
                ActivityHistoryBucket(
                    startDate: startDate,
                    endDate: startDate.addingTimeInterval(interval),
                    byteDelta: bucketEvents.compactMap(\.byteDelta).reduce(Int64(0), +),
                    eventCount: bucketEvents.count,
                    unknownSizeEventCount: bucketEvents.filter { $0.byteDelta == nil }.count
                )
            }
            .sorted { $0.startDate < $1.startDate }
    }

    private static func bucketStart(for date: Date, interval: TimeInterval) -> Date {
        let timestamp = date.timeIntervalSince1970
        return Date(timeIntervalSince1970: floor(timestamp / interval) * interval)
    }

    private static func recentEventSort(lhs: DiskActivityEvent, rhs: DiskActivityEvent) -> Bool {
        if lhs.timestamp == rhs.timestamp {
            return lhs.path.path > rhs.path.path
        }
        return lhs.timestamp > rhs.timestamp
    }
}
