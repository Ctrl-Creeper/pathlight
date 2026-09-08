import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity baseline service")
struct ActivityBaselineServiceTests {
    @Test("consumes size and identity as one measurement per path")
    func consumesOneMeasurementPerPath() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let calls = LockedCounter()
        let identity = ActivityBaselineService.ObjectIdentity(device: 7, inode: 11)
        let service = ActivityBaselineService(
            measurementProvider: { _ in
                calls.increment()
                return ActivityBaselineService.Measurement(
                    allocatedSize: 4_096,
                    identity: identity
                )
            },
            contentsProvider: { _ in [] }
        )

        let baseline = await service.captureBaseline(rootPath: root)

        #expect(calls.value == 1)
        #expect(baseline.allocatedSize == 4_096)
        #expect(baseline.measuredObjectCount == 1)
        #expect(baseline.unidentifiedItemCount == 0)
    }

    @Test("captures recursive allocated size baseline")
    func capturesRecursiveAllocatedSizeBaseline() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let child = root.appending(path: "installer.dmg")
        let folder = root.appending(path: "Archives", directoryHint: .isDirectory)
        let nested = folder.appending(path: "old.zip")
        let service = ActivityBaselineService(
            measurementProvider: { url in
                let size = [
                    root.standardizedFileURL.path: Int64(64),
                    child.standardizedFileURL.path: Int64(2_048),
                    folder.standardizedFileURL.path: Int64(128),
                    nested.standardizedFileURL.path: Int64(1_024)
                ][url.standardizedFileURL.path]
                return ActivityBaselineService.Measurement(
                    allocatedSize: size,
                    identity: nil
                )
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
        #expect(baseline.unidentifiedItemCount == 4)
        #expect(!baseline.withObservedChanges(false).isUsableForReconciliation)
    }

    @Test("counts unreadable items without failing the baseline")
    func countsUnreadableItems() async {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let privateFolder = root.appending(path: "Private", directoryHint: .isDirectory)
        let service = ActivityBaselineService(
            measurementProvider: { url in
                ActivityBaselineService.Measurement(
                    allocatedSize: url.standardizedFileURL.path == root.standardizedFileURL.path ? 64 : nil,
                    identity: nil
                )
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
        #expect(baseline.scanState == .partial)
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

    @Test("counts allocated bytes once for multiple hard links to one file")
    func deduplicatesHardLinkedBytes() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appending(path: "original.bin")
        try Data(repeating: 0x41, count: 16_384).write(to: file)
        let before = await ActivityBaselineService().captureBaseline(rootPath: root)
        try FileManager.default.linkItem(at: file, to: root.appending(path: "alias.bin"))

        let after = await ActivityBaselineService().captureBaseline(rootPath: root)

        #expect(after.allocatedSize == before.allocatedSize)
        #expect(after.measuredItemCount == before.measuredItemCount + 1)
        #expect(after.measuredObjectCount == before.measuredObjectCount)
        #expect(after.unidentifiedItemCount == 0)
    }

    @Test("records the scan interval without asserting a point-in-time snapshot")
    func recordsScanInterval() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let startedAt = Date(timeIntervalSince1970: 100)
        let finishedAt = Date(timeIntervalSince1970: 110)
        let service = ActivityBaselineService(now: { finishedAt })

        let snapshot = await service.captureBaseline(rootPath: root, capturedAt: startedAt)

        #expect(snapshot.capturedAt == startedAt)
        #expect(snapshot.scanStartedAt == startedAt)
        #expect(snapshot.scanFinishedAt == finishedAt)
        #expect(snapshot.scanState == .completed)
        #expect(snapshot.consistency == .unverified)
        #expect(!snapshot.isUsableForReconciliation)
        #expect(snapshot.withObservedChanges(false).isUsableForReconciliation)
        #expect(!snapshot.withObservedChanges(true).isUsableForReconciliation)
    }

    @Test("cancelled scans explicitly remain incomplete")
    func marksCancelledScan() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return await ActivityBaselineService().captureBaseline(rootPath: root)
        }

        let snapshot = await task.value

        #expect(snapshot.scanState == .cancelled)
        #expect(!snapshot.withObservedChanges(false).isUsableForReconciliation)
    }

    @Test("old persisted snapshots retain unknown intervals and cannot be reconciled")
    func decodesLegacySnapshot() throws {
        let data = Data(#"{"rootPath":"file:///Users/example/Downloads/","capturedAt":0,"allocatedSize":1000,"measuredItemCount":10,"unreadableItemCount":0}"#.utf8)

        let snapshot = try JSONDecoder().decode(ActivityBaselineSnapshot.self, from: data)

        #expect(snapshot.allocatedSize == 1_000)
        #expect(snapshot.scanStartedAt == nil)
        #expect(snapshot.scanFinishedAt == nil)
        #expect(snapshot.scanState == .unknown)
        #expect(snapshot.consistency == .unverified)
        #expect(snapshot.measuredObjectCount == nil)
        #expect(!snapshot.withObservedChanges(false).isUsableForReconciliation)
        #expect(try JSONDecoder().decode(ActivityBaselineSnapshot.self, from: JSONEncoder().encode(snapshot)) == snapshot)
    }
}

private final class LockedCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var value: Int { lock.withLock { count } }
    func increment() { lock.withLock { count += 1 } }
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appending(path: "PathlightBaselineTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Suite("Activity baseline reconciler")
struct ActivityBaselineReconcilerTests {
    @Test("an unreadable subtree cannot become an estimated deletion")
    func refusesPartialSnapshotDelta() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let previous = snapshot(root: root, start: 0, finish: 10, allocatedSize: 1_000, count: 10)
        let partial = snapshot(root: root, start: 50, finish: 60, allocatedSize: 100, count: 1, unreadableCount: 2)

        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: partial) == nil)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: partial, current: previous) == nil)
    }

    @Test("emits one estimated aggregate for the delta between baselines")
    func emitsAggregateForBaselineDelta() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let previous = snapshot(root: root, start: 0, finish: 10, allocatedSize: 1_000, count: 10)
        let current = snapshot(root: root, start: 50, finish: 60, allocatedSize: 4_000, count: 13)

        let event = ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: current)

        #expect(event?.kind == .aggregate)
        #expect(event?.byteDelta == 3_000)
        #expect(event?.timestamp == current.scanFinishedAt)

        let recorded = [
            DiskActivityEvent(kind: .created, path: root.appending(path: "a"), rootPath: root, timestamp: Date(timeIntervalSince1970: 30), byteDelta: 1_000, confidence: .confirmed, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .aggregate, path: root, rootPath: root, timestamp: Date(timeIntervalSince1970: 0), byteDelta: 999, confidence: .estimated, previousPath: nil, affectedItemCount: 1)
        ]
        let alreadyRecorded = ActivityBaselineReconciler.recordedByteDelta(in: recorded, after: previous, before: current)
        #expect(alreadyRecorded == 1_000)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: current, recordedByteDeltaSincePrevious: alreadyRecorded)?.byteDelta == 2_000)
        #expect(event?.affectedItemCount == 3)
        #expect(event?.confidence == .estimated)
        #expect(event?.path == root)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: nil, current: current) == nil)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: current, current: current) == nil)
    }

    @Test("overlapping or actively changing scans cannot create reconciliation rows")
    func refusesUncertainIntervals() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let previous = snapshot(root: root, start: 0, finish: 30, allocatedSize: 1_000, count: 10)
        let overlapping = snapshot(root: root, start: 20, finish: 40, allocatedSize: 500, count: 5)
        let later = snapshot(root: root, start: 50, finish: 60, allocatedSize: 500, count: 5)

        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: overlapping) == nil)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: previous, current: later.withObservedChanges(true)) == nil)
        #expect(ActivityBaselineReconciler.reconciliationEvent(previous: later, current: previous) == nil)
    }

    @Test("subtracts only records between completed scan intervals")
    func subtractsRecordsBetweenIntervals() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let previous = snapshot(root: root, start: 0, finish: 10, allocatedSize: 1_000, count: 10)
        let current = snapshot(root: root, start: 50, finish: 60, allocatedSize: 4_000, count: 13)
        let events = [5, 10, 30, 50, 55].map { timestamp in
            DiskActivityEvent(kind: .modified, path: root, rootPath: root, timestamp: Date(timeIntervalSince1970: Double(timestamp)), byteDelta: 100, confidence: .confirmed, previousPath: nil, affectedItemCount: 1)
        }

        #expect(ActivityBaselineReconciler.recordedByteDelta(in: events, after: previous, before: current) == 100)
    }

    private func snapshot(root: URL, start: TimeInterval, finish: TimeInterval, allocatedSize: Int64, count: Int, unreadableCount: Int = 0) -> ActivityBaselineSnapshot {
        ActivityBaselineSnapshot(
            rootPath: root, capturedAt: Date(timeIntervalSince1970: start),
            allocatedSize: allocatedSize, measuredItemCount: count, unreadableItemCount: unreadableCount,
            scanStartedAt: Date(timeIntervalSince1970: start), scanFinishedAt: Date(timeIntervalSince1970: finish),
            scanState: unreadableCount == 0 ? .completed : .partial,
            consistency: .noObservedChanges, measuredObjectCount: count, unidentifiedItemCount: 0
        )
    }
}
