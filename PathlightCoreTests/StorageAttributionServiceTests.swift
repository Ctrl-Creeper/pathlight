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
}
