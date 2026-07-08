import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity dashboard presentation")
struct ActivityDashboardPresentationTests {
    @Test("summarizes long-term targets and selected history")
    func summarizesLongTermTargetsAndSelectedHistory() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let desktop = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let history = ActivityHistorySnapshot(
            rootPath: downloads,
            generatedAt: Date(timeIntervalSince1970: 20_000),
            totalNetByteDelta: 1_024 * 1_024 * 1_024,
            eventCount: 3,
            unknownSizeEventCount: 1,
            buckets: [
                ActivityHistoryBucket(
                    startDate: Date(timeIntervalSince1970: 3_600),
                    endDate: Date(timeIntervalSince1970: 7_200),
                    byteDelta: 1_024 * 1_024 * 1_024,
                    eventCount: 2,
                    unknownSizeEventCount: 0
                )
            ],
            recentEvents: [
                event(.created, root: downloads, name: "movie.mov", timestamp: 7_100, byteDelta: 1_024 * 1_024 * 1_024),
                event(.deleted, root: downloads, name: "missing.zip", timestamp: 7_000, byteDelta: nil)
            ]
        )

        let presentation = ActivityDashboardPresentation(
            targets: [
                LongTermWatchTarget(
                    rootPath: downloads,
                    isEnabled: true,
                    options: LongTermWatchTargetOptions(
                        minimumRecordedByteDelta: 1_024 * 1_024,
                        aggregationWindow: 300,
                        recordsFileNames: true
                    )
                ),
                LongTermWatchTarget(rootPath: desktop, isEnabled: false)
            ],
            selectedRootPath: downloads,
            history: history
        )

        #expect(presentation.title == "Activity")
        #expect(presentation.summaryText == "2 tracked folders • 1 active")
        #expect(presentation.selectedTargetID == downloads.standardizedFileURL.path)
        #expect(presentation.targetRows.map(\.title) == ["Downloads", "Desktop"])
        #expect(presentation.targetRows.map(\.statusText) == ["Active", "Paused"])
        #expect(presentation.targetRows.first?.changeText == "+1.07 GB net")
        #expect(presentation.targetRows.first?.eventText == "3 events")
        #expect(presentation.targetRows.first?.thresholdText == "Records changes over 1 MB")
        #expect(presentation.trendBuckets.map(\.detail) == ["+1.07 GB"])
        #expect(presentation.timelineRows.map(\.title) == ["Created movie.mov", "Deleted missing.zip"])
    }

    @Test("shows empty state when there are no long-term targets")
    func showsEmptyState() {
        let presentation = ActivityDashboardPresentation(
            targets: [],
            selectedRootPath: nil,
            history: nil
        )

        #expect(presentation.summaryText == "No tracked folders")
        #expect(presentation.emptyStateTitle == "No Long-Term Watches")
        #expect(presentation.targetRows.isEmpty)
        #expect(presentation.timelineRows.isEmpty)
    }
}

private func event(
    _ kind: DiskActivityEventKind,
    root: URL,
    name: String,
    timestamp: TimeInterval,
    byteDelta: Int64?
) -> DiskActivityEvent {
    DiskActivityEvent(
        kind: kind,
        path: root.appending(path: name),
        rootPath: root,
        timestamp: Date(timeIntervalSince1970: timestamp),
        byteDelta: byteDelta,
        confidence: byteDelta == nil ? .unknown : .confirmed,
        previousPath: nil,
        affectedItemCount: 1
    )
}
