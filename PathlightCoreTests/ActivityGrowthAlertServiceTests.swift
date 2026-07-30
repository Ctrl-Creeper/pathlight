import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity growth alert evaluator")
struct ActivityGrowthAlertServiceTests {
    private let calendar = Calendar(identifier: .gregorian)
    private let now = Date(timeIntervalSince1970: 200_000)

    @Test("fires when today's growth crosses the threshold")
    func firesWhenGrowthCrossesThreshold() {
        let growth = ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: 1_000),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: nil,
            now: now,
            calendar: calendar
        )

        #expect(growth == 1_500)
    }

    @Test("stays quiet below the threshold, without a threshold, or when paused")
    func staysQuietWhenNotEligible() {
        #expect(ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: 2_000),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: nil,
            now: now,
            calendar: calendar
        ) == nil)

        #expect(ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: nil),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: nil,
            now: now,
            calendar: calendar
        ) == nil)

        #expect(ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: 1_000, isEnabled: false),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: nil,
            now: now,
            calendar: calendar
        ) == nil)
    }

    @Test("alerts at most once per day but again the next day")
    func alertsOncePerDay() {
        let alertedToday = calendar.startOfDay(for: now).addingTimeInterval(60)
        #expect(ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: 1_000),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: alertedToday,
            now: now,
            calendar: calendar
        ) == nil)

        let alertedYesterday = calendar.startOfDay(for: now).addingTimeInterval(-60)
        #expect(ActivityGrowthAlertEvaluator.alertworthyGrowth(
            target: target(thresholdBytes: 1_000),
            history: history(todayByteDelta: 1_500),
            lastAlertedAt: alertedYesterday,
            now: now,
            calendar: calendar
        ) == 1_500)
    }

    private func target(thresholdBytes: Int64?, isEnabled: Bool = true) -> LongTermWatchTarget {
        LongTermWatchTarget(
            rootPath: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory),
            isEnabled: isEnabled,
            options: LongTermWatchTargetOptions(
                minimumRecordedByteDelta: 1,
                aggregationWindow: 300,
                recordsFileNames: true,
                growthAlertThresholdBytes: thresholdBytes
            )
        )
    }

    private func history(todayByteDelta: Int64) -> ActivityHistorySnapshot {
        let dayStart = calendar.startOfDay(for: now)
        return ActivityHistorySnapshot(
            rootPath: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory),
            generatedAt: now,
            totalNetByteDelta: todayByteDelta,
            eventCount: 1,
            unknownSizeEventCount: 0,
            buckets: [
                ActivityHistoryBucket(
                    startDate: dayStart.addingTimeInterval(3_600),
                    endDate: dayStart.addingTimeInterval(7_200),
                    byteDelta: todayByteDelta,
                    eventCount: 1,
                    unknownSizeEventCount: 0
                )
            ],
            recentEvents: []
        )
    }
}
