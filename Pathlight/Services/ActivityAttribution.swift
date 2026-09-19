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
    /// One scope's sizes: a sorted table the way the core keeps it (16 bytes a
    /// file, 64 MB for a 4M-file home) plus what the walk has recorded since the
    /// last merge. Lookups read the tail linearly and the table by bisection.
    private struct Table {
        typealias Entry = (key: UInt64, size: Int64)
        var sorted: [Entry] = []
        var pending: [Entry] = []

        // ponytail: merge every 64K records; a lookup scans at most that many
        // linearly (~30 µs), and a 4M-file walk merges about sixty times.
        static let mergeAt = 1 << 16

        func size(for key: UInt64) -> Int64? {
            // A rescan records a path again, so the newest word wins.
            if let hit = pending.last(where: { $0.key == key }) { return hit.size }
            var low = 0
            var high = sorted.count
            while low < high {
                let mid = (low + high) / 2
                if sorted[mid].key < key { low = mid + 1 } else { high = mid }
            }
            return low < sorted.count && sorted[low].key == key ? sorted[low].size : nil
        }

        mutating func merge() {
            let incoming = pending.sorted { $0.key < $1.key }
            pending.removeAll(keepingCapacity: true)
            var merged: [Entry] = []
            merged.reserveCapacity(sorted.count + incoming.count)
            var old = 0
            var new = 0
            while new < incoming.count {
                // Later records of one key replace earlier ones, and any table row.
                if new + 1 < incoming.count, incoming[new + 1].key == incoming[new].key { new += 1; continue }
                while old < sorted.count, sorted[old].key < incoming[new].key { merged.append(sorted[old]); old += 1 }
                if old < sorted.count, sorted[old].key == incoming[new].key { old += 1 }
                merged.append(incoming[new])
                new += 1
            }
            merged.append(contentsOf: sorted[old...])
            sorted = merged
        }
    }

    private let lock = NSLock()
    private var tables: [String: Table] = [:]

    func record(_ size: Int64, for url: URL, scope: String) {
        let key = Self.key(url)
        lock.lock()
        defer { lock.unlock() }
        tables[scope, default: Table()].pending.append((key, size))
    }

    func size(for url: URL, scope: String) -> Int64? {
        let key = Self.key(url)
        lock.lock()
        defer { lock.unlock() }
        guard tables[scope] != nil else { return nil }
        if tables[scope]!.pending.count >= Table.mergeAt { tables[scope]!.merge() }
        return tables[scope]!.size(for: key)
    }

    func forget(scope: String) {
        lock.lock()
        defer { lock.unlock() }
        tables[scope] = nil
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
