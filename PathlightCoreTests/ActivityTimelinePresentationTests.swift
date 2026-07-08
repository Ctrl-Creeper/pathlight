import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity timeline presentation")
struct ActivityTimelinePresentationTests {
    @Test("summarizes net growth and recent events")
    func summarizesNetGrowthAndRecentEvents() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        var session = WatchSessionModel(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000011")!,
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100)
        )
        session.append([
            DiskActivityEvent(
                kind: .created,
                path: root.appending(path: "a.dmg"),
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 120),
                byteDelta: 2_048,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            ),
            DiskActivityEvent(
                kind: .deleted,
                path: root.appending(path: "old.zip"),
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 130),
                byteDelta: -1_024,
                confidence: .estimated,
                previousPath: nil,
                affectedItemCount: 1
            )
        ])

        let presentation = ActivityTimelinePresentation(session: session)

        #expect(presentation.title == "Watching Downloads")
        #expect(presentation.summaryText == "2 events • +1 KB net")
        #expect(presentation.rows.map(\.title) == ["Deleted old.zip", "Created a.dmg"])
        #expect(presentation.rows.map(\.detail) == ["-1 KB", "+2 KB"])
    }

    @Test("shows unknown size events in summary and rows")
    func showsUnknownSizeEvents() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        var session = WatchSessionModel(rootPath: root, startedAt: Date(timeIntervalSince1970: 100))
        session.append([
            DiskActivityEvent(
                kind: .deleted,
                path: root.appending(path: "missing.zip"),
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 140),
                byteDelta: nil,
                confidence: .unknown,
                previousPath: nil,
                affectedItemCount: 1
            )
        ])

        let presentation = ActivityTimelinePresentation(session: session)

        #expect(presentation.summaryText == "1 event • unknown size")
        #expect(presentation.rows.first?.title == "Deleted missing.zip")
        #expect(presentation.rows.first?.detail == "Unknown size")
    }
}
