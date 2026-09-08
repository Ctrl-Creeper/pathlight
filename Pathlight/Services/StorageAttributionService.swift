import Foundation

struct StorageAttributionService {
    typealias SizeProvider = @Sendable (URL) -> Int64?

    private let options: DiskActivityAggregationOptions
    /// Current allocated size; the live provider also records it for later lookups.
    private let sizeProvider: SizeProvider
    /// Last known size of a path that just vanished; consumed on use.
    private let priorSizeProvider: SizeProvider
    /// Last known size of a path that still exists, read before `sizeProvider`
    /// so a modification reports growth instead of the whole file again.
    private let knownSizeProvider: SizeProvider

    init(
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping SizeProvider,
        priorSizeProvider: @escaping SizeProvider = { _ in nil },
        knownSizeProvider: @escaping SizeProvider = { _ in nil }
    ) {
        self.options = options
        self.sizeProvider = sizeProvider
        self.priorSizeProvider = priorSizeProvider
        self.knownSizeProvider = knownSizeProvider
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        return aggregate(changes.compactMap(event(for:))).filter(isRecordable)
    }

    /// The size filter runs *after* aggregation so a folder's worth of small
    /// changes is judged as one change, and removals skip it entirely: wiping a
    /// thousand tiny files is the thing a threshold should surface, not hide.
    private func isRecordable(_ event: DiskActivityEvent) -> Bool {
        guard let byteDelta = event.byteDelta else {
            return true
        }
        return byteDelta < 0 || event.kind == .deleted || byteDelta >= options.minimumRecordedByteDelta
    }

    private func event(for change: DiskActivityChange) -> DiskActivityEvent? {
        switch change.kind {
        case .created:
            return sizedEvent(kind: .created, change: change, confidence: .confirmed)
        case .modified:
            return sizedEvent(kind: .modified, change: change, confidence: .confirmed)
        case .deleted:
            if let priorSize = priorSizeProvider(change.path) {
                return DiskActivityEvent(
                    kind: .deleted,
                    path: change.path,
                    rootPath: change.rootPath,
                    timestamp: change.timestamp,
                    byteDelta: -priorSize,
                    confidence: .estimated,
                    previousPath: nil,
                    affectedItemCount: 1
                )
            }
            return DiskActivityEvent(
                kind: .deleted,
                path: change.path,
                rootPath: change.rootPath,
                timestamp: change.timestamp,
                byteDelta: nil,
                confidence: .unknown,
                previousPath: nil,
                affectedItemCount: 1
            )
        case .renamed(let previousPath):
            // Consume the departed path's size first so a move inside the root
            // nets to its real growth (usually zero) instead of the whole file.
            let previousKnownSize = previousPath.flatMap(priorSizeProvider)
            let rootPrefix = change.rootPath.standardizedFileURL.path + "/"
            let movedWithinRoot = previousPath?.standardizedFileURL.path.hasPrefix(rootPrefix) == true
            if let size = sizeProvider(change.path) {
                return DiskActivityEvent(
                    kind: .moved,
                    path: change.path,
                    rootPath: change.rootPath,
                    timestamp: change.timestamp,
                    byteDelta: movedWithinRoot ? size - (previousKnownSize ?? 0) : size,
                    confidence: movedWithinRoot && previousKnownSize == nil ? .estimated : .confirmed,
                    previousPath: previousPath,
                    affectedItemCount: 1
                )
            }
            // The path vanished, so this is the departure side of a rename
            // (e.g. into the Trash). Attribute it like a deletion so moved-away
            // bytes are not silently dropped.
            if let priorSize = priorSizeProvider(change.path) {
                return DiskActivityEvent(
                    kind: .moved,
                    path: change.path,
                    rootPath: change.rootPath,
                    timestamp: change.timestamp,
                    byteDelta: -priorSize,
                    confidence: .estimated,
                    previousPath: previousPath,
                    affectedItemCount: 1
                )
            }
            return DiskActivityEvent(
                kind: .moved,
                path: change.path,
                rootPath: change.rootPath,
                timestamp: change.timestamp,
                byteDelta: nil,
                confidence: .unknown,
                previousPath: previousPath,
                affectedItemCount: 1
            )
        }
    }

    private func sizedEvent(
        kind: DiskActivityEventKind,
        change: DiskActivityChange,
        confidence: DiskActivityEventConfidence,
        previousPath: URL? = nil
    ) -> DiskActivityEvent? {
        let knownSize = knownSizeProvider(change.path)
        guard let size = sizeProvider(change.path) else {
            return nil
        }
        // Creation establishes a zero starting point. A first observation of an
        // existing file does not: its current allocation is not evidence of how
        // many bytes this modification added. The provider still records `size`
        // so a subsequent change can be measured against this observation.
        let byteDelta = knownSize.map { size - $0 } ?? (kind == .created ? size : nil)
        return DiskActivityEvent(
            kind: kind,
            path: change.path,
            rootPath: change.rootPath,
            timestamp: change.timestamp,
            byteDelta: byteDelta,
            confidence: byteDelta == nil ? .unknown : confidence,
            previousPath: previousPath,
            affectedItemCount: 1
        )
    }

    private func aggregate(_ events: [DiskActivityEvent]) -> [DiskActivityEvent] {
        guard !options.longTermRecordsFileNames, options.aggregationWindow > 0 else {
            return events
        }

        let groups = Dictionary(grouping: events) { event in
            AggregationKey(event: event, window: options.aggregationWindow)
        }
        return groups.values
            .map(aggregateGroup)
            .sorted { lhs, rhs in
                if lhs.timestamp == rhs.timestamp {
                    return lhs.path.path < rhs.path.path
                }
                return lhs.timestamp < rhs.timestamp
            }
    }

    private func aggregateGroup(_ events: [DiskActivityEvent]) -> DiskActivityEvent {
        guard events.count > 1 else {
            return events[0]
        }

        let sortedEvents = events.sorted { lhs, rhs in
            if lhs.timestamp == rhs.timestamp {
                return lhs.path.path < rhs.path.path
            }
            return lhs.timestamp < rhs.timestamp
        }
        let knownDelta = sortedEvents.compactMap(\.byteDelta).reduce(Int64(0), +)
        let byteDelta = sortedEvents.contains { $0.byteDelta == nil } ? nil : knownDelta
        let confidence: DiskActivityEventConfidence = sortedEvents.allSatisfy { $0.confidence == .confirmed }
            ? .confirmed
            : .estimated

        return DiskActivityEvent(
            kind: .aggregate,
            path: URL(filePath: sortedEvents[0].path.deletingLastPathComponent().path),
            rootPath: sortedEvents[0].rootPath,
            timestamp: sortedEvents[0].timestamp,
            byteDelta: byteDelta,
            confidence: byteDelta == nil ? .unknown : confidence,
            previousPath: nil,
            affectedItemCount: sortedEvents.reduce(0) { $0 + $1.affectedItemCount }
        )
    }

    private struct AggregationKey: Hashable {
        let rootPath: String
        let parentPath: String
        let bucket: Int

        init(event: DiskActivityEvent, window: TimeInterval) {
            rootPath = event.rootPath.standardizedFileURL.path
            parentPath = event.path.deletingLastPathComponent().standardizedFileURL.path
            bucket = Int(floor(event.timestamp.timeIntervalSince1970 / window))
        }
    }
}

enum FileAllocatedSizeProvider {
    nonisolated static func allocatedSize(for url: URL) -> Int64? {
        guard let values = try? url.resourceValues(forKeys: [
            .totalFileAllocatedSizeKey,
            .fileAllocatedSizeKey,
            .isDirectoryKey
        ]) else {
            return nil
        }

        if let totalFileAllocatedSize = values.totalFileAllocatedSize {
            return Int64(totalFileAllocatedSize)
        }
        if let fileAllocatedSize = values.fileAllocatedSize {
            return Int64(fileAllocatedSize)
        }
        // Directories report no allocated size of their own; nil here would make
        // every readable folder count as unreadable in baselines.
        if values.isDirectory == true {
            return 0
        }
        return nil
    }
}
