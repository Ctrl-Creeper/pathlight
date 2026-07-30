import Foundation

nonisolated final class ActivitySizeIndex: @unchecked Sendable {
    private let lock = NSLock()
    private let ioQueue = DispatchQueue(label: "com.pathlight.activity-size-index-journal")
    private let journalURL: URL?
    private let lineCodec: ActivityStorageLineCodec
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private let compactionEntryThreshold: Int
    private var sizesByPath: [String: Int64]
    private var journalEntryCount: Int

    init(
        journalURL: URL? = nil,
        lineCodec: ActivityStorageLineCodec = .plaintext,
        compactionEntryThreshold: Int = 10_000
    ) {
        self.journalURL = journalURL
        self.lineCodec = lineCodec
        self.compactionEntryThreshold = max(compactionEntryThreshold, 1)

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        self.encoder = encoder

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        self.decoder = decoder
        let loadedState = Self.loadState(from: journalURL, lineCodec: lineCodec, decoder: decoder)
        sizesByPath = loadedState.sizes
        journalEntryCount = loadedState.entryCount
    }

    static func live(
        lineCodec: ActivityStorageLineCodec = .plaintext
    ) -> ActivitySizeIndex {
        ActivitySizeIndex(journalURL: defaultJournalURL(), lineCodec: lineCodec)
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
    func recordKnownSize(_ size: Int64?, for url: URL) -> Int64? {
        guard let size else {
            return nil
        }

        lock.lock()
        defer {
            lock.unlock()
        }

        let key = key(for: url)
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

    func knownSize(for url: URL) -> Int64? {
        lock.lock()
        defer {
            lock.unlock()
        }
        return sizesByPath[key(for: url)]
    }

    func takeKnownSize(for url: URL) -> Int64? {
        lock.lock()
        defer {
            lock.unlock()
        }

        let key = key(for: url)
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
            compact()
        }
    }

    func flushPendingJournalWrites() {
        ioQueue.sync {}
    }

    private func key(for url: URL) -> String {
        url.standardizedFileURL.path
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

        lock.lock()
        let snapshot = sizesByPath
        lock.unlock()

        do {
            try ActivityStorageFileProtection.createProtectedDirectory(
                at: journalURL.deletingLastPathComponent()
            )

            let entries = snapshot
                .sorted { lhs, rhs in lhs.key < rhs.key }
                .map { path, size in
                    JournalEntry(
                        kind: .record,
                        path: path,
                        size: size,
                        recordedAt: Date()
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
        } catch {
            // Keep appending to the journal; a later compaction can try again.
        }
    }

    private nonisolated static func loadState(
        from journalURL: URL?,
        lineCodec: ActivityStorageLineCodec,
        decoder: JSONDecoder
    ) -> LoadedState {
        guard let journalURL,
              FileManager.default.fileExists(atPath: journalURL.path),
              let data = try? Data(contentsOf: journalURL) else {
            return LoadedState(sizes: [:], entryCount: 0)
        }

        let contents = String(decoding: data, as: UTF8.self)
        var entryCount = 0
        let sizes = contents
            .split(separator: "\n", omittingEmptySubsequences: true)
            .reduce(into: [String: Int64]()) { sizes, line in
                guard let data = try? lineCodec.decode(line),
                      let entry = try? decoder.decode(JournalEntry.self, from: data) else {
                    return
                }
                entryCount += 1

                switch entry.kind {
                case .record:
                    if let size = entry.size {
                        sizes[entry.path] = size
                    }
                case .remove:
                    sizes.removeValue(forKey: entry.path)
                }
            }

        return LoadedState(sizes: sizes, entryCount: entryCount)
    }

    private nonisolated struct LoadedState {
        let sizes: [String: Int64]
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
