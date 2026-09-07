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

    @Test("walks a real directory without counting files as unreadable")
    func walksRealDirectory() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let subdirectory = root.appending(path: "Nested", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: subdirectory, withIntermediateDirectories: true)
        try Data(repeating: 0, count: 4_096).write(to: root.appending(path: "top.bin"))
        try Data(repeating: 0, count: 4_096).write(to: subdirectory.appending(path: "nested.bin"))

        let baseline = await ActivityBaselineService().captureBaseline(rootPath: root)

        #expect(baseline.unreadableItemCount == 0)
        #expect(baseline.measuredItemCount == 4)
        #expect(baseline.allocatedSize >= 8_192)
    }

    @Test("terminates on symlink cycles without following them")
    func terminatesOnSymlinkCycles() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try Data(repeating: 0, count: 4_096).write(to: root.appending(path: "file.bin"))
        try FileManager.default.createSymbolicLink(
            at: root.appending(path: "loop"),
            withDestinationURL: root
        )

        let baseline = await ActivityBaselineService().captureBaseline(rootPath: root)

        #expect(baseline.measuredItemCount + baseline.unreadableItemCount <= 3)
    }
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appending(path: "PathlightBaselineTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Suite("Activity baseline reconciler")
struct ActivityBaselineReconcilerTests {
    @Test("emits one estimated aggregate for the delta between baselines")
    func emitsAggregateForBaselineDelta() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let previous = ActivityBaselineSnapshot(rootPath: root, capturedAt: Date(timeIntervalSince1970: 0), allocatedSize: 1_000, measuredItemCount: 10, unreadableItemCount: 0)
        let current = ActivityBaselineSnapshot(rootPath: root, capturedAt: Date(timeIntervalSince1970: 60), allocatedSize: 4_000, measuredItemCount: 13, unreadableItemCount: 0)

        let event = ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: current, at: Date(timeIntervalSince1970: 60))

        #expect(event?.kind == .aggregate)
        #expect(event?.byteDelta == 3_000)
        #expect(event?.affectedItemCount == 3)
        #expect(event?.confidence == .estimated)
        #expect(event?.path == root)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: nil, current: current) == nil)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: current, current: current) == nil)
    }
}
