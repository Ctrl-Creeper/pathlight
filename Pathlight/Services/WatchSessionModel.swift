import Foundation

nonisolated enum WatchSessionHistoryState: Equatable, Sendable {
    case live
    case catchingUp
    case gapDetected
}

struct WatchSessionModel: Equatable, Sendable {
    let id: UUID
    let rootPath: URL
    let startedAt: Date
    private(set) var events: [DiskActivityEvent]
    private(set) var lastObservedEventID: UInt64?
    private(set) var historyState: WatchSessionHistoryState

    init(
        id: UUID = UUID(),
        rootPath: URL,
        startedAt: Date = Date(),
        events: [DiskActivityEvent] = [],
        lastObservedEventID: UInt64? = nil,
        historyState: WatchSessionHistoryState = .live
    ) {
        self.id = id
        self.rootPath = rootPath
        self.startedAt = startedAt
        self.events = events
        self.lastObservedEventID = lastObservedEventID
        self.historyState = historyState
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

    mutating func record(eventID: UInt64) {
        lastObservedEventID = max(lastObservedEventID ?? 0, eventID)
    }

    mutating func setHistoryState(_ historyState: WatchSessionHistoryState) {
        self.historyState = historyState
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
