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
}
