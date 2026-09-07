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
    /// Oldest events dropped once `maxRetainedEvents` is exceeded, so a storm
    /// cannot grow memory without bound.
    private(set) var droppedEventCount = 0

    // ponytail: fixed cap; make it an option if someone needs longer live scrollback.
    static let maxRetainedEvents = 5_000

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

    mutating func append(
        _ newEvents: [DiskActivityEvent],
        coalescingWindow: TimeInterval = 0
    ) {
        for event in newEvents {
            guard coalescingWindow > 0,
                  let index = events.lastIndex(where: { existing in
                      existing.rootPath.standardizedFileURL == event.rootPath.standardizedFileURL &&
                          existing.path.standardizedFileURL == event.path.standardizedFileURL &&
                          event.timestamp.timeIntervalSince(existing.timestamp) <= coalescingWindow &&
                          event.timestamp >= existing.timestamp
                  }),
                  let mergedEvent = Self.merge(events[index], with: event) else {
                events.append(event)
                continue
            }
            events[index] = mergedEvent
        }
        events.sort { lhs, rhs in
            if lhs.timestamp == rhs.timestamp {
                return lhs.path.path < rhs.path.path
            }
            return lhs.timestamp < rhs.timestamp
        }
        let overflow = events.count - Self.maxRetainedEvents
        if overflow > 0 {
            events.removeFirst(overflow)
            droppedEventCount += overflow
        }
    }

    private static func merge(
        _ existing: DiskActivityEvent,
        with incoming: DiskActivityEvent
    ) -> DiskActivityEvent? {
        let kind: DiskActivityEventKind
        switch (existing.kind, incoming.kind) {
        case (.created, .created), (.created, .modified):
            kind = .created
        case (.modified, .modified):
            kind = .modified
        default:
            return nil
        }

        return DiskActivityEvent(
            kind: kind,
            path: incoming.path,
            rootPath: incoming.rootPath,
            timestamp: incoming.timestamp,
            byteDelta: incoming.byteDelta ?? existing.byteDelta,
            confidence: incoming.byteDelta == nil ? existing.confidence : incoming.confidence,
            previousPath: incoming.previousPath ?? existing.previousPath,
            affectedItemCount: max(existing.affectedItemCount, incoming.affectedItemCount),
            processName: incoming.processName ?? existing.processName
        )
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
