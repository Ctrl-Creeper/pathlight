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

    @Test("coalesces an immediate created and modified burst for one file")
    func coalescesCreatedAndModifiedBurst() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "copy.txt")
        var session = WatchSessionModel(rootPath: root)

        session.append([
            DiskActivityEvent(
                kind: .created,
                path: file,
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 100),
                byteDelta: 100,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            ),
            DiskActivityEvent(
                kind: .modified,
                path: file,
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 100.2),
                byteDelta: 100,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        ], coalescingWindow: 1)

        #expect(session.events.count == 1)
        #expect(session.events.first?.kind == .created)
        #expect(session.events.first?.timestamp == Date(timeIntervalSince1970: 100.2))
    }

    @Test("does not coalesce a deletion with a previous update")
    func keepsDeletionAsItsOwnEvent() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "remove.txt")
        var session = WatchSessionModel(rootPath: root)

        session.append([
            DiskActivityEvent(kind: .modified, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 100), byteDelta: 100, confidence: .confirmed, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .deleted, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 100.2), byteDelta: -100, confidence: .estimated, previousPath: nil, affectedItemCount: 1)
        ], coalescingWindow: 1)

        #expect(session.events.map(\.kind) == [.modified, .deleted])
    }
}

@Suite("Watch session model storm cap")
struct WatchSessionModelStormTests {
    @Test("drops the oldest events past the retention cap")
    func dropsOldestEventsPastCap() {
        let root = URL(filePath: "/Users/example", directoryHint: .isDirectory)
        var session = WatchSessionModel(rootPath: root)
        let events = (0..<(WatchSessionModel.maxRetainedEvents + 10)).map { index in
            DiskActivityEvent(
                kind: .modified,
                path: root.appending(path: "file\(index)"),
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: TimeInterval(index)),
                byteDelta: 1,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        }
        session.append(events)

        #expect(session.events.count == WatchSessionModel.maxRetainedEvents)
        #expect(session.droppedEventCount == 10)
        #expect(session.events.first?.path.lastPathComponent == "file10")
    }
}

@Suite("Watch session model change tracking")
struct WatchSessionModelChangeTrackingTests {
    private let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

    private func event(_ name: String, kind: DiskActivityEventKind, at seconds: TimeInterval, delta: Int64?) -> DiskActivityEvent {
        DiskActivityEvent(kind: kind, path: root.appending(path: name), rootPath: root, timestamp: Date(timeIntervalSince1970: seconds), byteDelta: delta, confidence: .confirmed, previousPath: nil, affectedItemCount: 1)
    }

    @Test("a merged burst keeps one identity and sums incremental deltas")
    func mergedBurstKeepsIdentity() {
        var session = WatchSessionModel(rootPath: root)
        session.append([event("copy.txt", kind: .created, at: 100, delta: 100)], coalescingWindow: 1)
        let firstID = session.latestChanges.first?.id
        #expect(session.latestChanges.count == 1)

        session.clearLatestChanges()
        session.append([
            event("copy.txt", kind: .modified, at: 100.5, delta: 400),
            event("other.txt", kind: .created, at: 100.4, delta: 7)
        ], coalescingWindow: 1)

        #expect(session.events.count == 2)
        #expect(session.latestChanges.count == 2)
        let merged = session.latestChanges.first { $0.event.path.lastPathComponent == "copy.txt" }
        #expect(merged?.id == firstID)
        #expect(merged?.event.kind == .created)
        #expect(merged?.event.byteDelta == 500)
        #expect(session.latestChanges.contains { $0.event.path.lastPathComponent == "other.txt" && $0.id != firstID })

        session.clearLatestChanges()
        #expect(session.latestChanges.isEmpty)
    }

    @Test("an unknown-size side makes the merged delta unknown")
    func unknownSidePoisonsMerge() {
        var session = WatchSessionModel(rootPath: root)
        session.append([
            event("blob.bin", kind: .created, at: 100, delta: 10),
            event("blob.bin", kind: .modified, at: 100.2, delta: nil)
        ], coalescingWindow: 1)
        #expect(session.events.count == 1)
        #expect(session.events.first?.byteDelta == nil)
        #expect(session.events.first?.confidence == .unknown)
    }
}
