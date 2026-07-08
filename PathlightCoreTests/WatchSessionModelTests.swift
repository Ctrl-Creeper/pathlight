import Foundation
import Testing
@testable import PathlightCore

@Suite("Watch session model")
struct WatchSessionModelTests {
    @Test("summarizes net growth and event counts")
    func summarizesNetGrowthAndEventCounts() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        var session = WatchSessionModel(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100)
        )

        session.append([
            DiskActivityEvent(kind: .created, path: root.appending(path: "a.dmg"), rootPath: root, timestamp: Date(timeIntervalSince1970: 110), byteDelta: 800, confidence: .confirmed, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .deleted, path: root.appending(path: "b.zip"), rootPath: root, timestamp: Date(timeIntervalSince1970: 120), byteDelta: -300, confidence: .estimated, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .deleted, path: root.appending(path: "c.zip"), rootPath: root, timestamp: Date(timeIntervalSince1970: 130), byteDelta: nil, confidence: .unknown, previousPath: nil, affectedItemCount: 1)
        ])

        let summary = session.summary(endedAt: Date(timeIntervalSince1970: 200))

        #expect(summary.netByteDelta == 500)
        #expect(summary.eventCount == 3)
        #expect(summary.unknownSizeEventCount == 1)
        #expect(summary.startedAt == Date(timeIntervalSince1970: 100))
        #expect(summary.endedAt == Date(timeIntervalSince1970: 200))
    }
}
