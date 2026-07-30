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
        let lastActivityText: String
        let hasHistoryGap: Bool
        let isEnabled: Bool
        let growthAlertThresholdBytes: Int64?
        let rootPath: URL
    }

    struct TopChange: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let changeText: String
        let eventText: String
        let byteDelta: Int64
        let magnitudeFraction: Double
        let url: URL
    }

    let title: String
    let summaryText: String
    let emptyStateTitle: String?
    let selectedTargetID: String?
    let targetRows: [TargetRow]
    let trendBuckets: [ActivityHistoryPresentation.Bucket]
    let timelineRows: [ActivityHistoryPresentation.Row]
    let topChanges: [TopChange]

    init(
        targets: [LongTermWatchTarget],
        selectedRootPath: URL?,
        histories: [ActivityHistorySnapshot],
        runtimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus] = [:],
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
        let historyByRoot = histories.reduce(into: [String: ActivityHistorySnapshot]()) { historiesByRoot, history in
            historiesByRoot[history.rootPath.standardizedFileURL.path] = history
        }
        targetRows = targets.map { target in
            let targetHistory = historyByRoot[target.rootPath.standardizedFileURL.path]
            return Self.targetRow(
                for: target,
                history: targetHistory,
                runtimeStatus: runtimeStatuses[target.id]
            )
        }

        if let standardizedSelectedRoot {
            selectedTargetID = standardizedSelectedRoot.path
        } else {
            selectedTargetID = targets.first?.id
        }

        if let selectedTargetID,
           let history = historyByRoot[selectedTargetID] {
            let historyPresentation = ActivityHistoryPresentation(
                snapshot: history,
                eventLimit: eventLimit,
                bucketLimit: bucketLimit
            )
            trendBuckets = historyPresentation.buckets
            timelineRows = historyPresentation.rows
            topChanges = Self.topChanges(history: history)
        } else {
            trendBuckets = []
            timelineRows = []
            topChanges = []
        }
    }

    /// Aggregates recent events by the immediate child of the watch root, so the
    /// dashboard can answer "what inside this folder is growing".
    private static func topChanges(
        history: ActivityHistorySnapshot,
        limit: Int = 6
    ) -> [TopChange] {
        struct Accumulator {
            var byteDelta: Int64 = 0
            var eventCount = 0
        }

        let root = history.rootPath.standardizedFileURL
        let rootPrefix = root.path.hasSuffix("/") ? root.path : root.path + "/"
        var accumulators: [String: Accumulator] = [:]
        for event in history.recentEvents where event.kind != .aggregate {
            let path = event.path.standardizedFileURL.path
            guard path.hasPrefix(rootPrefix) else {
                continue
            }
            let relative = path.dropFirst(rootPrefix.count)
            guard let childName = relative.split(separator: "/").first.map(String.init) else {
                continue
            }
            accumulators[childName, default: Accumulator()].byteDelta += event.byteDelta ?? 0
            accumulators[childName, default: Accumulator()].eventCount += 1
        }

        let maxMagnitude = accumulators.values.map { abs($0.byteDelta) }.max() ?? 0
        return accumulators
            .sorted { lhs, rhs in
                if abs(lhs.value.byteDelta) == abs(rhs.value.byteDelta) {
                    return lhs.key < rhs.key
                }
                return abs(lhs.value.byteDelta) > abs(rhs.value.byteDelta)
            }
            .prefix(limit)
            .map { childName, accumulator in
                TopChange(
                    id: childName,
                    title: childName,
                    changeText: signedSize(accumulator.byteDelta),
                    eventText: eventCountText(accumulator.eventCount),
                    byteDelta: accumulator.byteDelta,
                    magnitudeFraction: maxMagnitude > 0
                        ? Double(abs(accumulator.byteDelta)) / Double(maxMagnitude)
                        : 0,
                    url: root.appending(path: childName)
                )
            }
    }

    private static func targetRow(
        for target: LongTermWatchTarget,
        history: ActivityHistorySnapshot?,
        runtimeStatus: LongTermWatchRuntimeStatus?
    ) -> TargetRow {
        let status = runtimeStatus ?? (target.isEnabled
            ? LongTermWatchRuntimeStatus(state: .starting, lastActivityAt: nil, retryCount: 0)
            : .paused)
        return TargetRow(
            id: target.id,
            title: displayName(for: target.rootPath),
            subtitle: target.rootPath.path,
            statusText: statusText(for: status),
            changeText: history.map { "\(signedSize($0.totalNetByteDelta)) net" } ?? "No history",
            eventText: history.map { eventCountText($0.eventCount) } ?? "0 events",
            thresholdText: "Records changes over \(PathlightFormatters.size(target.options.minimumRecordedByteDelta))",
            lastActivityText: lastActivityText(for: status),
            hasHistoryGap: status.state == .historyGap,
            isEnabled: target.isEnabled,
            growthAlertThresholdBytes: target.options.growthAlertThresholdBytes,
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

    static func statusText(for status: LongTermWatchRuntimeStatus) -> String {
        switch status.state {
        case .starting:
            return "Starting"
        case .watching:
            return "Watching"
        case .reconnecting:
            return "Reconnecting"
        case .catchingUp:
            return "Catching Up"
        case .historyGap:
            return "History Gap"
        case .paused:
            return "Paused"
        }
    }

    private static func lastActivityText(for status: LongTermWatchRuntimeStatus) -> String {
        guard let lastActivityAt = status.lastActivityAt else {
            return status.state == .paused ? "Monitoring paused" : "No activity yet"
        }
        return "Last activity \(PathlightFormatters.date(lastActivityAt))"
    }

    static func signedSize(_ bytes: Int64) -> String {
        if bytes > 0 {
            return "+\(PathlightFormatters.size(bytes))"
        }
        if bytes < 0 {
            return "-\(PathlightFormatters.size(abs(bytes)))"
        }
        return PathlightFormatters.size(0)
    }
}

extension ActivityHistorySnapshot {
    /// Net byte change across buckets that end after the cutoff (e.g. start of today).
    func netByteDelta(onOrAfter cutoff: Date) -> Int64 {
        buckets
            .filter { $0.endDate > cutoff }
            .reduce(Int64(0)) { $0 + $1.byteDelta }
    }
}

struct MenuBarActivityPresentation: Equatable, Sendable {
    struct Row: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let statusText: String
        let todayChangeText: String
        let isEnabled: Bool
        let rootPath: URL
    }

    let rows: [Row]
    let summaryText: String

    init(
        targets: [LongTermWatchTarget],
        histories: [ActivityHistorySnapshot],
        runtimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus] = [:],
        now: Date = Date(),
        calendar: Calendar = .current
    ) {
        let dayStart = calendar.startOfDay(for: now)
        let historyByRoot = histories.reduce(into: [String: ActivityHistorySnapshot]()) { result, history in
            result[history.rootPath.standardizedFileURL.path] = history
        }

        var todayTotal: Int64 = 0
        rows = targets.map { target in
            let todayDelta = historyByRoot[target.id]?.netByteDelta(onOrAfter: dayStart) ?? 0
            todayTotal += todayDelta
            let status = runtimeStatuses[target.id] ?? (target.isEnabled
                ? LongTermWatchRuntimeStatus(state: .starting, lastActivityAt: nil, retryCount: 0)
                : .paused)
            let name = target.rootPath.lastPathComponent
            return Row(
                id: target.id,
                title: name.isEmpty ? target.rootPath.path : name,
                statusText: ActivityDashboardPresentation.statusText(for: status),
                todayChangeText: "\(ActivityDashboardPresentation.signedSize(todayDelta)) today",
                isEnabled: target.isEnabled,
                rootPath: target.rootPath
            )
        }

        if targets.isEmpty {
            summaryText = "No folders monitored"
        } else {
            let watchingCount = targets.filter(\.isEnabled).count
            summaryText = "\(watchingCount.formatted()) watching • \(ActivityDashboardPresentation.signedSize(todayTotal)) today"
        }
    }
}
