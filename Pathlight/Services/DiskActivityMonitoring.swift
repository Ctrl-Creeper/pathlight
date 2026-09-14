import Foundation

/// What a watch needs from the operating system: a stream of typed changes
/// with resumable event IDs.
///
/// No implementation ships in this package. FSEvents, inotify, fanotify,
/// ReadDirectoryChangesW and the NTFS change journal all live in the Rust
/// core, each declaring what it can guarantee, and the app injects
/// `RustDiskActivityMonitor`; tests inject a scripted one. A second FSEvents
/// wrapper here would be a second answer to "what did the filesystem do",
/// maintained by whoever noticed the divergence last.
protocol DiskActivityMonitoring: Sendable {
    nonisolated func events(
        for root: URL,
        since eventID: UInt64?,
        latency: TimeInterval
    ) -> AsyncStream<DiskActivityStreamEvent>

    /// Called when a watch cannot be started for a root (for example, the user
    /// revoked Full Disk Access). Without it a failing watch goes quiet with no
    /// explanation. `AppModel` sets this to raise the visible monitoring status,
    /// since the monitor has no UI of its own. Conformers that have nothing to
    /// report (test doubles, hosts without a status surface) get a no-op default.
    var onStartFailure: (@Sendable (Error, URL) -> Void)? { get set }
}

extension DiskActivityMonitoring {
    var onStartFailure: (@Sendable (Error, URL) -> Void)? {
        get { nil }
        set { }
    }
}
