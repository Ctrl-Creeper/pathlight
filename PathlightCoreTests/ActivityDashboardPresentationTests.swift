import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity dashboard presentation")
struct ActivityDashboardPresentationTests {
    @Test("summarizes long-term targets and selected history")
    func summarizesLongTermTargetsAndSelectedHistory() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let desktop = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let history = historySnapshot(
            rootPath: downloads,
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
            histories: [history]
        )

        #expect(presentation.title == "Activity")
        #expect(presentation.summaryText == "2 tracked folders • 1 active")
        #expect(presentation.selectedTargetID == downloads.standardizedFileURL.path)
        #expect(presentation.targetRows.map(\.title) == ["Downloads", "Desktop"])
        #expect(presentation.targetRows.map(\.statusText) == ["Starting", "Paused"])
        #expect(presentation.targetRows.first?.changeText == "+1.07 GB net")
        #expect(presentation.targetRows.first?.eventText == "3 events")
        #expect(presentation.targetRows.first?.thresholdText == "Records changes over 1 MB")
        #expect(presentation.trendBuckets.map(\.detail) == ["+1.07 GB"])
        #expect(presentation.timelineRows.map(\.title) == ["Created movie.mov", "Deleted missing.zip"])
    }

    @Test("shows runtime watch state and last activity time")
    func showsRuntimeWatchStateAndLastActivityTime() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let lastActivityAt = Date(timeIntervalSince1970: 9_000)
        let presentation = ActivityDashboardPresentation(
            targets: [LongTermWatchTarget(rootPath: downloads)],
            selectedRootPath: downloads,
            histories: [],
            runtimeStatuses: [
                downloads.standardizedFileURL.path: LongTermWatchRuntimeStatus(
                    state: .reconnecting,
                    lastActivityAt: lastActivityAt,
                    retryCount: 2
                )
            ]
        )

        #expect(presentation.targetRows.first?.statusText == "Reconnecting")
        #expect(
            presentation.targetRows.first?.lastActivityText
                == "Last activity \(PathlightFormatters.date(lastActivityAt))"
        )
    }

    /// A watch whose folder was renamed away kept reporting "Watching" and
    /// still counted toward the active total, so nothing said it was blind.
    @Test("says so when the watched folder is gone")
    func reportsAMissingRoot() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let desktop = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let presentation = ActivityDashboardPresentation(
            targets: [LongTermWatchTarget(rootPath: downloads), LongTermWatchTarget(rootPath: desktop)],
            selectedRootPath: downloads,
            histories: [],
            runtimeStatuses: [
                downloads.standardizedFileURL.path: LongTermWatchRuntimeStatus(state: .rootMissing, lastActivityAt: nil, retryCount: 0),
                desktop.standardizedFileURL.path: LongTermWatchRuntimeStatus(state: .watching, lastActivityAt: nil, retryCount: 0)
            ]
        )

        #expect(presentation.targetRows.first?.statusText == "Folder Missing")
        #expect(
            presentation.targetRows.first?.lastActivityText
                == "Folder is missing — nothing can be recorded"
        )
        #expect(presentation.summaryText.contains("1 active"))
    }

    /// `/` has no last path component, so the whole-disk watch showed the raw
    /// path as both its title and its subtitle.
    @Test("names the whole disk instead of showing a bare slash")
    func namesTheWholeDisk() {
        let root = URL(filePath: "/", directoryHint: .isDirectory)

        #expect(WatchRootNaming.displayName(for: root) == "Whole Disk")
        #expect(
            WatchRootNaming.displayName(
                for: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
            ) == "Downloads"
        )
    }

    @Test("shows per-target history summaries for unselected targets")
    func showsPerTargetHistorySummaries() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let desktop = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let downloadsHistory = historySnapshot(
            rootPath: downloads,
            totalNetByteDelta: 20 * 1_024 * 1_024,
            eventCount: 2
        )
        let desktopHistory = historySnapshot(
            rootPath: desktop,
            totalNetByteDelta: -5 * 1_024 * 1_024,
            eventCount: 1
        )

        let presentation = ActivityDashboardPresentation(
            targets: [
                LongTermWatchTarget(rootPath: downloads),
                LongTermWatchTarget(rootPath: desktop)
            ],
            selectedRootPath: downloads,
            histories: [downloadsHistory, desktopHistory]
        )

        #expect(presentation.targetRows.map(\.changeText) == ["+21 MB net", "-5.2 MB net"])
        #expect(presentation.targetRows.map(\.eventText) == ["2 events", "1 event"])
        #expect(presentation.trendBuckets.isEmpty)
        #expect(presentation.selectedTargetID == downloads.standardizedFileURL.path)
    }

    @Test("flags rows with a history gap")
    func flagsRowsWithHistoryGap() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let desktop = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)

        let presentation = ActivityDashboardPresentation(
            targets: [
                LongTermWatchTarget(rootPath: downloads),
                LongTermWatchTarget(rootPath: desktop)
            ],
            selectedRootPath: downloads,
            histories: [],
            runtimeStatuses: [
                downloads.standardizedFileURL.path: LongTermWatchRuntimeStatus(
                    state: .historyGap,
                    lastActivityAt: nil,
                    retryCount: 0
                ),
                desktop.standardizedFileURL.path: LongTermWatchRuntimeStatus(
                    state: .watching,
                    lastActivityAt: nil,
                    retryCount: 0
                )
            ]
        )

        #expect(presentation.targetRows.map(\.hasHistoryGap) == [true, false])
        #expect(presentation.targetRows.first?.statusText == "History Gap")
    }

    @Test("aggregates top changes by immediate child of the watch root")
    func aggregatesTopChangesByImmediateChild() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let history = historySnapshot(
            rootPath: downloads,
            totalNetByteDelta: 0,
            eventCount: 4,
            recentEvents: [
                event(.created, root: downloads, name: "Movies/one.mov", timestamp: 100, byteDelta: 3_000),
                event(.created, root: downloads, name: "Movies/two.mov", timestamp: 110, byteDelta: 2_000),
                event(.deleted, root: downloads, name: "Archives/old.zip", timestamp: 120, byteDelta: -1_000),
                event(.created, root: downloads, name: "loose.txt", timestamp: 130, byteDelta: 500)
            ]
        )

        let presentation = ActivityDashboardPresentation(
            targets: [LongTermWatchTarget(rootPath: downloads)],
            selectedRootPath: downloads,
            histories: [history]
        )

        #expect(presentation.topChanges.map(\.title) == ["Movies", "Archives", "loose.txt"])
        #expect(presentation.topChanges.map(\.byteDelta) == [5_000, -1_000, 500])
        #expect(presentation.topChanges.first?.eventText == "2 events")
        #expect(presentation.topChanges.first?.magnitudeFraction == 1)
        #expect(presentation.topChanges.first?.url == downloads.appending(path: "Movies"))
    }

    @Test("summarizes menu bar activity with today's net change")
    func summarizesMenuBarActivity() {
        let downloads = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let calendar = Calendar(identifier: .gregorian)
        let now = Date(timeIntervalSince1970: 200_000)
        let dayStart = calendar.startOfDay(for: now)
        let history = historySnapshot(
            rootPath: downloads,
            totalNetByteDelta: 4_096,
            eventCount: 2,
            buckets: [
                ActivityHistoryBucket(
                    startDate: dayStart.addingTimeInterval(-7_200),
                    endDate: dayStart.addingTimeInterval(-3_600),
                    byteDelta: 1_000_000,
                    eventCount: 1,
                    unknownSizeEventCount: 0
                ),
                ActivityHistoryBucket(
                    startDate: dayStart.addingTimeInterval(3_600),
                    endDate: dayStart.addingTimeInterval(7_200),
                    byteDelta: 2_048,
                    eventCount: 1,
                    unknownSizeEventCount: 0
                )
            ]
        )

        let presentation = MenuBarActivityPresentation(
            targets: [LongTermWatchTarget(rootPath: downloads)],
            histories: [history],
            runtimeStatuses: [
                downloads.standardizedFileURL.path: LongTermWatchRuntimeStatus(
                    state: .watching,
                    lastActivityAt: nil,
                    retryCount: 0
                )
            ],
            now: now,
            calendar: calendar
        )

        #expect(presentation.rows.map(\.title) == ["Downloads"])
        #expect(presentation.rows.first?.statusText == "Watching")
        #expect(presentation.rows.first?.todayChangeText == "+2 KB today")
        #expect(presentation.summaryText == "1 watching • +2 KB today")
    }

    @Test("shows empty state when there are no long-term targets")
    func showsEmptyState() {
        let presentation = ActivityDashboardPresentation(
            targets: [],
            selectedRootPath: nil,
            histories: []
        )

        #expect(presentation.summaryText == "No tracked folders")
        #expect(presentation.emptyStateTitle == "No Long-Term Watches")
        #expect(presentation.targetRows.isEmpty)
        #expect(presentation.timelineRows.isEmpty)
    }
}

private func historySnapshot(
    rootPath: URL,
    totalNetByteDelta: Int64,
    eventCount: Int,
    unknownSizeEventCount: Int = 0,
    buckets: [ActivityHistoryBucket] = [],
    recentEvents: [DiskActivityEvent] = []
) -> ActivityHistorySnapshot {
    ActivityHistorySnapshot(
        rootPath: rootPath,
        generatedAt: Date(timeIntervalSince1970: 20_000),
        totalNetByteDelta: totalNetByteDelta,
        eventCount: eventCount,
        unknownSizeEventCount: unknownSizeEventCount,
        buckets: buckets,
        recentEvents: recentEvents
    )
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

@Suite("Activity dashboard recent deletions")
struct ActivityDashboardRecentDeletionsTests {
    @Test("lists the largest removals with a trash hint")
    func listsLargestRemovals() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let target = LongTermWatchTarget(rootPath: root)
        func event(_ name: String, kind: DiskActivityEventKind, delta: Int64?) -> DiskActivityEvent {
            DiskActivityEvent(kind: kind, path: root.appending(path: name), rootPath: root, timestamp: Date(timeIntervalSince1970: 100), byteDelta: delta, confidence: .confirmed, previousPath: nil, affectedItemCount: 1)
        }
        let history = ActivityHistorySnapshot(
            rootPath: root,
            generatedAt: Date(timeIntervalSince1970: 200),
            totalNetByteDelta: 0,
            eventCount: 4,
            unknownSizeEventCount: 1,
            buckets: [],
            recentEvents: [
                event("small.txt", kind: .deleted, delta: -10),
                event("movie.mov", kind: .deleted, delta: -5_000_000),
                event("kept.txt", kind: .created, delta: 500),
                event("archive.zip", kind: .moved, delta: -2_000)
            ]
        )

        let presentation = ActivityDashboardPresentation(
            targets: [target],
            selectedRootPath: root,
            histories: [history],
            trashContains: { $0 == "movie.mov" }
        )

        #expect(presentation.recentDeletions.map(\.title) == ["movie.mov", "archive.zip", "small.txt"])
        #expect(presentation.recentDeletions.map(\.isInTrash) == [true, false, false])
        #expect(presentation.recentDeletions.first?.sizeText == "5 MB")
    }
}
