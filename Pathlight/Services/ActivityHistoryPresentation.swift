import Foundation

struct ActivityHistoryPresentation: Equatable, Sendable {
    struct Bucket: Equatable, Identifiable, Sendable {
        let id: String
        let label: String
        let detail: String
        let byteDelta: Int64
        let eventCount: Int
        let magnitudeFraction: Double
    }

    struct Row: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let detail: String
        let path: String
        let timestamp: Date
    }

    let title: String
    let summaryText: String
    let buckets: [Bucket]
    let rows: [Row]

    init(
        snapshot: ActivityHistorySnapshot,
        eventLimit: Int = 8,
        bucketLimit: Int = 24
    ) {
        title = "History for \(snapshot.rootPath.lastPathComponent.isEmpty ? snapshot.rootPath.path : snapshot.rootPath.lastPathComponent)"

        let eventLabel = snapshot.eventCount == 1 ? "event" : "events"
        let sizeSummary: String
        if snapshot.eventCount == snapshot.unknownSizeEventCount, snapshot.eventCount > 0 {
            sizeSummary = "unknown size"
        } else {
            sizeSummary = "\(Self.signedSize(snapshot.totalNetByteDelta)) net"
        }
        summaryText = "\(snapshot.eventCount.formatted()) \(eventLabel) • \(sizeSummary)"

        let visibleBuckets = Array(snapshot.buckets.suffix(bucketLimit))
        let maxMagnitude = visibleBuckets.map { abs($0.byteDelta) }.max() ?? 0
        buckets = visibleBuckets.map { bucket in
            Self.presentationBucket(for: bucket, maxMagnitude: maxMagnitude)
        }

        rows = snapshot.recentEvents
            .sorted { lhs, rhs in
                if lhs.timestamp == rhs.timestamp {
                    return lhs.path.path > rhs.path.path
                }
                return lhs.timestamp > rhs.timestamp
            }
            .prefix(eventLimit)
            .map(Self.row)
    }

    private static func presentationBucket(
        for bucket: ActivityHistoryBucket,
        maxMagnitude: Int64
    ) -> Bucket {
        let fraction: Double
        if maxMagnitude > 0 {
            fraction = Double(abs(bucket.byteDelta)) / Double(maxMagnitude)
        } else if bucket.eventCount > 0 {
            fraction = 1
        } else {
            fraction = 0
        }

        return Bucket(
            id: [
                bucket.startDate.timeIntervalSince1970.description,
                bucket.endDate.timeIntervalSince1970.description
            ].joined(separator: "|"),
            label: PathlightFormatters.date(bucket.startDate),
            detail: bucket.detailText,
            byteDelta: bucket.byteDelta,
            eventCount: bucket.eventCount,
            magnitudeFraction: fraction
        )
    }

    private static func row(for event: DiskActivityEvent) -> Row {
        Row(
            id: [
                event.timestamp.timeIntervalSince1970.description,
                event.kindDescription,
                event.path.path
            ].joined(separator: "|"),
            title: "\(event.kindTitle) \(event.path.lastPathComponent)",
            detail: event.byteDelta.map(signedSize) ?? "Unknown size",
            path: event.path.path,
            timestamp: event.timestamp
        )
    }

    private static func signedSize(_ bytes: Int64) -> String {
        if bytes > 0 {
            return "+\(PathlightFormatters.size(bytes))"
        }
        if bytes < 0 {
            return "-\(PathlightFormatters.size(abs(bytes)))"
        }
        return PathlightFormatters.size(0)
    }
}

private extension ActivityHistoryBucket {
    var detailText: String {
        if eventCount == unknownSizeEventCount, eventCount > 0 {
            return "Unknown size"
        }
        if byteDelta > 0 {
            return "+\(PathlightFormatters.size(byteDelta))"
        }
        if byteDelta < 0 {
            return "-\(PathlightFormatters.size(abs(byteDelta)))"
        }
        return PathlightFormatters.size(0)
    }
}

private extension DiskActivityEvent {
    var kindTitle: String {
        switch kind {
        case .created:
            return "Created"
        case .modified:
            return "Modified"
        case .deleted:
            return "Deleted"
        case .moved:
            return "Moved"
        case .aggregate:
            return "Changed"
        }
    }

    var kindDescription: String {
        switch kind {
        case .created:
            return "created"
        case .modified:
            return "modified"
        case .deleted:
            return "deleted"
        case .moved:
            return "moved"
        case .aggregate:
            return "aggregate"
        }
    }
}
