import Foundation

struct LiveWatchSessionCoordinator: Sendable {
    private let monitor: any DiskActivityMonitoring

    init(monitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor()) {
        self.monitor = monitor
    }

    func sessions(
        rootPath: URL,
        startedAt: Date = Date(),
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping StorageAttributionService.SizeProvider,
        priorSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil }
    ) -> AsyncStream<WatchSessionModel> {
        let standardizedRoot = rootPath.standardizedFileURL
        return AsyncStream { continuation in
            let task = Task {
                var session = WatchSessionModel(rootPath: standardizedRoot, startedAt: startedAt)
                continuation.yield(session)

                let attributionService = StorageAttributionService(
                    options: options,
                    sizeProvider: sizeProvider,
                    priorSizeProvider: priorSizeProvider
                )
                for await change in monitor.changes(for: standardizedRoot) {
                    guard !Task.isCancelled else {
                        break
                    }

                    let events = attributionService.process([change])
                    guard !events.isEmpty else {
                        continue
                    }

                    session.append(events)
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
