import Foundation
import PathlightRustCore

nonisolated enum ActivityEventStoreError: Error {
    /// The store could not prove whether part of a failed batch reached disk.
    /// Replaying into the same journal could duplicate evidence.
    case commitStateUnknown
}

/// Pathlight's own storage, held out of every watch that can reach it.
///
/// Recording a change to the journal appends a row to the journal, and that
/// append is another change: on a live watch the event journal and the size
/// index push each other around at the stream's latency for as long as the
/// watch runs, and every round trip also records a timeline event the user
/// never caused. The byte threshold does not stop it, because attribution
/// writes the size index before any threshold is applied.
///
/// This is deliberately not part of `ActivityExclusionFilter`: that filter is
/// a user setting, and watching the home folder or the whole disk is a normal
/// thing to ask for. So this exclusion is not negotiable and not editable.
nonisolated enum ActivityStorageIsolation {
    /// The directory holding the event journal, the size index, and the
    /// temporary files their rewrites go through.
    static let directoryPath: String = JSONLActivityEventStore.defaultJournalURL()
        .deletingLastPathComponent()
        .resolvingSymlinksInPath()
        .standardizedFileURL
        .path

    static func excludes(_ url: URL) -> Bool {
        excludes(url.standardizedFileURL.path, storageDirectory: directoryPath)
    }

    /// A sibling whose name merely starts the same way — `Pathlight-backup` —
    /// is somebody else's folder, so the separator is part of the match.
    static func excludes(_ path: String, storageDirectory: String) -> Bool {
        path == storageDirectory || path.hasPrefix(storageDirectory + "/")
    }
}

nonisolated protocol ActivityEventStoring: Sendable {
    /// Returns only after the whole batch is durably committed. A thrown error
    /// is retryable unless it is `commitStateUnknown`.
    func append(_ events: [DiskActivityEvent]) async throws
    /// The newest `limit` events for the root, newest first.
    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent]
    /// One page of the rows `query` matched, plus totals over *every* matching
    /// row retained for the root, so a capped page can never understate the
    /// dashboard and a search box can never overstate what it found.
    func loadEventPage(
        rootPath: URL,
        limit: Int,
        bucketInterval: TimeInterval,
        query: ActivityHistoryQuery
    ) async throws -> ActivityEventPage
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws
    /// Deletes every row recorded for the root, grouped rows included, so a
    /// folder added back starts a fresh history. Mirrors `Journal::forget_root`.
    func removeEvents(rootPath: URL) async throws
}

extension ActivityEventStoring {
    func removeEvents(rootPath: URL) async throws {}

    /// Stores without their own single-pass reader still answer correctly; they
    /// just pay for materializing every event first.
    func loadEventPage(
        rootPath: URL,
        limit: Int,
        bucketInterval: TimeInterval,
        query: ActivityHistoryQuery = .everything
    ) async throws -> ActivityEventPage {
        ActivityEventPage(
            historyOfEvents(
                rootPath: rootPath.standardizedFileURL.path,
                events: try await loadEvents(rootPath: rootPath, limit: .max).map(\.coreEvent),
                bucketIntervalSecs: UInt64(max(bucketInterval, 1)),
                recentLimit: UInt32(clamping: max(limit, 0)),
                query: query.coreQuery,
                generatedAt: Date()
            )
        )
    }
}

/// The app's journal. The file, its line format, retention and the storage
/// lock are the Rust core's (`core/src/journal.rs`), shared with the other
/// hosts; what stays here is what only this app knows: where its key comes
/// from, whether new rows are sealed, and when a trim is not due yet.
actor JSONLActivityEventStore: ActivityEventStoring {
    private let journalURL: URL
    private let lineCodec: ActivityStorageLineCodec
    private var hasScannedJournal = false
    private var oldestKnownEventTimestamp: Date?
    private var requiresReopenAfterFailedRollback = false

    init(
        journalURL: URL,
        lineCodec: ActivityStorageLineCodec = .plaintext
    ) {
        self.journalURL = journalURL
        self.lineCodec = lineCodec
    }

    static func live(
        lineCodec: ActivityStorageLineCodec = .plaintext
    ) -> JSONLActivityEventStore {
        JSONLActivityEventStore(journalURL: defaultJournalURL(), lineCodec: lineCodec)
    }

    nonisolated static func defaultJournalURL() -> URL {
        let baseURL = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.temporaryDirectory

        return baseURL
            .appending(path: "Pathlight", directoryHint: .isDirectory)
            .appending(path: "activity-events.jsonl")
    }

    /// The core's journal over this file. Built per call: whether new rows
    /// are sealed is a preference, and it can change while the app runs.
    private func journal(encrypting: Bool = false) -> Journal {
        guard let keys = lineCodec.legacyKeys else {
            return encrypting
                ? Journal.encrypting(path: journalURL.path)
                : Journal(path: journalURL.path)
        }
        return Journal.withLegacyKeys(path: journalURL.path, encrypt: encrypting, keys: keys)
    }

    /// The journal for a call that may write rows. The key is made ready
    /// through this app's provider first, because that is the one that adopts
    /// an older Keychain key instead of minting a second; the core then finds
    /// the key file already there.
    private func writingJournal() throws -> Journal {
        let encrypting = lineCodec.encryptsNewData
        if encrypting {
            try lineCodec.prepare()
        }
        return journal(encrypting: encrypting)
    }

    func append(_ events: [DiskActivityEvent]) async throws {
        guard !events.isEmpty else {
            return
        }
        guard !requiresReopenAfterFailedRollback else {
            throw ActivityEventStoreError.commitStateUnknown
        }
        do {
            try writingJournal().appendLocked(events: events.map(\.coreEvent))
        } catch CoreError.CommitStateUnknown {
            requiresReopenAfterFailedRollback = true
            throw ActivityEventStoreError.commitStateUnknown
        }
        if let minTimestamp = events.map(\.timestamp).min() {
            oldestKnownEventTimestamp = min(oldestKnownEventTimestamp ?? .distantFuture, minTimestamp)
        }
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] {
        try journal()
            .load(rootPath: rootPath.standardizedFileURL.path, limit: UInt32(clamping: max(limit, 0)))
            .map(DiskActivityEvent.init)
    }

    /// One pass over the journal: totals and trend buckets come from every
    /// retained line, while only the page asked for is kept in memory.
    func loadEventPage(
        rootPath: URL,
        limit: Int,
        bucketInterval: TimeInterval,
        query: ActivityHistoryQuery
    ) async throws -> ActivityEventPage {
        ActivityEventPage(
            try journal().loadHistory(
                rootPath: rootPath.standardizedFileURL.path,
                limit: UInt32(clamping: max(limit, 0)),
                bucketIntervalSecs: UInt64(max(bucketInterval, 1)),
                query: query.coreQuery
            )
        )
    }

    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date = Date()
    ) async throws {
        guard FileManager.default.fileExists(atPath: journalURL.path),
              !canSkipEnforcement(
                  preferences: preferences,
                  eventJournalLimitBytes: eventJournalLimitBytes,
                  now: now
              ) else {
            return
        }
        let outcome = try writingJournal().trimLocked(
            retentionDays: UInt32(clamping: preferences.detailedRetentionDays),
            aggregateDays: UInt32(clamping: preferences.aggregateRetentionDays),
            // The core's zero is "no cap". Here zero is a budget the rest of
            // the storage already spent, which leaves the journal nothing.
            limitBytes: UInt64(max(eventJournalLimitBytes, 1)),
            now: now
        )
        hasScannedJournal = true
        oldestKnownEventTimestamp = outcome.oldestKept
    }

    func removeEvents(rootPath: URL) async throws {
        _ = try journal().forgetRootLocked(rootPath: rootPath.standardizedFileURL.path)
    }

    func reset(additionalStorageFiles: [URL] = []) async throws {
        try ActivityStorageFileProtection.withStorageLock(
            in: journalURL.deletingLastPathComponent()
        ) {
            try ActivityStorageFileProtection.advanceRecordsGenerationWhileLocked(
                in: journalURL.deletingLastPathComponent()
            )
            for url in [journalURL] + additionalStorageFiles
            where FileManager.default.fileExists(atPath: url.path) {
                try FileManager.default.removeItem(at: url)
            }
        }
        hasScannedJournal = false
        oldestKnownEventTimestamp = nil
        requiresReopenAfterFailedRollback = false
    }

    private func canSkipEnforcement(
        preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) -> Bool {
        // The cached oldest timestamp is only ever older-or-equal to the true oldest
        // on disk, so skipping is conservative: at worst one unnecessary full pass,
        // never a missed cleanup.
        guard hasScannedJournal else {
            return false
        }
        let attributes = try? FileManager.default.attributesOfItem(atPath: journalURL.path)
        guard let fileSize = (attributes?[.size] as? NSNumber)?.int64Value,
              fileSize <= eventJournalLimitBytes else {
            return false
        }
        guard let oldestKnownEventTimestamp else {
            return true
        }
        return oldestKnownEventTimestamp >= Self.detailedCutoff(preferences: preferences, now: now)
    }

    nonisolated private static func detailedCutoff(preferences: ActivityStoragePreferences, now: Date) -> Date {
        now.addingTimeInterval(-TimeInterval(preferences.detailedRetentionDays) * 86_400)
    }
}
