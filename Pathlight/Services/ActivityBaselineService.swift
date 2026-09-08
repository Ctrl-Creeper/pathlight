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
    /// Items inside the root whose bytes can be changed from outside it without
    /// any event for a path under the root: symlinks pointing elsewhere, and
    /// hard links whose other names are not in this folder. FSEvents reports
    /// paths, so a write through the outside name is invisible here.
    let unobservableLinkCount: Int?

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
        unidentifiedItemCount: Int? = nil,
        unobservableLinkCount: Int? = nil
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
        self.unobservableLinkCount = unobservableLinkCount
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
        unobservableLinkCount = try container.decodeIfPresent(Int.self, forKey: .unobservableLinkCount)
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
            measuredObjectCount: measuredObjectCount, unidentifiedItemCount: unidentifiedItemCount,
            unobservableLinkCount: unobservableLinkCount
        )
    }
}

nonisolated struct ActivityBaselineService: Sendable {
    typealias ContentsProvider = @Sendable (URL) throws -> [URL]
    typealias MeasurementProvider = @Sendable (URL) -> Measurement

    /// Scoped to one traversal only: an inode can be reused after unlink, so this
    /// must not become a durable identity without lifecycle evidence.
    struct ObjectIdentity: Hashable, Sendable {
        let device: UInt64
        let inode: UInt64
    }

    /// One native metadata observation. Keeping allocation and identity in the
    /// same value prevents a replacement between two path lookups from joining
    /// the old object's bytes to the new object's identity.
    struct Measurement: Sendable {
        let allocatedSize: Int64?
        let identity: ObjectIdentity?
        /// How many names this object has. More than one means a write through
        /// another name changes these bytes; if that name is outside the root,
        /// no event ever mentions a path under the root.
        var linkCount: UInt64 = 1
        var isSymbolicLink: Bool = false
    }

    private let measurementProvider: MeasurementProvider
    private let contentsProvider: ContentsProvider
    private let isSystemUnderPressure: @Sendable () -> Bool
    private let now: @Sendable () -> Date

    init(
        measurementProvider: @escaping MeasurementProvider = Self.liveMeasurement(at:),
        contentsProvider: @escaping ContentsProvider = Self.liveContents(at:),
        isSystemUnderPressure: @escaping @Sendable () -> Bool = Self.liveSystemPressure,
        now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.measurementProvider = measurementProvider
        self.contentsProvider = contentsProvider
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
        // Only multi-named objects are tracked, so this stays empty on the
        // overwhelming majority of folders.
        var namesSeenPerLinkedObject: [ObjectIdentity: (expected: UInt64, seen: UInt64)] = [:]
        var escapingSymlinkCount = 0

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

            let measurement = measurementProvider(standardizedURL)
            if measurement.isSymbolicLink {
                if !Self.isUnder(standardizedRoot, standardizedURL.resolvingSymlinksInPath()) {
                    escapingSymlinkCount += 1
                }
            } else if measurement.linkCount > 1, let identity = measurement.identity {
                let seen = (namesSeenPerLinkedObject[identity]?.seen ?? 0) + 1
                namesSeenPerLinkedObject[identity] = (measurement.linkCount, seen)
            }

            if let size = measurement.allocatedSize, size >= 0 {
                measuredItemCount += 1
                let isNewObject: Bool
                if let identity = measurement.identity {
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

        // A traversal that could not read everything has not seen every name of
        // a hard link either, so only a complete scan may conclude a name is
        // outside the root. Escaping symlinks need no such caveat.
        let scanState: ActivityBaselineScanState = Task.isCancelled
            ? .cancelled
            : (unreadableItemCount == 0 ? .completed : .partial)
        let externalHardLinkCount = scanState == .completed
            ? namesSeenPerLinkedObject.values.count(where: { $0.seen < $0.expected })
            : 0

        return ActivityBaselineSnapshot(
            rootPath: standardizedRoot,
            capturedAt: capturedAt,
            allocatedSize: allocatedSize,
            measuredItemCount: measuredItemCount,
            unreadableItemCount: unreadableItemCount,
            scanStartedAt: capturedAt,
            scanFinishedAt: now(),
            scanState: scanState,
            measuredObjectCount: measuredObjects.count,
            unidentifiedItemCount: unidentifiedItemCount,
            unobservableLinkCount: escapingSymlinkCount + externalHardLinkCount
        )
    }

    /// Whether `url` is the root or somewhere inside it. `/` contains
    /// everything, and appending a separator to it would spell `//`.
    static func isUnder(_ root: URL, _ url: URL) -> Bool {
        let rootPath = root.standardizedFileURL.path
        let path = url.standardizedFileURL.path
        return rootPath == "/" || path == rootPath || path.hasPrefix(rootPath + "/")
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

    private static func liveMeasurement(at url: URL) -> Measurement {
        var metadata = stat()
        let result = url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return Int32(-1) }
            return lstat(path, &metadata)
        }
        guard result == 0 else {
            return Measurement(allocatedSize: nil, identity: nil)
        }
        let isDirectory = metadata.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR)
        let allocation = metadata.st_blocks.multipliedReportingOverflow(by: 512)
        // Directories always report more than one name (`.` and `..`), so only
        // files can say anything useful about hard links here.
        return Measurement(
            allocatedSize: isDirectory || !allocation.overflow ? (isDirectory ? 0 : allocation.partialValue) : nil,
            identity: ObjectIdentity(
                device: UInt64(bitPattern: Int64(metadata.st_dev)),
                inode: UInt64(metadata.st_ino)
            ),
            linkCount: isDirectory ? 1 : UInt64(metadata.st_nlink),
            isSymbolicLink: metadata.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK)
        )
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
