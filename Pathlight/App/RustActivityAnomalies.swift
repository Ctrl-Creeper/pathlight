import Foundation
import PathlightRustCore

/// Anomaly detection backed by the Rust core, so this app and the Windows and
/// Linux window agree on what is worth interrupting somebody for. The wording
/// stays on this side; only the policy crosses.
nonisolated enum RustActivityAnomalies {
    /// Cached because it is a constant on the other side of the boundary and
    /// this is asked once per finding.
    private static let windowMinutes = Int(activityAnomalyWindowSecs() / 60)

    static func anomalies(
        in events: [DiskActivityEvent],
        rootPath: URL,
        now: Date
    ) -> [ActivityAnomaly] {
        activityAnomalies(events: events.map(coreEvent(for:)), now: now).map { found in
            ActivityAnomaly(
                kind: found.kind == .removal ? .massDeletion : .burst,
                rootPath: rootPath,
                bytes: found.bytes,
                itemCount: Int(found.items),
                windowMinutes: windowMinutes
            )
        }
    }

    private static func coreEvent(for event: DiskActivityEvent) -> ActivityEvent {
        let kind: EventKind = switch event.kind {
        case .created: .created
        case .modified: .modified
        case .deleted: .deleted
        case .moved: .moved
        case .aggregate: .aggregate
        }
        let confidence: Confidence = switch event.confidence {
        case .confirmed: .confirmed
        case .estimated: .estimated
        case .unknown: .unknown
        }
        return ActivityEvent(
            kind: kind,
            path: event.path.path,
            rootPath: event.rootPath.path,
            timestamp: event.timestamp,
            byteDelta: event.byteDelta,
            confidence: confidence,
            previousPath: event.previousPath?.path,
            affectedItemCount: UInt32(clamping: event.affectedItemCount),
            processName: event.processName
        )
    }
}
