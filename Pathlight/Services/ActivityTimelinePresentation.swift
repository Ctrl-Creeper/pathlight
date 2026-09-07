import Foundation

struct ActivityTimelinePresentation: Equatable, Sendable {
    struct Row: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let detail: String
        let path: String
        let timestamp: Date
        let timestampText: String
    }

    let title: String
    let summaryText: String
    let rows: [Row]

    init(session: WatchSessionModel, eventLimit: Int = 8) {
        title = "Watching \(session.rootPath.lastPathComponent.isEmpty ? session.rootPath.path : session.rootPath.lastPathComponent)"

        let summary = session.summary()
        let eventLabel = summary.eventCount == 1 ? "event" : "events"
        let sizeSummary: String
        if summary.eventCount == summary.unknownSizeEventCount, summary.eventCount > 0 {
            sizeSummary = "unknown size"
        } else {
            sizeSummary = "\(Self.signedSize(summary.netByteDelta)) net"
        }
        summaryText = "\(summary.eventCount.formatted()) \(eventLabel) • \(sizeSummary)"

        rows = session.events
            .sorted { lhs, rhs in
                if lhs.timestamp == rhs.timestamp {
                    return lhs.path.path > rhs.path.path
                }
                return lhs.timestamp > rhs.timestamp
            }
            .prefix(eventLimit)
            .map(Self.row)
    }

    private static func row(for event: DiskActivityEvent) -> Row {
        Row(
            id: [
                event.timestamp.timeIntervalSince1970.description,
                event.kindDescription,
                event.path.path
            ].joined(separator: "|"),
            title: event.rowTitle,
            detail: event.byteDelta.map(signedSize) ?? "Unknown size",
            path: event.path.path,
            timestamp: event.timestamp,
            timestampText: PathlightFormatters.date(event.timestamp)
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

private extension DiskActivityEvent {
    var rowTitle: String {
        if kind == .moved, let previousPath, previousPath.lastPathComponent != path.lastPathComponent {
            return "Moved \(previousPath.lastPathComponent) → \(path.lastPathComponent)"
        }
        return "\(kindTitle) \(path.lastPathComponent)"
    }

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
