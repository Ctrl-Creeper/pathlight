import Foundation

/// One size lookup for one watch.
typealias ActivitySizeProvider = @Sendable (URL) -> Int64?

/// The three byte-attribution lookups one watch needs, already scoped to it.
nonisolated struct ActivitySizeProviders: Sendable {
    /// Current allocated size; the live provider also records it for later lookups.
    var size: ActivitySizeProvider
    /// Last known size of a path that just vanished; consumed on use.
    var prior: ActivitySizeProvider = { _ in nil }
    /// Last known size of a path that still exists, read before `size` so a
    /// modification reports growth instead of the whole file again.
    var known: ActivitySizeProvider = { _ in nil }

    /// Stable scope for a watch: live and background watches over one root are
    /// separate observers, so they must not share a baseline either.
    nonisolated static func scope(kind: String, rootPath: URL) -> String {
        "\(kind):\(rootPath.standardizedFileURL.path)"
    }
}

/// Turns observed changes into recordable events with byte deltas.
///
/// Declared here and implemented in the Rust core, which every host shares —
/// two implementations of "how many bytes changed" eventually disagree, and the
/// journal both hosts read would then hold both answers. This package does not
/// link the core, so `swift test` needs no xcframework; the app target injects
/// `RustActivityAttribution`, the way it already injects its monitor.
protocol ActivityAttributing: Sendable {
    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent]
}

/// Builds the attributor one watch needs. A factory rather than an instance
/// because the options belong to the watch — and a long-term target's options
/// can change between reconnects.
typealias ActivityAttributionFactory = @Sendable (
    DiskActivityAggregationOptions,
    ActivitySizeProviders
) -> any ActivityAttributing

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
