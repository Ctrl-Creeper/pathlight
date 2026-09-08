import Darwin
import Foundation

nonisolated enum ActivityBaselineScanState: String, Codable, Equatable, Sendable {
    /// The traversal finished; this does not imply an atomic filesystem snapshot.
    case completed
    case partial
    case cancelled
    case unknown
}

nonisolated enum ActivityBaselineConsistency: String, Codable, Equatable, Sendable {
    case unverified
    /// A caller validated source coverage across the scan interval. An unchanged
    /// callback counter alone is insufficient when a backend delays notifications.
    case noObservedChanges
    case changed
}

nonisolated struct ActivityBaselineSnapshot: Codable, Equatable, Sendable {
    let rootPath: URL
    /// Legacy display timestamp. New captures use the start of the traversal.
    let capturedAt: Date
    /// Sum of file-reported allocations, deduplicated by object identity where
    /// available. This does not measure exclusive blocks or reclaimed space;
    /// clones can still share physical blocks across distinct file objects.
    let allocatedSize: Int64
    /// Readable path entries, including additional names for the same hard link.
    let measuredItemCount: Int
    let unreadableItemCount: Int
    /// Old persisted snapshots have no known observation interval.
    let scanStartedAt: Date?
    let scanFinishedAt: Date?
    let scanState: ActivityBaselineScanState
    let consistency: ActivityBaselineConsistency
    /// Distinct identified objects. Entries lacking identity are counted separately.
    let measuredObjectCount: Int?
    let unidentifiedItemCount: Int?

    init(
        rootPath: URL,
        capturedAt: Date,
        allocatedSize: Int64,
        measuredItemCount: Int,
        unreadableItemCount: Int,
        scanStartedAt: Date? = nil,
        scanFinishedAt: Date? = nil,
        scanState: ActivityBaselineScanState = .unknown,
        consistency: ActivityBaselineConsistency = .unverified,
        measuredObjectCount: Int? = nil,
        unidentifiedItemCount: Int? = nil
    ) {
        self.rootPath = rootPath.standardizedFileURL
        self.capturedAt = capturedAt
        self.allocatedSize = allocatedSize
        self.measuredItemCount = measuredItemCount
        self.unreadableItemCount = unreadableItemCount
        self.scanStartedAt = scanStartedAt
        self.scanFinishedAt = scanFinishedAt
        self.scanState = scanState
        self.consistency = consistency
        self.measuredObjectCount = measuredObjectCount
        self.unidentifiedItemCount = unidentifiedItemCount
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        rootPath = try container.decode(URL.self, forKey: .rootPath).standardizedFileURL
        capturedAt = try container.decode(Date.self, forKey: .capturedAt)
        allocatedSize = try container.decode(Int64.self, forKey: .allocatedSize)
        measuredItemCount = try container.decode(Int.self, forKey: .measuredItemCount)
        unreadableItemCount = try container.decode(Int.self, forKey: .unreadableItemCount)
        scanStartedAt = try container.decodeIfPresent(Date.self, forKey: .scanStartedAt)
        scanFinishedAt = try container.decodeIfPresent(Date.self, forKey: .scanFinishedAt)
        scanState = try container.decodeIfPresent(ActivityBaselineScanState.self, forKey: .scanState) ?? .unknown
        consistency = try container.decodeIfPresent(ActivityBaselineConsistency.self, forKey: .consistency) ?? .unverified
        measuredObjectCount = try container.decodeIfPresent(Int.self, forKey: .measuredObjectCount)
        unidentifiedItemCount = try container.decodeIfPresent(Int.self, forKey: .unidentifiedItemCount)
    }

    var isUsableForReconciliation: Bool {
        guard let scanStartedAt, let scanFinishedAt else { return false }
        return scanState == .completed
            && consistency == .noObservedChanges
            && unreadableItemCount == 0
            && unidentifiedItemCount == 0
            && measuredObjectCount != nil
            && scanFinishedAt >= scanStartedAt
    }

    /// `false` requires an explicitly validated observation boundary, not a quiet
    /// timer. Current delayed-event backends leave captures unverified.
    func withObservedChanges(_ changed: Bool) -> Self {
        Self(
            rootPath: rootPath, capturedAt: capturedAt, allocatedSize: allocatedSize,
            measuredItemCount: measuredItemCount, unreadableItemCount: unreadableItemCount,
            scanStartedAt: scanStartedAt, scanFinishedAt: scanFinishedAt, scanState: scanState,
            consistency: changed ? .changed : .noObservedChanges,
            measuredObjectCount: measuredObjectCount, unidentifiedItemCount: unidentifiedItemCount
        )
    }
}

nonisolated struct ActivityBaselineService: Sendable {
    typealias ContentsProvider = @Sendable (URL) throws -> [URL]
    typealias IdentityProvider = @Sendable (URL) -> ObjectIdentity?

    /// Scoped to one traversal only: an inode can be reused after unlink, so this
    /// must not become a durable identity without lifecycle evidence.
    struct ObjectIdentity: Hashable, Sendable {
        let device: UInt64
        let inode: UInt64
    }

    private let sizeProvider: StorageAttributionService.SizeProvider
    private let contentsProvider: ContentsProvider
    private let identityProvider: IdentityProvider
    private let isSystemUnderPressure: @Sendable () -> Bool
    private let now: @Sendable () -> Date

    init(
        sizeProvider: @escaping StorageAttributionService.SizeProvider = FileAllocatedSizeProvider.allocatedSize(for:),
        contentsProvider: @escaping ContentsProvider = Self.liveContents(at:),
        identityProvider: @escaping IdentityProvider = Self.liveIdentity(at:),
        isSystemUnderPressure: @escaping @Sendable () -> Bool = Self.liveSystemPressure,
        now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.sizeProvider = sizeProvider
        self.contentsProvider = contentsProvider
        self.identityProvider = identityProvider
        self.isSystemUnderPressure = isSystemUnderPressure
        self.now = now
    }

    func captureBaseline(
        rootPath: URL,
        capturedAt: Date = Date()
    ) async -> ActivityBaselineSnapshot {
        let standardizedRoot = rootPath.standardizedFileURL
        var pending = [standardizedRoot]
        var allocatedSize: Int64 = 0
        var measuredItemCount = 0
        var unreadableItemCount = 0
        var unidentifiedItemCount = 0
        var visitedPaths = Set<String>()
        var measuredObjects = Set<ObjectIdentity>()

        while let url = pending.popLast() {
            guard !Task.isCancelled else { break }
            if visitedPaths.count % 512 == 511 {
                if isSystemUnderPressure() {
                    try? await Task.sleep(for: .milliseconds(500))
                } else {
                    await Task.yield()
                }
                guard !Task.isCancelled else { break }
            }
            let standardizedURL = url.standardizedFileURL
            guard visitedPaths.insert(standardizedURL.path).inserted else { continue }

            if let size = sizeProvider(standardizedURL), size >= 0 {
                measuredItemCount += 1
                let isNewObject: Bool
                if let identity = identityProvider(standardizedURL) {
                    isNewObject = measuredObjects.insert(identity).inserted
                } else {
                    unidentifiedItemCount += 1
                    isNewObject = true
                }
                if isNewObject {
                    let (sum, overflow) = allocatedSize.addingReportingOverflow(size)
                    if overflow {
                        unreadableItemCount += 1
                    } else {
                        allocatedSize = sum
                    }
                }
            } else {
                unreadableItemCount += 1
            }

            do {
                pending.append(contentsOf: try contentsProvider(standardizedURL))
            } catch {
                unreadableItemCount += 1
            }
        }

        return ActivityBaselineSnapshot(
            rootPath: standardizedRoot,
            capturedAt: capturedAt,
            allocatedSize: allocatedSize,
            measuredItemCount: measuredItemCount,
            unreadableItemCount: unreadableItemCount,
            scanStartedAt: capturedAt,
            scanFinishedAt: now(),
            scanState: Task.isCancelled ? .cancelled : (unreadableItemCount == 0 ? .completed : .partial),
            measuredObjectCount: measuredObjects.count,
            unidentifiedItemCount: unidentifiedItemCount
        )
    }

    static func liveSystemPressure() -> Bool {
        let processInfo = ProcessInfo.processInfo
        switch processInfo.thermalState {
        case .serious, .critical:
            return true
        case .nominal, .fair:
            break
        @unknown default:
            break
        }
        return processInfo.isLowPowerModeEnabled
    }

    private static func liveIdentity(at url: URL) -> ObjectIdentity? {
        var metadata = stat()
        let result = url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return Int32(-1) }
            return lstat(path, &metadata)
        }
        guard result == 0 else { return nil }
        return ObjectIdentity(device: UInt64(bitPattern: Int64(metadata.st_dev)), inode: UInt64(metadata.st_ino))
    }

    private static func liveContents(at url: URL) throws -> [URL] {
        // Only real directories are enumerated: regular files would throw here and
        // be miscounted as unreadable, and following symlinks double counts or loops.
        let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard values.isDirectory == true, values.isSymbolicLink != true else { return [] }
        return try FileManager.default.contentsOfDirectory(
            at: url,
            includingPropertiesForKeys: [
                .isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey,
                .totalFileAllocatedSizeKey, .fileAllocatedSizeKey
            ],
            options: []
        )
    }
}

/// A delta between sufficiently observed traversals is an estimate of object
/// allocation changes. It neither reconstructs lost operations nor closes a
/// historical gap. Partial or unverified scans must never fabricate deletions.
nonisolated enum ActivityBaselineReconciler {
    static func recordedByteDelta(
        in events: [DiskActivityEvent],
        after previous: ActivityBaselineSnapshot,
        before current: ActivityBaselineSnapshot
    ) -> Int64 {
        guard let previousFinishedAt = previous.scanFinishedAt,
              let currentStartedAt = current.scanStartedAt else { return 0 }
        return events
            .filter { $0.timestamp > previousFinishedAt && $0.timestamp < currentStartedAt }
            .compactMap(\.byteDelta)
            .reduce(Int64(0), +)
    }

    static func reconciliationEvent(
        previous: ActivityBaselineSnapshot?,
        current: ActivityBaselineSnapshot,
        recordedByteDeltaSincePrevious: Int64 = 0
    ) -> DiskActivityEvent? {
        guard let previous, previous.rootPath == current.rootPath,
              previous.isUsableForReconciliation, current.isUsableForReconciliation,
              let previousFinishedAt = previous.scanFinishedAt,
              let currentStartedAt = current.scanStartedAt,
              let currentFinishedAt = current.scanFinishedAt,
              previousFinishedAt < currentStartedAt else { return nil }
        let (observedDelta, observedOverflow) = current.allocatedSize.subtractingReportingOverflow(previous.allocatedSize)
        let (byteDelta, recordedOverflow) = observedDelta.subtractingReportingOverflow(recordedByteDeltaSincePrevious)
        guard !observedOverflow, !recordedOverflow else { return nil }
        let itemDelta = abs(current.measuredItemCount - previous.measuredItemCount)
        guard byteDelta != 0 || itemDelta != 0 else { return nil }
        return DiskActivityEvent(
            kind: .aggregate,
            path: current.rootPath,
            rootPath: current.rootPath,
            timestamp: currentFinishedAt,
            byteDelta: byteDelta,
            confidence: .estimated,
            previousPath: nil,
            affectedItemCount: max(itemDelta, 1)
        )
    }
}
