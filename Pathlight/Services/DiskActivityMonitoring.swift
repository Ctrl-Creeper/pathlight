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
}
