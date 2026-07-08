import Foundation

struct WatchSessionModel: Equatable, Sendable {
    let id: UUID
    let rootPath: URL
    let startedAt: Date
    private(set) var events: [DiskActivityEvent]

    init(
        id: UUID = UUID(),
        rootPath: URL,
        startedAt: Date = Date(),
        events: [DiskActivityEvent] = []
    ) {
        self.id = id
        self.rootPath = rootPath
        self.startedAt = startedAt
        self.events = events
    }

    mutating func append(_ newEvents: [DiskActivityEvent]) {
        events.append(contentsOf: newEvents)
        events.sort { lhs, rhs in
            if lhs.timestamp == rhs.timestamp {
                return lhs.path.path < rhs.path.path
            }
            return lhs.timestamp < rhs.timestamp
        }
    }

    func summary(endedAt: Date = Date()) -> WatchSessionSummary {
        WatchSessionSummary(
            id: id,
            rootPath: rootPath,
            startedAt: startedAt,
            endedAt: endedAt,
            netByteDelta: events.compactMap(\.byteDelta).reduce(Int64(0), +),
            eventCount: events.count,
            unknownSizeEventCount: events.filter { $0.byteDelta == nil }.count
        )
    }
}
