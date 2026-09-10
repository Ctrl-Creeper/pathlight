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
    /// Always positive: the size of what happened, not its direction.
    let bytes: Int64
    let itemCount: Int
    /// The span the finding was measured over, so the message can say it. The
    /// core decides how long that is.
    let windowMinutes: Int

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
        switch kind {
        case .massDeletion:
            return "\(itemCount.formatted()) items (\(PathlightFormatters.size(bytes))) removed in the last \(windowMinutes) minutes."
        case .burst:
            return "\(PathlightFormatters.size(bytes)) written in the last \(windowMinutes) minutes."
        }
    }
}

/// Finds the short-window anomalies in these events: a mass deletion (mistake
/// or ransomware) and a write burst.
///
/// The window and the thresholds live in the Rust core, which every host
/// shares — a second definition of "anomaly" means this app stays quiet
/// through what the Windows and Linux window calls an emergency. This package
/// does not link the core, so the app injects the detector and a host without
/// one simply never raises an anomaly. What the notification *says* stays
/// here: the wording is this platform's, not the policy's.
typealias ActivityAnomalyDetecting = @Sendable (
    _ events: [DiskActivityEvent],
    _ rootPath: URL,
    _ now: Date
) -> [ActivityAnomaly]

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
