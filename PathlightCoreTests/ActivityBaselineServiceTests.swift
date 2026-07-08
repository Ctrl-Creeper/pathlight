import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity baseline service")
struct ActivityBaselineServiceTests {
    @Test("captures recursive allocated size baseline")
    func capturesRecursiveAllocatedSizeBaseline() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let child = root.appending(path: "installer.dmg")
        let folder = root.appending(path: "Archives", directoryHint: .isDirectory)
        let nested = folder.appending(path: "old.zip")
        let service = ActivityBaselineService(
            sizeProvider: { url in
                [
                    root.standardizedFileURL.path: Int64(64),
                    child.standardizedFileURL.path: Int64(2_048),
                    folder.standardizedFileURL.path: Int64(128),
                    nested.standardizedFileURL.path: Int64(1_024)
                ][url.standardizedFileURL.path]
            },
            contentsProvider: { url in
                switch url.standardizedFileURL.path {
                case root.standardizedFileURL.path:
                    return [child, folder]
                case folder.standardizedFileURL.path:
                    return [nested]
                default:
                    return []
                }
            }
        )

        let baseline = await service.captureBaseline(
            rootPath: root,
            capturedAt: Date(timeIntervalSince1970: 100)
        )

        #expect(baseline.rootPath == root.standardizedFileURL)
        #expect(baseline.capturedAt == Date(timeIntervalSince1970: 100))
        #expect(baseline.allocatedSize == 3_264)
        #expect(baseline.measuredItemCount == 4)
        #expect(baseline.unreadableItemCount == 0)
    }

    @Test("counts unreadable items without failing the baseline")
    func countsUnreadableItems() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let privateFolder = root.appending(path: "Private", directoryHint: .isDirectory)
        let service = ActivityBaselineService(
            sizeProvider: { url in
                url.standardizedFileURL.path == root.standardizedFileURL.path ? 64 : nil
            },
            contentsProvider: { url in
                if url.standardizedFileURL.path == root.standardizedFileURL.path {
                    return [privateFolder]
                }
                throw CocoaError(.fileReadNoPermission)
            }
        )

        let baseline = await service.captureBaseline(
            rootPath: root,
            capturedAt: Date(timeIntervalSince1970: 100)
        )

        #expect(baseline.allocatedSize == 64)
        #expect(baseline.measuredItemCount == 1)
        #expect(baseline.unreadableItemCount == 2)
    }
}
