import Foundation
import Testing
@testable import PathlightCore

@Suite("Storage attribution service")
struct StorageAttributionServiceTests {
    @Test("records a created file above the threshold")
    func recordsCreatedFileAboveThreshold() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "movie.dmg")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { url in
                url.path == file.path ? 842 : nil
            }
        )

        let events = service.process([
            DiskActivityChange(kind: .created, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 100))
        ])

        #expect(events == [
            DiskActivityEvent(
                kind: .created,
                path: file,
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 100),
                byteDelta: 842,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        ])
    }

    @Test("filters events below the configured threshold")
    func filtersEventsBelowThreshold() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let smallFile = root.appending(path: "small.txt")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 100,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in 99 }
        )

        let events = service.process([
            DiskActivityChange(kind: .modified, path: smallFile, rootPath: root, timestamp: Date(timeIntervalSince1970: 100))
        ])

        #expect(events.isEmpty)
    }

    @Test("records deleted files with known prior size as negative delta")
    func recordsDeletedFileWithKnownPriorSize() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "old.zip")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in nil },
            priorSizeProvider: { url in
                url.path == file.path ? 500 : nil
            }
        )

        let events = service.process([
            DiskActivityChange(kind: .deleted, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
        ])

        #expect(events.first?.kind == .deleted)
        #expect(events.first?.byteDelta == -500)
        #expect(events.first?.confidence == .estimated)
    }

    @Test("records deleted files with unknown prior size")
    func recordsDeletedFileWithUnknownPriorSize() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "missing.zip")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in nil },
            priorSizeProvider: { _ in nil }
        )

        let events = service.process([
            DiskActivityChange(kind: .deleted, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
        ])

        #expect(events.first?.kind == .deleted)
        #expect(events.first?.byteDelta == nil)
        #expect(events.first?.confidence == .unknown)
    }

    @Test("records the departure side of a rename with the last known size")
    func recordsRenameDepartureWithKnownPriorSize() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "trashed.zip")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in nil },
            priorSizeProvider: { url in
                url.path == file.path ? 700 : nil
            }
        )

        let events = service.process([
            DiskActivityChange(kind: .renamed(previousPath: nil), path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
        ])

        #expect(events.first?.kind == .moved)
        #expect(events.first?.byteDelta == -700)
        #expect(events.first?.confidence == .estimated)
    }

    @Test("records the departure side of a rename with unknown prior size")
    func recordsRenameDepartureWithUnknownPriorSize() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "vanished.zip")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in nil },
            priorSizeProvider: { _ in nil }
        )

        let events = service.process([
            DiskActivityChange(kind: .renamed(previousPath: nil), path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
        ])

        #expect(events.first?.kind == .moved)
        #expect(events.first?.byteDelta == nil)
        #expect(events.first?.confidence == .unknown)
    }

    @Test("aggregates many changes under the same parent inside the window")
    func aggregatesChangesByParentInsideWindow() {
        let root = URL(filePath: "/Users/example/Library/Caches", directoryHint: .isDirectory)
        let parent = root.appending(path: "Browser")
        let changes = [
            DiskActivityChange(kind: .created, path: parent.appending(path: "a.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 100)),
            DiskActivityChange(kind: .created, path: parent.appending(path: "b.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 130)),
            DiskActivityChange(kind: .modified, path: parent.appending(path: "c.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 140))
        ]
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { _ in 50 }
        )

        let events = service.process(changes)

        #expect(events.count == 1)
        #expect(events.first?.kind == .aggregate)
        #expect(events.first?.path == parent)
        #expect(events.first?.byteDelta == 150)
        #expect(events.first?.affectedItemCount == 3)
    }
}

@Suite("Storage attribution increments")
struct StorageAttributionIncrementTests {
    private let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

    @Test("a modification reports growth since the last known size")
    func modificationReportsGrowth() {
        let file = root.appending(path: "report.pdf")
        let service = StorageAttributionService(
            options: .shortTermDefault,
            sizeProvider: { _ in 5_000 },
            knownSizeProvider: { url in url.path == file.path ? 3_000 : nil }
        )
        let events = service.process([
            DiskActivityChange(kind: .modified, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 10)),
            DiskActivityChange(kind: .created, path: root.appending(path: "fresh.txt"), rootPath: root, timestamp: Date(timeIntervalSince1970: 11))
        ])
        #expect(events.map(\.byteDelta) == [2_000, 5_000])
    }

    @Test("a move inside the root nets to growth, a move in from outside counts fully")
    func movesInsideAndIntoRoot() {
        let destination = root.appending(path: "final.mov")
        let previousInside = root.appending(path: "draft.mov")
        let previousOutside = URL(filePath: "/Users/example/Desktop/clip.mov")
        let service = StorageAttributionService(
            options: .shortTermDefault,
            sizeProvider: { _ in 9_000 },
            priorSizeProvider: { url in
                url.path == previousInside.path ? 9_000 : nil
            }
        )
        let events = service.process([
            DiskActivityChange(kind: .renamed(previousPath: previousInside), path: destination, rootPath: root, timestamp: Date(timeIntervalSince1970: 10)),
            DiskActivityChange(kind: .renamed(previousPath: previousOutside), path: destination, rootPath: root, timestamp: Date(timeIntervalSince1970: 11)),
            DiskActivityChange(kind: .renamed(previousPath: root.appending(path: "unknown.mov")), path: destination, rootPath: root, timestamp: Date(timeIntervalSince1970: 12))
        ])
        #expect(events.map(\.byteDelta) == [0, 9_000, 9_000])
        #expect(events.map(\.confidence) == [.confirmed, .confirmed, .estimated])
    }
}
