import Foundation
import UserNotifications

nonisolated protocol ActivityGrowthAlertPosting: Sendable {
    func postGrowthAlert(rootPath: URL, growthBytes: Int64, thresholdBytes: Int64) async
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

final class UserNotificationGrowthAlertPoster: ActivityGrowthAlertPosting, Sendable {
    func postGrowthAlert(rootPath: URL, growthBytes: Int64, thresholdBytes: Int64) async {
        let center = UNUserNotificationCenter.current()
        let granted = (try? await center.requestAuthorization(options: [.alert, .badge])) ?? false
        guard granted else {
            return
        }

        let name = rootPath.lastPathComponent.isEmpty ? rootPath.path : rootPath.lastPathComponent
        let content = UNMutableNotificationContent()
        content.title = "\(name) is growing quickly"
        content.body = "Up \(PathlightFormatters.size(growthBytes)) today — past your \(PathlightFormatters.size(thresholdBytes)) alert threshold."
        let request = UNNotificationRequest(
            identifier: "pathlight-growth-\(rootPath.standardizedFileURL.path)",
            content: content,
            trigger: nil
        )
        try? await center.add(request)
    }
}
