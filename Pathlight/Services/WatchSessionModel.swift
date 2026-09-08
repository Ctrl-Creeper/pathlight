import Foundation

nonisolated enum WatchSessionHistoryState: Equatable, Sendable {
    case live
    case catchingUp
    case gapDetected
}

nonisolated struct WatchSessionModel: Equatable, Sendable {
    /// An event plus a session-local identity that survives coalescing, so a
    /// consumer can persist the final merged version exactly once.
    struct ChangedEvent: Equatable, Sendable {
        let id: UUID
        let event: DiskActivityEvent
    }

    let id: UUID
    let rootPath: URL
    let startedAt: Date
    private(set) var events: [DiskActivityEvent]
    private var eventIDs: [UUID]
    private(set) var lastObservedEventID: UInt64?
    private(set) var receivedStreamEventCount: UInt64 = 0
    private(set) var historyGapCount: UInt64 = 0
    private(set) var historyState: WatchSessionHistoryState
    /// Oldest events dropped once `maxRetainedEvents` is exceeded, so a storm
    /// cannot grow memory without bound.
    private(set) var droppedEventCount = 0
    /// Events appended or merged by the most recent `append`; cleared by the
    /// coordinator before each stream event so consumers only see real changes.
    private(set) var latestChanges: [ChangedEvent] = []

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
        self.eventIDs = events.map { _ in UUID() }
        self.lastObservedEventID = lastObservedEventID
        self.historyState = historyState
    }

    mutating func append(
        _ newEvents: [DiskActivityEvent],
        coalescingWindow: TimeInterval = 0
    ) {
        var changed: [UUID: DiskActivityEvent] = [:]
        var order: [UUID] = []
        for event in newEvents {
            if coalescingWindow > 0,
               let index = events.lastIndex(where: { existing in
                   existing.rootPath.standardizedFileURL == event.rootPath.standardizedFileURL &&
                       existing.path.standardizedFileURL == event.path.standardizedFileURL &&
                       event.timestamp.timeIntervalSince(existing.timestamp) <= coalescingWindow &&
                       event.timestamp >= existing.timestamp
               }),
               let mergedEvent = Self.merge(events[index], with: event) {
                events[index] = mergedEvent
                let id = eventIDs[index]
                if changed[id] == nil {
                    order.append(id)
                }
                changed[id] = mergedEvent
            } else {
                let id = UUID()
                events.append(event)
                eventIDs.append(id)
                order.append(id)
                changed[id] = event
            }
        }

        var pairs = Array(zip(events, eventIDs))
        pairs.sort { lhs, rhs in
            if lhs.0.timestamp == rhs.0.timestamp {
                return lhs.0.path.path < rhs.0.path.path
            }
            return lhs.0.timestamp < rhs.0.timestamp
        }
        let overflow = pairs.count - Self.maxRetainedEvents
        if overflow > 0 {
            pairs.removeFirst(overflow)
            droppedEventCount += overflow
        }
        events = pairs.map(\.0)
        eventIDs = pairs.map(\.1)
        latestChanges = order.compactMap { id in
            changed[id].map { ChangedEvent(id: id, event: $0) }
        }
    }

    mutating func recordStreamEvent(isGap: Bool) {
        receivedStreamEventCount &+= 1
        if isGap { historyGapCount &+= 1 }
    }

    mutating func clearLatestChanges() {
        latestChanges = []
    }

    /// Deltas are incremental (a modification reports growth since the last
    /// known size), so a merged burst sums them; an unknown side poisons the sum.
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

        let byteDelta: Int64?
        if let existingDelta = existing.byteDelta, let incomingDelta = incoming.byteDelta {
            byteDelta = existingDelta + incomingDelta
        } else {
            byteDelta = nil
        }
        let confidence: DiskActivityEventConfidence
        if byteDelta == nil {
            confidence = .unknown
        } else if existing.confidence == .confirmed, incoming.confidence == .confirmed {
            confidence = .confirmed
        } else {
            confidence = .estimated
        }

        return DiskActivityEvent(
            kind: kind,
            path: incoming.path,
            rootPath: incoming.rootPath,
            timestamp: incoming.timestamp,
            byteDelta: byteDelta,
            confidence: confidence,
            previousPath: incoming.previousPath ?? existing.previousPath,
            affectedItemCount: max(existing.affectedItemCount, incoming.affectedItemCount),
            processName: incoming.processName ?? existing.processName
        )
    }

    mutating func record(eventID: UInt64) {
        // Zero is the fresh-start readiness marker, never a resume cursor. A
        // real FSEvents or Rust event ID is nonzero, so persisting zero as a
        // checkpoint would make the next launch replay the whole event history
        // instead of resuming. Leaving the cursor nil resumes from now.
        guard eventID != 0 else { return }
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
