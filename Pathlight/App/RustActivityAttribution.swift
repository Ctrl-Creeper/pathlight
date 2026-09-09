import Foundation
import PathlightRustCore

/// Byte attribution backed by the Rust core, so this app and the Windows and
/// Linux window answer "how many bytes changed" with the same code — and a
/// journal both of them read holds one answer rather than two.
///
/// The size index stays on this side of the boundary: it is persistent and
/// optionally encrypted, so a long-term watch still has baselines after a
/// relaunch. Only the arithmetic crosses.
nonisolated final class RustActivityAttribution: ActivityAttributing, @unchecked Sendable {
    private let attributor: ActivityAttributor

    init(options: DiskActivityAggregationOptions, sizeProviders: ActivitySizeProviders) {
        attributor = ActivityAttributor(
            options: AggregationOptions(
                minimumRecordedByteDelta: options.minimumRecordedByteDelta,
                aggregationWindowSecs: UInt64(max(options.aggregationWindow, 0)),
                recordsFileNames: options.longTermRecordsFileNames
            ),
            sizes: RustSizeLookup(providers: sizeProviders)
        )
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        attributor
            .process(changes: changes.map(Self.change(for:)))
            .map(Self.event(for:))
    }

    private static func change(for change: DiskActivityChange) -> Change {
        let kind: ChangeKind = switch change.kind {
        case .created: .created
        case .modified: .modified
        case .deleted: .deleted
        case let .renamed(previousPath): .renamed(previousPath: previousPath?.path)
        }
        return Change(
            kind: kind,
            path: change.path.path,
            rootPath: change.rootPath.path,
            timestamp: change.timestamp,
            // FSEvents never says who wrote; a privileged Linux watch does,
            // and this bridge is macOS-only.
            processName: nil
        )
    }

    private static func event(for event: ActivityEvent) -> DiskActivityEvent {
        let kind: DiskActivityEventKind = switch event.kind {
        case .created: .created
        case .modified: .modified
        case .deleted: .deleted
        case .moved: .moved
        case .aggregate: .aggregate
        }
        let confidence: DiskActivityEventConfidence = switch event.confidence {
        case .confirmed: .confirmed
        case .estimated: .estimated
        case .unknown: .unknown
        }
        return DiskActivityEvent(
            kind: kind,
            path: URL(filePath: event.path),
            rootPath: URL(filePath: event.rootPath),
            timestamp: event.timestamp,
            byteDelta: event.byteDelta,
            confidence: confidence,
            previousPath: event.previousPath.map { URL(filePath: $0) },
            affectedItemCount: Int(event.affectedItemCount),
            processName: event.processName
        )
    }
}

/// The watch's own size providers, reached from the core.
private nonisolated final class RustSizeLookup: SizeLookup, @unchecked Sendable {
    private let providers: ActivitySizeProviders

    init(providers: ActivitySizeProviders) {
        self.providers = providers
    }

    func size(path: String) -> Int64? {
        providers.size(URL(filePath: path))
    }

    func priorSize(path: String) -> Int64? {
        providers.prior(URL(filePath: path))
    }

    func knownSize(path: String) -> Int64? {
        providers.known(URL(filePath: path))
    }
}
