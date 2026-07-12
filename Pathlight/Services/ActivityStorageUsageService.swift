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
        let fileManager = FileManager.default
        if fileManager.fileExists(atPath: eventJournalURL.path) {
            try fileManager.removeItem(at: eventJournalURL)
        }
        if fileManager.fileExists(atPath: sizeIndexJournalURL.path) {
            try fileManager.removeItem(at: sizeIndexJournalURL)
        }
    }

    func compactSizeIndex() async {
        ActivitySizeIndex(journalURL: sizeIndexJournalURL, lineCodec: lineCodec).compactNow()
    }

    func availableEventJournalBytes(storageLimitBytes: Int64) async -> Int64 {
        await compactSizeIndex()
        let normalizedLimit = max(storageLimitBytes, 0)
        guard fileSize(at: sizeIndexJournalURL) > normalizedLimit else {
            return normalizedLimit - fileSize(at: sizeIndexJournalURL)
        }

        try? FileManager.default.removeItem(at: sizeIndexJournalURL)
        return normalizedLimit
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
