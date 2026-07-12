import Foundation

struct LiveWatchSessionCoordinator: Sendable {
    private let monitor: any DiskActivityMonitoring

    init(monitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor()) {
        self.monitor = monitor
    }

    func sessions(
        rootPath: URL,
        sinceEventID: UInt64? = nil,
        startedAt: Date = Date(),
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping StorageAttributionService.SizeProvider,
        priorSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil }
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
                    priorSizeProvider: priorSizeProvider
                )
                for await streamEvent in monitor.events(for: standardizedRoot, since: sinceEventID) {
                    guard !Task.isCancelled else {
                        break
                    }

                    switch streamEvent {
                    case let .change(change, eventID):
                        session.record(eventID: eventID)
                        let events = attributionService.process([change])
                        if !events.isEmpty {
                            session.append(events)
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
}
