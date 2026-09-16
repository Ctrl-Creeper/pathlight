import Foundation

/// Last-known allocated sizes, so a change can be reported as a delta instead
/// of the whole file again.
///
/// Every entry is namespaced by a `scope`. Two watches over the same file — a
/// whole-disk watch and a folder watch, or a live watch and a background one —
/// are independent observers, and each needs its own baseline. Sharing one
/// entry let whichever watch handled an event first record the new size, so
/// the others measured a delta of zero and dropped the change.
nonisolated final class ActivitySizeIndex: @unchecked Sendable {
    private let lock = NSLock()
    private let ioQueue = DispatchQueue(label: "com.pathlight.activity-size-index-journal")
    private let journalURL: URL?
    private let lineCodec: ActivityStorageLineCodec
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private let compactionEntryThreshold: Int
    /// How long a baseline nobody has touched is worth keeping. Zero is
    /// forever, as it is everywhere else. Without it this is the one thing
    /// here that only grows: one entry per file ever changed, reloaded at
    /// every launch, for the life of the machine. A dropped baseline costs
    /// one over-reported delta the next time that file changes, which is what
    /// a file first seen after a restart already costs.
    private let keepDays: Int
    private var sizesByPath: [String: Int64]
    private var journalEntryCount: Int

    init(
        journalURL: URL? = nil,
        lineCodec: ActivityStorageLineCodec = .plaintext,
        compactionEntryThreshold: Int = 10_000,
        keepDays: Int = ActivityStoragePreferences.defaults.detailedRetentionDays
    ) {
        self.journalURL = journalURL
        self.lineCodec = lineCodec
        self.compactionEntryThreshold = max(compactionEntryThreshold, 1)
        self.keepDays = keepDays

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        self.encoder = encoder

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        self.decoder = decoder
        let loadedState = Self.loadState(
            from: journalURL,
            lineCodec: lineCodec,
            decoder: decoder,
            cutoff: Self.cutoff(keepDays: keepDays)
        )
        sizesByPath = loadedState.baselines.mapValues(\.size)
        journalEntryCount = loadedState.entryCount
    }

    static func live(
        lineCodec: ActivityStorageLineCodec = .plaintext,
        keepDays: Int = ActivityStoragePreferences.defaults.detailedRetentionDays
    ) -> ActivitySizeIndex {
        ActivitySizeIndex(journalURL: defaultJournalURL(), lineCodec: lineCodec, keepDays: keepDays)
    }

    /// When a baseline stops being worth its memory. `nil` is forever.
    private static func cutoff(keepDays: Int) -> Date? {
        guard keepDays > 0 else {
            return nil
        }
        return Date(timeIntervalSinceNow: -Double(keepDays) * 86_400)
    }

    nonisolated static func defaultJournalURL() -> URL {
        let baseURL = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.temporaryDirectory

        return baseURL
            .appending(path: "Pathlight", directoryHint: .isDirectory)
            .appending(path: "activity-size-index.jsonl")
    }

    @discardableResult
    func recordKnownSize(_ size: Int64?, for url: URL, scope: String = "") -> Int64? {
        guard let size else {
            return nil
        }

        lock.lock()
        defer {
            lock.unlock()
        }

        let key = key(for: url, scope: scope)
        sizesByPath[key] = size
        enqueueJournalWrite(
            JournalEntry(
                kind: .record,
                path: key,
                size: size,
                recordedAt: Date()
            )
        )
        return size
    }

    func knownSize(for url: URL, scope: String = "") -> Int64? {
        lock.lock()
        defer {
            lock.unlock()
        }
        return sizesByPath[key(for: url, scope: scope)]
    }

    func takeKnownSize(for url: URL, scope: String = "") -> Int64? {
        lock.lock()
        defer {
            lock.unlock()
        }

        let key = key(for: url, scope: scope)
        guard let size = sizesByPath.removeValue(forKey: key) else {
            return nil
        }
        enqueueJournalWrite(
            JournalEntry(
                kind: .remove,
                path: key,
                size: nil,
                recordedAt: Date()
            )
        )
        return size
    }

    func compactNow() {
        ioQueue.sync {
            withStorageLock {
                compact()
            }
        }
    }

    func flushPendingJournalWrites() {
        ioQueue.sync {}
    }

    func reset() {
        ioQueue.sync {
            withStorageLock {
                lock.lock()
                sizesByPath.removeAll()
                lock.unlock()
                journalEntryCount = 0
                if let journalURL,
                   FileManager.default.fileExists(atPath: journalURL.path) {
                    try? FileManager.default.removeItem(at: journalURL)
                }
            }
        }
    }

    // A newline cannot appear in a scope, so no scope can spell another one's key.
    private func key(for url: URL, scope: String) -> String {
        scope.isEmpty ? url.standardizedFileURL.path : scope + "\n" + url.standardizedFileURL.path
    }

    // Enqueued while holding `lock`, so journal order matches dictionary mutation order.
    // ponytail: pending writes drop on process exit; the journal is a rebuildable cache.
    private func enqueueJournalWrite(_ entry: JournalEntry) {
        guard journalURL != nil else {
            return
        }
        ioQueue.async { [self] in
            writeJournalEntry(entry)
        }
    }

    // Runs on ioQueue only; `journalEntryCount` and compaction are ioQueue-confined.
    private func writeJournalEntry(_ entry: JournalEntry) {
        withStorageLock {
            writeJournalEntryWhileLocked(entry)
        }
    }

    private func writeJournalEntryWhileLocked(_ entry: JournalEntry) {
        guard let journalURL else {
            return
        }

        do {
            try ActivityStorageFileProtection.createProtectedDirectory(
                at: journalURL.deletingLastPathComponent()
            )
            try ActivityStorageFileProtection.ensureProtectedFile(at: journalURL)

            let data = try encoder.encode(entry)
            let handle = try FileHandle(forWritingTo: journalURL)
            defer {
                try? handle.close()
            }
            try handle.seekToEnd()
            let encodedLine = try lineCodec.encode(data)
            try handle.write(contentsOf: Data((encodedLine + "\n").utf8))
            try ActivityStorageFileProtection.applyProtectedFilePermissions(to: journalURL)
            journalEntryCount += 1
            compactIfNeeded()
        } catch {
            // The in-memory index remains useful even when persistence fails.
        }
    }

    private func compactIfNeeded() {
        guard journalEntryCount >= compactionEntryThreshold else {
            return
        }
        compact()
    }

    private func compact() {
        guard let journalURL else {
            return
        }

        do {
            try ActivityStorageFileProtection.createProtectedDirectory(
                at: journalURL.deletingLastPathComponent()
            )

            // A CLI or second window may have appended since this instance
            // loaded. ioQueue has already drained this instance's writes, so
            // the locked file is the complete, freshest compaction source.
            let snapshot = Self.loadState(
                from: journalURL,
                lineCodec: lineCodec,
                decoder: decoder,
                cutoff: Self.cutoff(keepDays: keepDays)
            ).baselines

            let entries = snapshot
                .sorted { lhs, rhs in lhs.key < rhs.key }
                .map { path, baseline in
                    JournalEntry(
                        kind: .record,
                        path: path,
                        size: baseline.size,
                        // The stamp this rewrite found, not the time of the
                        // rewrite: restamping would make every survivor look
                        // new and nothing would ever age out.
                        recordedAt: baseline.recordedAt
                    )
                }
            let payload = try entries
                .map { entry in
                    let data = try encoder.encode(entry)
                    return try lineCodec.encode(data)
                }
                .joined(separator: "\n")

            if payload.isEmpty {
                try Data().write(to: journalURL, options: .atomic)
            } else {
                try Data((payload + "\n").utf8).write(to: journalURL, options: .atomic)
            }
            try ActivityStorageFileProtection.applyProtectedFilePermissions(to: journalURL)
            journalEntryCount = 0
            // The rewrite is where what aged out goes, from memory as well as
            // from the file. ioQueue has already drained this instance's
            // writes, so anything missing from the snapshot is gone for good.
            lock.lock()
            sizesByPath = sizesByPath.filter { path, _ in snapshot[path] != nil }
            lock.unlock()
        } catch {
            // Keep appending to the journal; a later compaction can try again.
        }
    }

    private func withStorageLock(_ operation: () -> Void) {
        guard let journalURL else {
            operation()
            return
        }
        try? ActivityStorageFileProtection.withStorageLock(
            in: journalURL.deletingLastPathComponent(),
            operation
        )
    }

    private nonisolated static func loadState(
        from journalURL: URL?,
        lineCodec: ActivityStorageLineCodec,
        decoder: JSONDecoder,
        cutoff: Date?
    ) -> LoadedState {
        guard let journalURL,
              FileManager.default.fileExists(atPath: journalURL.path),
              let data = try? Data(contentsOf: journalURL) else {
            return LoadedState(baselines: [:], entryCount: 0)
        }

        let contents = String(decoding: data, as: UTF8.self)
        var entryCount = 0
        let baselines = contents
            .split(separator: "\n", omittingEmptySubsequences: true)
            .reduce(into: [String: Baseline]()) { baselines, line in
                guard let data = try? lineCodec.decode(line),
                      let entry = try? decoder.decode(JournalEntry.self, from: data) else {
                    return
                }
                entryCount += 1

                switch entry.kind {
                case .record:
                    guard let size = entry.size else {
                        return
                    }
                    if let cutoff, entry.recordedAt < cutoff {
                        baselines.removeValue(forKey: entry.path)
                    } else {
                        baselines[entry.path] = Baseline(size: size, recordedAt: entry.recordedAt)
                    }
                case .remove:
                    baselines.removeValue(forKey: entry.path)
                }
            }

        return LoadedState(baselines: baselines, entryCount: entryCount)
    }

    /// A baseline as the file holds it. Only the size is kept in memory between
    /// compactions; the stamp is what the rewrite has to carry forward.
    private nonisolated struct Baseline {
        let size: Int64
        let recordedAt: Date
    }

    private nonisolated struct LoadedState {
        let baselines: [String: Baseline]
        let entryCount: Int
    }

    private struct JournalEntry: Codable {
        enum Kind: String, Codable {
            case record
            case remove
        }

        let kind: Kind
        let path: String
        let size: Int64?
        let recordedAt: Date
    }
}
