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

/// What every file held when a watch's folder was last walked, for the two
/// questions a live change cannot answer on its own: how big was the file
/// that just vanished, and how big was this one before it was modified. The
/// size index knows files the watch has already touched; this knows the rest.
///
/// ponytail: one dictionary per scope keyed by path hash, 16 bytes a file plus
/// dictionary overhead; the core keeps the same table for the other hosts.
nonisolated final class ActivityBaselineSizes: @unchecked Sendable {
    private let lock = NSLock()
    private var sizes: [String: [UInt64: Int64]] = [:]

    func record(_ size: Int64, for url: URL, scope: String) {
        let key = Self.key(url)
        lock.lock()
        defer { lock.unlock() }
        sizes[scope, default: [:]][key] = size
    }

    func size(for url: URL, scope: String) -> Int64? {
        let key = Self.key(url)
        lock.lock()
        defer { lock.unlock() }
        return sizes[scope]?[key]
    }

    func forget(scope: String) {
        lock.lock()
        defer { lock.unlock() }
        sizes[scope] = nil
    }

    /// The core rebases every change onto the root the watch was asked for,
    /// so the walk and the events already spell a path the same way.
    private static func key(_ url: URL) -> UInt64 {
        hash(url.standardizedFileURL.path)
    }

    /// FNV-1a, the hash the core's table uses too.
    private static func hash(_ text: String) -> UInt64 {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return hash
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
