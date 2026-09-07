import Foundation

nonisolated struct DiskActivityChange: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case created
        case modified
        case deleted
        case renamed(previousPath: URL?)
    }

    let kind: Kind
    let path: URL
    let rootPath: URL
    let timestamp: Date
}

nonisolated enum DiskActivityStreamEvent: Equatable, Sendable {
    case change(DiskActivityChange, eventID: UInt64)
    case historyCaughtUp(eventID: UInt64)
    case requiresRescan(eventID: UInt64)
}

nonisolated enum DiskActivityEventKind: Equatable, Codable, Sendable {
    case created
    case modified
    case deleted
    case moved
    case aggregate
}

nonisolated enum DiskActivityEventConfidence: Equatable, Codable, Sendable {
    case confirmed
    case estimated
    case unknown
}

nonisolated struct DiskActivityEvent: Equatable, Codable, Sendable {
    let kind: DiskActivityEventKind
    let path: URL
    let rootPath: URL
    let timestamp: Date
    let byteDelta: Int64?
    let confidence: DiskActivityEventConfidence
    let previousPath: URL?
    let affectedItemCount: Int
    /// Best-effort name of the process that had the path open when the change
    /// was observed; nil when unknown. Absent from rows written before this field existed.
    let processName: String?

    init(
        kind: DiskActivityEventKind,
        path: URL,
        rootPath: URL,
        timestamp: Date,
        byteDelta: Int64?,
        confidence: DiskActivityEventConfidence,
        previousPath: URL?,
        affectedItemCount: Int,
        processName: String? = nil
    ) {
        self.kind = kind
        self.path = path
        self.rootPath = rootPath
        self.timestamp = timestamp
        self.byteDelta = byteDelta
        self.confidence = confidence
        self.previousPath = previousPath
        self.affectedItemCount = affectedItemCount
        self.processName = processName
    }

    func withProcessName(_ processName: String?) -> DiskActivityEvent {
        DiskActivityEvent(
            kind: kind,
            path: path,
            rootPath: rootPath,
            timestamp: timestamp,
            byteDelta: byteDelta,
            confidence: confidence,
            previousPath: previousPath,
            affectedItemCount: affectedItemCount,
            processName: processName
        )
    }
}

nonisolated struct DiskActivityAggregationOptions: Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let longTermRecordsFileNames: Bool

    static let `default` = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 10 * 1_024 * 1_024,
        aggregationWindow: 5 * 60,
        longTermRecordsFileNames: false
    )

    static let shortTermDefault = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 0,
        aggregationWindow: 0,
        longTermRecordsFileNames: true
    )

    static func shortTerm(minimumRecordedByteDelta: Int64) -> DiskActivityAggregationOptions {
        DiskActivityAggregationOptions(
            minimumRecordedByteDelta: minimumRecordedByteDelta,
            aggregationWindow: 0,
            longTermRecordsFileNames: true
        )
    }

    var isDetailedFileTimeline: Bool {
        longTermRecordsFileNames && aggregationWindow == 0
    }
}

nonisolated struct WatchSessionSummary: Equatable, Sendable {
    let id: UUID
    let rootPath: URL
    let startedAt: Date
    let endedAt: Date
    let netByteDelta: Int64
    let eventCount: Int
    let unknownSizeEventCount: Int
}
