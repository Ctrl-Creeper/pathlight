import Foundation

struct LiveWatchSessionCoordinator: Sendable {
    private let monitor: any DiskActivityMonitoring
    private let processHints: (any ProcessHinting)?

    /// Minimum spacing between `lsof` snapshots while events are flowing.
    nonisolated static let processHintInterval: TimeInterval = 5
    /// A snapshot older than this no longer annotates events.
    nonisolated static let processHintValidity: TimeInterval = 30

    init(
        monitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor(),
        processHints: (any ProcessHinting)? = nil
    ) {
        self.monitor = monitor
        self.processHints = processHints
    }

    func sessions(
        rootPath: URL,
        sinceEventID: UInt64? = nil,
        startedAt: Date = Date(),
        options: DiskActivityAggregationOptions = .default,
        monitorLatency: TimeInterval = 0.25,
        exclusionFilter: ActivityExclusionFilter? = nil,
        sizeProvider: @escaping StorageAttributionService.SizeProvider,
        priorSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil },
        knownSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil }
    ) -> AsyncStream<WatchSessionModel> {
        let standardizedRoot = rootPath.standardizedFileURL
        return AsyncStream { continuation in
            let task = Task {
                var session = WatchSessionModel(
                    rootPath: standardizedRoot,
                    startedAt: startedAt,
                    lastObservedEventID: sinceEventID,
                    historyState: sinceEventID == nil ? .live : .catchingUp
                )
                continuation.yield(session)

                let attributionService = StorageAttributionService(
                    options: options,
                    sizeProvider: sizeProvider,
                    priorSizeProvider: priorSizeProvider,
                    knownSizeProvider: knownSizeProvider
                )
                var hintCache: [String: String] = [:]
                var lastHintSnapshotAt: Date?
                for await streamEvent in monitor.events(for: standardizedRoot, since: sinceEventID, latency: monitorLatency) {
                    guard !Task.isCancelled else {
                        break
                    }
                    session.clearLatestChanges()
                    if case .requiresRescan = streamEvent {
                        session.recordStreamEvent(isGap: true)
                    } else {
                        session.recordStreamEvent(isGap: false)
                    }

                    switch streamEvent {
                    case let .change(change, eventID):
                        // Excluded changes still advance the cursor so
                        // checkpoints move past them.
                        session.record(eventID: eventID)
                        if exclusionFilter?.excludes(change.path) != true {
                            var events = attributionService.process([change])
                            if !events.isEmpty {
                                if let processHints,
                                   Self.shouldSnapshotProcesses(
                                       for: events,
                                       options: options,
                                       lastSnapshotAt: lastHintSnapshotAt
                                   ) {
                                    lastHintSnapshotAt = Date()
                                    hintCache = await processHints.openPaths(under: standardizedRoot)
                                }
                                if let lastHintSnapshotAt,
                                   Date().timeIntervalSince(lastHintSnapshotAt) <= Self.processHintValidity {
                                    events = ProcessHintMatcher.annotate(events, hints: hintCache, rootPath: standardizedRoot)
                                }
                                session.append(
                                    events,
                                    coalescingWindow: options.isDetailedFileTimeline ? 1 : 0
                                )
                            }
                        }
                    case let .historyCaughtUp(eventID):
                        session.record(eventID: eventID)
                        session.setHistoryState(.live)
                    case let .requiresRescan(eventID):
                        session.record(eventID: eventID)
                        session.setHistoryState(.gapDetected)
                    }
                    continuation.yield(session)
                }

                continuation.finish()
            }

            continuation.onTermination = { @Sendable _ in
                task.cancel()
            }
        }
    }

    nonisolated static func shouldSnapshotProcesses(
        for events: [DiskActivityEvent],
        options: DiskActivityAggregationOptions,
        lastSnapshotAt: Date?,
        now: Date = Date()
    ) -> Bool {
        if let lastSnapshotAt, now.timeIntervalSince(lastSnapshotAt) < processHintInterval {
            return false
        }
        // Only an interactive watch can attribute a process: by the time a
        // background watch's latency window closes, the writer has exited and
        // the snapshot would either name nothing or name the wrong thing.
        return options.isDetailedFileTimeline
    }
}
