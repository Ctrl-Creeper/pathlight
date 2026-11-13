import Foundation
import PathlightRustCore

/// Last-known allocated sizes, so a change can be reported as a delta instead
/// of the whole file again.
///
/// Every entry is namespaced by a `scope`. Two watches over the same file — a
/// whole-disk watch and a folder watch, or a live watch and a background one —
/// are independent observers, and each needs its own baseline. Sharing one
/// entry let whichever watch handled an event first record the new size, so
/// the others measured a delta of zero and dropped the change.
///
/// The table, its file and its compaction are the Rust core's
/// (`SizeIndex` in `core/src/attribution.rs`), shared with the other hosts.
/// What stays here is when to write and whether to seal, which follow this
/// app's preferences.
nonisolated final class ActivitySizeIndex: @unchecked Sendable {
    private let lock = NSLock()
    private let ioQueue = DispatchQueue(label: "com.pathlight.activity-size-index-journal")
    private let lineCodec: ActivityStorageLineCodec
    private let index: SizeIndex
    private var flushScheduled = false

    /// `keepDays` is how long a baseline nobody has touched is worth keeping;
    /// zero is forever. A dropped baseline costs one over-reported delta the
    /// next time that file changes, which is what a file first seen after a
    /// restart already costs.
    init(
        journalURL: URL? = nil,
        lineCodec: ActivityStorageLineCodec = .plaintext,
        compactionEntryThreshold: Int = 10_000,
        keepDays: Int = ActivityStoragePreferences.defaults.detailedRetentionDays
    ) {
        self.lineCodec = lineCodec
        index = journalURL.map { journalURL in
            SizeIndex.open(
                path: journalURL.path,
                // One key for everything Pathlight keeps: the event journal's.
                keyJournal: journalURL
                    .deletingLastPathComponent()
                    .appending(path: "activity-events.jsonl")
                    .path,
                keepDays: UInt32(clamping: max(keepDays, 0)),
                compactAt: UInt32(clamping: max(compactionEntryThreshold, 1))
            )
        } ?? SizeIndex.inMemory()
    }

    static func live(
        lineCodec: ActivityStorageLineCodec = .plaintext,
        keepDays: Int = ActivityStoragePreferences.defaults.detailedRetentionDays
    ) -> ActivitySizeIndex {
        ActivitySizeIndex(journalURL: defaultJournalURL(), lineCodec: lineCodec, keepDays: keepDays)
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
        index.recordSize(scope: scope, path: url.standardizedFileURL.path, size: size)
        scheduleFlush()
        return size
    }

    func knownSize(for url: URL, scope: String = "") -> Int64? {
        index.knownSize(scope: scope, path: url.standardizedFileURL.path)
    }

    func takeKnownSize(for url: URL, scope: String = "") -> Int64? {
        let size = index.takeSize(scope: scope, path: url.standardizedFileURL.path)
        if size != nil {
            scheduleFlush()
        }
        return size
    }

    func compactNow() {
        ioQueue.sync {
            write { try index.compact(encrypt: $0) }
        }
    }

    func flushPendingJournalWrites() {
        ioQueue.sync {
            write { try index.flush(encrypt: $0) }
        }
    }

    /// Forgets one watch's baselines, here and on disk.
    func forget(scope: String) {
        ioQueue.sync {
            write { try index.forgetScope(scope: scope, encrypt: $0) }
        }
    }

    func reset() {
        ioQueue.sync {
            try? index.clear()
        }
    }

    /// One write behind however many changes arrived before it ran: a burst
    /// costs one append rather than one per file.
    private func scheduleFlush() {
        lock.lock()
        let alreadyScheduled = flushScheduled
        flushScheduled = true
        lock.unlock()
        guard !alreadyScheduled else {
            return
        }
        ioQueue.async { [self] in
            lock.lock()
            flushScheduled = false
            lock.unlock()
            write { try index.flush(encrypt: $0) }
        }
    }

    /// A write that fails costs baselines, never history, so it is not
    /// reported: what was recorded stays pending for the next one.
    private func write(_ body: (_ encrypting: Bool) throws -> Void) {
        let encrypting = lineCodec.encryptsNewData
        if encrypting, (try? lineCodec.prepare()) == nil {
            return
        }
        try? body(encrypting)
    }
}
