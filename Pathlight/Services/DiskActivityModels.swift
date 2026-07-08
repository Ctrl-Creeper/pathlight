import Foundation

struct DiskActivityChange: Equatable, Sendable {
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

enum DiskActivityEventKind: Equatable, Sendable {
    case created
    case modified
    case deleted
    case moved
    case aggregate
}

enum DiskActivityEventConfidence: Equatable, Sendable {
    case confirmed
    case estimated
    case unknown
}

struct DiskActivityEvent: Equatable, Sendable {
    let kind: DiskActivityEventKind
    let path: URL
    let rootPath: URL
    let timestamp: Date
    let byteDelta: Int64?
    let confidence: DiskActivityEventConfidence
    let previousPath: URL?
    let affectedItemCount: Int
}

struct DiskActivityAggregationOptions: Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let longTermRecordsFileNames: Bool

    static let `default` = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 10 * 1_024 * 1_024,
        aggregationWindow: 5 * 60,
        longTermRecordsFileNames: false
    )
}
