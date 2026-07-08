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
