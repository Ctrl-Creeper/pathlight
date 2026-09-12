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

nonisolated enum DiskActivityEventKind: Equatable, Codable, Sendable, CaseIterable {
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
    /// Smallest and largest file this watch records at all; nil is no bound.
    /// The file's own size, not how much of it changed — a file outside these
    /// bounds is not watched, however much of it is rewritten.
    var minimumFileBytes: Int64?
    var maximumFileBytes: Int64?

    /// Names on by default: the byte threshold already keeps the journal small,
    /// and "which file grew" is the answer the dashboard exists to give. Turning
    /// names off trades that answer for directory-level rows.
    static let `default` = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 1_024,
        aggregationWindow: 5 * 60,
        longTermRecordsFileNames: true
    )

    static let shortTermDefault = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 1_024,
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

/// The two choices made before a watch opens. Attribution owns the byte
/// threshold; the native monitor owns delivery latency.
nonisolated struct MonitoringStartConfiguration: Equatable, Sendable {
    static let maximumMinimumKilobytes: Int64 = 1_000_000

    let minimumRecordedByteDelta: Int64
    let monitorLatency: TimeInterval

    static let `default` = MonitoringStartConfiguration(
        minimumRecordedByteDelta: 1_024,
        monitorLatency: 5
    )

    init(minimumRecordedByteDelta: Int64, monitorLatency: TimeInterval) {
        self.minimumRecordedByteDelta = max(minimumRecordedByteDelta, 0)
        self.monitorLatency = max(monitorLatency, 0.25)
    }

    init(minimumKilobytes: Int64, monitorLatency: TimeInterval) {
        let boundedKilobytes = min(
            max(minimumKilobytes, 0),
            Self.maximumMinimumKilobytes
        )
        self.init(
            minimumRecordedByteDelta: boundedKilobytes * 1_024,
            monitorLatency: monitorLatency
        )
    }

    var liveOptions: DiskActivityAggregationOptions {
        .shortTerm(minimumRecordedByteDelta: minimumRecordedByteDelta)
    }

    func longTermOptions(
        basedOn base: LongTermWatchTargetOptions = .default
    ) -> LongTermWatchTargetOptions {
        LongTermWatchTargetOptions(
            minimumRecordedByteDelta: minimumRecordedByteDelta,
            aggregationWindow: base.aggregationWindow,
            recordsFileNames: base.recordsFileNames,
            monitorLatency: monitorLatency,
            growthAlertThresholdBytes: base.growthAlertThresholdBytes,
            exclusionPatterns: base.exclusionPatterns,
            minimumFileBytes: base.minimumFileBytes,
            maximumFileBytes: base.maximumFileBytes
        )
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
