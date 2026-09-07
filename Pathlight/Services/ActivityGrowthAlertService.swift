import Foundation
import UserNotifications

nonisolated protocol ActivityGrowthAlertPosting: Sendable {
    func postGrowthAlert(rootPath: URL, growthBytes: Int64, thresholdBytes: Int64) async
    func postActivityAlert(rootPath: URL, identifier: String, title: String, body: String) async
}

extension ActivityGrowthAlertPosting {
    func postActivityAlert(rootPath: URL, identifier: String, title: String, body: String) async {}
}

nonisolated struct ActivityAnomaly: Equatable, Sendable {
    enum Kind: String, Sendable {
        case massDeletion
        case burst
    }

    let kind: Kind
    let rootPath: URL
    let byteDelta: Int64
    let itemCount: Int

    var identifier: String {
        "pathlight-anomaly-\(kind.rawValue)-\(rootPath.standardizedFileURL.path)"
    }

    var title: String {
        let name = rootPath.lastPathComponent.isEmpty ? rootPath.path : rootPath.lastPathComponent
        switch kind {
        case .massDeletion:
            return "Many files deleted in \(name)"
        case .burst:
            return "\(name) is filling up fast"
        }
    }

    var body: String {
        let minutes = Int(ActivityAnomalyDetector.window / 60)
        switch kind {
        case .massDeletion:
            return "\(itemCount.formatted()) items (\(PathlightFormatters.size(abs(byteDelta)))) removed in the last \(minutes) minutes."
        case .burst:
            return "\(PathlightFormatters.size(byteDelta)) written in the last \(minutes) minutes."
        }
    }
}

/// Short-window anomalies on top of the daily growth threshold: a mass deletion
/// (mistake or ransomware) and a write burst.
enum ActivityAnomalyDetector {
    nonisolated static let window: TimeInterval = 10 * 60
    nonisolated static let massDeletionMinimumItems = 100
    nonisolated static let massDeletionMinimumBytes: Int64 = 1_000_000_000
    nonisolated static let burstMinimumBytes: Int64 = 5_000_000_000

    nonisolated static func anomalies(
        in history: ActivityHistorySnapshot,
        now: Date = Date()
    ) -> [ActivityAnomaly] {
        let cutoff = now.addingTimeInterval(-window)
        let recent = history.recentEvents.filter { $0.timestamp >= cutoff && $0.timestamp <= now }
        guard !recent.isEmpty else {
            return []
        }
        var anomalies: [ActivityAnomaly] = []

        let removals = recent.filter { $0.kind == .deleted || ($0.byteDelta ?? 0) < 0 }
        let removedBytes = removals.compactMap(\.byteDelta).filter { $0 < 0 }.reduce(Int64(0), +)
        let removedItems = removals.reduce(0) { $0 + $1.affectedItemCount }
        if removedItems >= massDeletionMinimumItems || -removedBytes >= massDeletionMinimumBytes {
            anomalies.append(ActivityAnomaly(kind: .massDeletion, rootPath: history.rootPath, byteDelta: removedBytes, itemCount: removedItems))
        }

        let written = recent.compactMap(\.byteDelta).filter { $0 > 0 }.reduce(Int64(0), +)
        if written >= burstMinimumBytes {
            let items = recent.filter { ($0.byteDelta ?? 0) > 0 }.reduce(0) { $0 + $1.affectedItemCount }
            anomalies.append(ActivityAnomaly(kind: .burst, rootPath: history.rootPath, byteDelta: written, itemCount: items))
        }
        return anomalies
    }
}

enum ActivityGrowthAlertEvaluator {
    /// Returns today's growth when it crosses the target's alert threshold and no
    /// alert was posted yet today; nil means stay quiet.
    nonisolated static func alertworthyGrowth(
        target: LongTermWatchTarget,
        history: ActivityHistorySnapshot,
        lastAlertedAt: Date?,
        now: Date = Date(),
        calendar: Calendar = .current
    ) -> Int64? {
        guard target.isEnabled,
              let threshold = target.options.growthAlertThresholdBytes,
              threshold > 0 else {
            return nil
        }

        let dayStart = calendar.startOfDay(for: now)
        if let lastAlertedAt, lastAlertedAt >= dayStart {
            return nil
        }

        let growth = history.netByteDelta(onOrAfter: dayStart)
        return growth >= threshold ? growth : nil
    }
}

@MainActor
final class UserNotificationGrowthAlertPoster: ActivityGrowthAlertPosting, Sendable {
    func postGrowthAlert(rootPath: URL, growthBytes: Int64, thresholdBytes: Int64) async {
        let center = UNUserNotificationCenter.current()
        let granted = (try? await center.requestAuthorization(options: [.alert, .badge])) ?? false
        guard granted else {
            return
        }

        let name = rootPath.lastPathComponent.isEmpty ? rootPath.path : rootPath.lastPathComponent
        let (growthText, thresholdText) = await MainActor.run {
            (PathlightFormatters.size(growthBytes), PathlightFormatters.size(thresholdBytes))
        }
        let content = UNMutableNotificationContent()
        content.title = "\(name) is growing quickly"
        content.body = "Up \(growthText) today — past your \(thresholdText) alert threshold."
        let request = UNNotificationRequest(
            identifier: "pathlight-growth-\(rootPath.standardizedFileURL.path)",
            content: content,
            trigger: nil
        )
        try? await center.add(request)
    }

    func postActivityAlert(rootPath: URL, identifier: String, title: String, body: String) async {
        let center = UNUserNotificationCenter.current()
        let granted = (try? await center.requestAuthorization(options: [.alert, .badge])) ?? false
        guard granted else {
            return
        }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        try? await center.add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil))
    }
}
