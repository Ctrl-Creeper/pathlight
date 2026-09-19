import Foundation

struct ActivityHistoryPresentation: Equatable, Sendable {
    struct Bucket: Equatable, Identifiable, Sendable {
        let id: String
        let label: String
        let detail: String
        let byteDelta: Int64
        let eventCount: Int
        let magnitudeFraction: Double
        let startDate: Date
        let endDate: Date
    }

    struct Row: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let detail: String
        let path: String
        let timestamp: Date
        let kind: DiskActivityEventKind
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
        // Totals cover all retained history; the lists below only see the
        // newest page, so say which is which instead of implying both.
        let truncationNote = snapshot.isTruncated
            ? " • newest \(snapshot.recentEvents.count.formatted()) listed"
            : ""
        summaryText = "\(snapshot.eventCount.formatted()) \(eventLabel) • \(sizeSummary)\(truncationNote)"

        let visibleBuckets = Array(Self.merged(snapshot.buckets, limit: bucketLimit).suffix(bucketLimit))
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

    /// The store totals every five minutes; the chart shows the coarsest step
    /// that still fits the whole span in `limit` bars, so two hours get
    /// five-minute bars and a month gets days instead of a few thin bars.
    static func merged(_ buckets: [ActivityHistoryBucket], limit: Int) -> [ActivityHistoryBucket] {
        guard let first = buckets.first, let last = buckets.last else { return [] }
        let span = last.endDate.timeIntervalSince(first.startDate)
        let steps: [TimeInterval] = [300, 900, 1_800, 3_600, 3 * 3_600, 6 * 3_600, 12 * 3_600, 86_400, 7 * 86_400]
        let step = steps.first { span <= $0 * Double(max(limit, 1)) } ?? steps[steps.count - 1]
        var merged: [Date: ActivityHistoryBucket] = [:]
        for bucket in buckets {
            let start = ActivityHistoryService.bucketStart(for: bucket.startDate, interval: step)
            let sum = merged[start]
            merged[start] = ActivityHistoryBucket(
                startDate: start,
                endDate: start.addingTimeInterval(step),
                byteDelta: (sum?.byteDelta ?? 0) + bucket.byteDelta,
                eventCount: (sum?.eventCount ?? 0) + bucket.eventCount,
                unknownSizeEventCount: (sum?.unknownSizeEventCount ?? 0) + bucket.unknownSizeEventCount
            )
        }
        return merged.values.sorted { $0.startDate < $1.startDate }
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
            magnitudeFraction: fraction,
            startDate: bucket.startDate,
            endDate: bucket.endDate
        )
    }

    private static func row(for event: DiskActivityEvent) -> Row {
        Row(
            id: [
                event.timestamp.timeIntervalSince1970.description,
                event.kindDescription,
                event.path.path
            ].joined(separator: "|"),
            title: event.rowTitle,
            detail: event.rowDetail(sizeText: event.byteDelta.map(signedSize) ?? "Unknown size"),
            path: event.path.path,
            timestamp: event.timestamp,
            kind: event.kind
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
    func rowDetail(sizeText: String) -> String {
        guard let processName, !processName.isEmpty else {
            return sizeText
        }
        return "\(sizeText) • \(processName)"
    }

    var rowTitle: String {
        if kind == .moved, let previousPath, previousPath.lastPathComponent != path.lastPathComponent {
            return "Moved \(previousPath.lastPathComponent) → \(path.lastPathComponent)"
        }
        return "\(kindTitle) \(path.lastPathComponent)"
    }

    var kindTitle: String { kind.title }

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

extension DiskActivityEventKind {
    /// The word one kind of change is named with, wherever a person picks or
    /// reads one. `core/src/text.rs` spells the same list for the other two
    /// hosts.
    var title: String {
        switch self {
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
}
