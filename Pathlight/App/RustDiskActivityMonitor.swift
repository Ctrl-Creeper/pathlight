import Foundation
import PathlightRustCore

/// Event source backed by the Rust core (FSEvents on macOS), so macOS and the
/// other platforms share one monitoring implementation.
nonisolated final class RustDiskActivityMonitor: DiskActivityMonitoring {
    nonisolated func events(
        for root: URL,
        since eventID: UInt64?,
        latency: TimeInterval
    ) -> AsyncStream<DiskActivityStreamEvent> {
        let standardizedRoot = root.standardizedFileURL
        return AsyncStream { continuation in
            let listener = RustActivityListener(root: standardizedRoot, continuation: continuation)
            let watcher: Watcher
            do {
                watcher = try Watcher.start(
                    rootPath: standardizedRoot.path,
                    sinceEventId: eventID,
                    latencyMs: UInt64(max(latency, 0) * 1_000),
                    listener: listener
                )
            } catch {
                continuation.finish()
                return
            }
            continuation.onTermination = { @Sendable _ in
                watcher.stop()
            }
        }
    }
}

private nonisolated final class RustActivityListener: ActivityListener, @unchecked Sendable {
    private let root: URL
    private let continuation: AsyncStream<DiskActivityStreamEvent>.Continuation

    init(root: URL, continuation: AsyncStream<DiskActivityStreamEvent>.Continuation) {
        self.root = root
        self.continuation = continuation
    }

    func onEvent(event: StreamEvent) {
        switch event {
        case let .change(change, eventId):
            continuation.yield(
                .change(
                    DiskActivityChange(
                        kind: Self.kind(for: change.kind),
                        path: URL(filePath: change.path),
                        rootPath: root,
                        timestamp: change.timestamp
                    ),
                    eventID: eventId
                )
            )
        case let .historyCaughtUp(eventId):
            continuation.yield(.historyCaughtUp(eventID: eventId))
        case let .requiresRescan(eventId):
            continuation.yield(.requiresRescan(eventID: eventId))
        }
    }

    private static func kind(for kind: ChangeKind) -> DiskActivityChange.Kind {
        switch kind {
        case .created:
            return .created
        case .modified:
            return .modified
        case .deleted:
            return .deleted
        case let .renamed(previousPath):
            return .renamed(previousPath: previousPath.map { URL(filePath: $0) })
        }
    }
}
