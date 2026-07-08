import Foundation

struct ActivityDashboardPresentation: Equatable, Sendable {
    struct TargetRow: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let subtitle: String
        let statusText: String
        let changeText: String
        let eventText: String
        let thresholdText: String
        let isEnabled: Bool
        let rootPath: URL
    }

    let title: String
    let summaryText: String
    let emptyStateTitle: String?
    let selectedTargetID: String?
    let targetRows: [TargetRow]
    let trendBuckets: [ActivityHistoryPresentation.Bucket]
    let timelineRows: [ActivityHistoryPresentation.Row]

    init(
        targets: [LongTermWatchTarget],
        selectedRootPath: URL?,
        history: ActivityHistorySnapshot?,
        eventLimit: Int = 16,
        bucketLimit: Int = 32
    ) {
        title = "Activity"
        emptyStateTitle = targets.isEmpty ? "No Long-Term Watches" : nil

        let activeCount = targets.filter(\.isEnabled).count
        if targets.isEmpty {
            summaryText = "No tracked folders"
        } else {
            let folderLabel = targets.count == 1 ? "tracked folder" : "tracked folders"
            summaryText = "\(targets.count.formatted()) \(folderLabel) • \(activeCount.formatted()) active"
        }

        let standardizedSelectedRoot = selectedRootPath?.standardizedFileURL
        let historyByRoot = history.map { [$0.rootPath.standardizedFileURL.path: $0] } ?? [:]
        targetRows = targets.map { target in
            let targetHistory = historyByRoot[target.rootPath.standardizedFileURL.path]
            return Self.targetRow(for: target, history: targetHistory)
        }

        if let standardizedSelectedRoot {
            selectedTargetID = standardizedSelectedRoot.path
        } else {
            selectedTargetID = targets.first?.id
        }

        if let history,
           selectedTargetID == history.rootPath.standardizedFileURL.path {
            let historyPresentation = ActivityHistoryPresentation(
                snapshot: history,
                eventLimit: eventLimit,
                bucketLimit: bucketLimit
            )
            trendBuckets = historyPresentation.buckets
            timelineRows = historyPresentation.rows
        } else {
            trendBuckets = []
            timelineRows = []
        }
    }

    private static func targetRow(
        for target: LongTermWatchTarget,
        history: ActivityHistorySnapshot?
    ) -> TargetRow {
        TargetRow(
            id: target.id,
            title: displayName(for: target.rootPath),
            subtitle: target.rootPath.path,
            statusText: target.isEnabled ? "Active" : "Paused",
            changeText: history.map { "\(signedSize($0.totalNetByteDelta)) net" } ?? "No history",
            eventText: history.map { eventCountText($0.eventCount) } ?? "0 events",
            thresholdText: "Records changes over \(PathlightFormatters.size(target.options.minimumRecordedByteDelta))",
            isEnabled: target.isEnabled,
            rootPath: target.rootPath
        )
    }

    private static func displayName(for url: URL) -> String {
        let name = url.lastPathComponent
        return name.isEmpty ? url.path : name
    }

    private static func eventCountText(_ count: Int) -> String {
        count == 1 ? "1 event" : "\(count.formatted()) events"
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
