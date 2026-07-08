import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity history presentation")
struct ActivityHistoryPresentationTests {
    @Test("summarizes history snapshot and bucket trend")
    func summarizesHistorySnapshotAndBucketTrend() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let snapshot = ActivityHistorySnapshot(
            rootPath: root,
            generatedAt: Date(timeIntervalSince1970: 20_000),
            totalNetByteDelta: 1_024,
            eventCount: 2,
            unknownSizeEventCount: 0,
            buckets: [
                ActivityHistoryBucket(
                    startDate: Date(timeIntervalSince1970: 3_600),
                    endDate: Date(timeIntervalSince1970: 7_200),
                    byteDelta: 2_048,
                    eventCount: 1,
                    unknownSizeEventCount: 0
                ),
                ActivityHistoryBucket(
                    startDate: Date(timeIntervalSince1970: 7_200),
                    endDate: Date(timeIntervalSince1970: 10_800),
                    byteDelta: -1_024,
                    eventCount: 1,
                    unknownSizeEventCount: 0
                )
            ],
            recentEvents: [
                event(.deleted, root: root, name: "old.zip", timestamp: 3_900, byteDelta: -1_024),
                event(.created, root: root, name: "installer.dmg", timestamp: 3_600, byteDelta: 2_048)
            ]
        )

        let presentation = ActivityHistoryPresentation(snapshot: snapshot, eventLimit: 4)

        #expect(presentation.title == "History for Downloads")
        #expect(presentation.summaryText == "2 events • +1 KB net")
        #expect(presentation.buckets.map(\.detail) == ["+2 KB", "-1 KB"])
        #expect(presentation.buckets.map(\.magnitudeFraction) == [1.0, 0.5])
        #expect(presentation.rows.map(\.title) == ["Deleted old.zip", "Created installer.dmg"])
    }

    @Test("marks unknown-size history without changing net summary")
    func marksUnknownSizeHistory() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let snapshot = ActivityHistorySnapshot(
            rootPath: root,
            generatedAt: Date(timeIntervalSince1970: 20_000),
            totalNetByteDelta: 0,
            eventCount: 1,
            unknownSizeEventCount: 1,
            buckets: [
                ActivityHistoryBucket(
                    startDate: Date(timeIntervalSince1970: 3_600),
                    endDate: Date(timeIntervalSince1970: 7_200),
                    byteDelta: 0,
                    eventCount: 1,
                    unknownSizeEventCount: 1
                )
            ],
            recentEvents: [
                event(.deleted, root: root, name: "missing.zip", timestamp: 3_900, byteDelta: nil)
            ]
        )

        let presentation = ActivityHistoryPresentation(snapshot: snapshot)

        #expect(presentation.summaryText == "1 event • unknown size")
        #expect(presentation.buckets.first?.detail == "Unknown size")
        #expect(presentation.rows.first?.detail == "Unknown size")
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
