import Foundation

/// What to call a watch root. `/` has no last path component, so the old
/// `lastPathComponent` fallback showed the raw path as both title and subtitle.
nonisolated enum WatchRootNaming {
    nonisolated static func displayName(for url: URL) -> String {
        let standardized = url.standardizedFileURL
        if standardized.path == "/" {
            return "Whole Disk"
        }
        let name = standardized.lastPathComponent
        return name.isEmpty ? standardized.path : name
    }
}

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
        /// Set when the folder contains links whose bytes can change from
        /// outside it. Nothing is wrong with the watch; it just cannot see them.
        let unobservableLinkNote: String?
        let isEnabled: Bool
        let growthAlertThresholdBytes: Int64?
        let exclusionPatterns: [String]
        let minimumFileBytes: Int64?
        let maximumFileBytes: Int64?
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

    struct RecentDeletion: Equatable, Identifiable, Sendable {
        let id: String
        let title: String
        let path: String
        let sizeText: String
        let timestampText: String
        let isInTrash: Bool
    }

    let title: String
    let summaryText: String
    let emptyStateTitle: String?
    let selectedTargetID: String?
    let targetRows: [TargetRow]
    let trendBuckets: [ActivityHistoryPresentation.Bucket]
    let timelineRows: [ActivityHistoryPresentation.Row]
    let topChanges: [TopChange]
    let recentDeletions: [RecentDeletion]

    init(
        targets: [LongTermWatchTarget],
        selectedRootPath: URL?,
        histories: [ActivityHistorySnapshot],
        runtimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus] = [:],
        eventLimit: Int = 16,
        bucketLimit: Int = 32,
        deletionLimit: Int = 6,
        trashContains: (String) -> Bool = TrashLocator.contains(itemNamed:)
    ) {
        title = "Activity"
        emptyStateTitle = targets.isEmpty ? "No Long-Term Watches" : nil

        // A folder that vanished is still enabled but is not being watched;
        // counting it as active is how the old summary hid the failure.
        let activeCount = targets.filter { target in
            target.isEnabled && runtimeStatuses[target.id]?.state != .rootMissing
        }.count
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
            recentDeletions = Self.recentDeletions(history: history, limit: deletionLimit, trashContains: trashContains)
        } else {
            trendBuckets = []
            timelineRows = []
            topChanges = []
            recentDeletions = []
        }
    }

    /// Largest recent removals with a hint about where they went.
    private static func recentDeletions(
        history: ActivityHistorySnapshot,
        limit: Int,
        trashContains: (String) -> Bool
    ) -> [RecentDeletion] {
        let root = history.rootPath.standardizedFileURL.path
        return history.recentEvents
            // Unmounting a volume or renaming the watch root reports the root
            // path as removed. That is a watch ending, not a file the user lost.
            .filter { $0.path.standardizedFileURL.path != root }
            .filter { $0.kind == .deleted || ($0.kind == .moved && ($0.byteDelta ?? 0) < 0) }
            .sorted { lhs, rhs in
                let lhsBytes = abs(lhs.byteDelta ?? 0)
                let rhsBytes = abs(rhs.byteDelta ?? 0)
                if lhsBytes == rhsBytes {
                    return lhs.timestamp > rhs.timestamp
                }
                return lhsBytes > rhsBytes
            }
            .prefix(limit)
            .map { event in
                RecentDeletion(
                    id: "\(event.timestamp.timeIntervalSince1970)|\(event.path.path)",
                    title: event.path.lastPathComponent,
                    path: event.path.path,
                    sizeText: event.byteDelta.map { PathlightFormatters.size(abs($0)) } ?? "Unknown size",
                    timestampText: PathlightFormatters.date(event.timestamp),
                    isInTrash: trashContains(event.path.lastPathComponent)
                )
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

    /// FSEvents reports paths, not inodes. A write through a name outside the
    /// watched folder changes the folder's contents with no event under it, so
    /// the honest answer is to say how many such names exist.
    private static func unobservableLinkNote(for baseline: ActivityBaselineSnapshot?) -> String? {
        guard let count = baseline?.unobservableLinkCount, count > 0 else { return nil }
        let itemLabel = count == 1 ? "item" : "items"
        return "\(count.formatted()) linked \(itemLabel) can change from outside this folder without being recorded"
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
            thresholdText: thresholdText(for: target.options),
            lastActivityText: lastActivityText(for: status, history: history),
            hasHistoryGap: status.state == .historyGap,
            unobservableLinkNote: unobservableLinkNote(for: target.baseline),
            isEnabled: target.isEnabled,
            growthAlertThresholdBytes: target.options.growthAlertThresholdBytes,
            exclusionPatterns: target.options.exclusionPatterns,
            minimumFileBytes: target.options.minimumFileBytes,
            maximumFileBytes: target.options.maximumFileBytes,
            rootPath: target.rootPath
        )
    }

    /// What this watch records, in the two ways it can be limited: how much of
    /// a file has to change, and how big the file itself may be.
    static func thresholdText(for options: LongTermWatchTargetOptions) -> String {
        let changes = "Records changes over \(PathlightFormatters.size(options.minimumRecordedByteDelta))"
        switch (options.minimumFileBytes, options.maximumFileBytes) {
        case (nil, nil):
            return changes
        case let (minimum?, maximum?):
            return "\(changes) · files \(PathlightFormatters.size(minimum))–\(PathlightFormatters.size(maximum))"
        case let (minimum?, nil):
            return "\(changes) · files over \(PathlightFormatters.size(minimum))"
        case let (nil, maximum?):
            return "\(changes) · files under \(PathlightFormatters.size(maximum))"
        }
    }

    static func displayName(for url: URL) -> String {
        WatchRootNaming.displayName(for: url)
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
        case .rootMissing:
            return "Folder Missing"
        case .paused:
            return "Paused"
        }
    }

    /// After a relaunch nothing has been observed yet, so the recorded history
    /// is the only thing that knows when this folder last changed.
    private static func lastActivityText(
        for status: LongTermWatchRuntimeStatus,
        history: ActivityHistorySnapshot?
    ) -> String {
        if status.state == .rootMissing {
            return "Folder is missing — nothing can be recorded"
        }
        guard let lastActivityAt = status.lastActivityAt ?? history?.recentEvents.first?.timestamp else {
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
    nonisolated func netByteDelta(onOrAfter cutoff: Date) -> Int64 {
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
    /// Compact "+1.2 GB" for the menu bar label; empty when nothing is monitored.
    let todayTotalText: String

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
            return Row(
                id: target.id,
                title: ActivityDashboardPresentation.displayName(for: target.rootPath),
                statusText: ActivityDashboardPresentation.statusText(for: status),
                todayChangeText: "\(ActivityDashboardPresentation.signedSize(todayDelta)) today",
                isEnabled: target.isEnabled,
                rootPath: target.rootPath
            )
        }

        if targets.isEmpty {
            summaryText = "No folders monitored"
            todayTotalText = ""
        } else {
            let watchingCount = targets.filter(\.isEnabled).count
            summaryText = "\(watchingCount.formatted()) watching • \(ActivityDashboardPresentation.signedSize(todayTotal)) today"
            todayTotalText = ActivityDashboardPresentation.signedSize(todayTotal)
        }
    }
}
