import Foundation

nonisolated struct ActivityStorageUsageSnapshot: Equatable, Sendable {
    let eventJournalBytes: Int64
    let sizeIndexJournalBytes: Int64
    let eventEntryCount: Int
    let sizeIndexEntryCount: Int

    static let empty = ActivityStorageUsageSnapshot(
        eventJournalBytes: 0,
        sizeIndexJournalBytes: 0,
        eventEntryCount: 0,
        sizeIndexEntryCount: 0
    )

    var totalBytes: Int64 {
        eventJournalBytes + sizeIndexJournalBytes
    }
}

nonisolated struct ActivityStorageUsageService: Sendable {
    private let eventJournalURL: URL
    private let sizeIndexJournalURL: URL
    private let lineCodec: ActivityStorageLineCodec

    init(
        eventJournalURL: URL = JSONLActivityEventStore.defaultJournalURL(),
        sizeIndexJournalURL: URL = ActivitySizeIndex.defaultJournalURL(),
        lineCodec: ActivityStorageLineCodec = .plaintext
    ) {
        self.eventJournalURL = eventJournalURL
        self.sizeIndexJournalURL = sizeIndexJournalURL
        self.lineCodec = lineCodec
    }

    func loadUsage() async -> ActivityStorageUsageSnapshot {
        ActivityStorageUsageSnapshot(
            eventJournalBytes: fileSize(at: eventJournalURL),
            sizeIndexJournalBytes: fileSize(at: sizeIndexJournalURL),
            eventEntryCount: lineCount(at: eventJournalURL),
            sizeIndexEntryCount: lineCount(at: sizeIndexJournalURL)
        )
    }

    func resetStorage() async throws {
        try ActivityStorageFileProtection.withStorageLock(
            in: eventJournalURL.deletingLastPathComponent()
        ) {
            try ActivityStorageFileProtection.advanceRecordsGenerationWhileLocked(
                in: eventJournalURL.deletingLastPathComponent()
            )
            let fileManager = FileManager.default
            if fileManager.fileExists(atPath: eventJournalURL.path) {
                try fileManager.removeItem(at: eventJournalURL)
            }
            if fileManager.fileExists(atPath: sizeIndexJournalURL.path) {
                try fileManager.removeItem(at: sizeIndexJournalURL)
            }
        }
    }

    func compactSizeIndex() async {
        ActivitySizeIndex(journalURL: sizeIndexJournalURL, lineCodec: lineCodec).compactNow()
    }

    func availableEventJournalBytes(storageLimitBytes: Int64) async -> Int64 {
        await compactSizeIndex()
        let normalizedLimit = max(storageLimitBytes, 0)
        do {
            return try ActivityStorageFileProtection.withStorageLock(
                in: eventJournalURL.deletingLastPathComponent()
            ) {
                let sizeIndexBytes = fileSize(at: sizeIndexJournalURL)
                guard sizeIndexBytes > normalizedLimit else {
                    return normalizedLimit - sizeIndexBytes
                }
                do {
                    try FileManager.default.removeItem(at: sizeIndexJournalURL)
                    return normalizedLimit
                } catch {
                    return 0
                }
            }
        } catch {
            return 0
        }
    }

    private func fileSize(at url: URL) -> Int64 {
        guard let values = try? url.resourceValues(forKeys: [.fileSizeKey]),
              let fileSize = values.fileSize else {
            return 0
        }
        return Int64(fileSize)
    }

    private func lineCount(at url: URL) -> Int {
        guard let data = try? Data(contentsOf: url), !data.isEmpty else {
            return 0
        }
        return String(decoding: data, as: UTF8.self)
            .split(separator: "\n", omittingEmptySubsequences: true)
            .count
    }
}
