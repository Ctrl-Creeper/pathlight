import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity storage usage service")
struct ActivityStorageUsageServiceTests {
    @Test("reports activity journal sizes and line counts")
    func reportsJournalSizesAndLineCounts() async throws {
        let directoryURL = makeTemporaryStorageDirectory()
        let eventsURL = directoryURL.appending(path: "activity-events.jsonl")
        let sizeIndexURL = directoryURL.appending(path: "activity-size-index.jsonl")
        try Data("event-one\nevent-two\n".utf8).write(to: eventsURL)
        try Data("index-one\n".utf8).write(to: sizeIndexURL)
        let service = ActivityStorageUsageService(
            eventJournalURL: eventsURL,
            sizeIndexJournalURL: sizeIndexURL
        )

        let usage = await service.loadUsage()

        #expect(usage.eventJournalBytes == 20)
        #expect(usage.sizeIndexJournalBytes == 10)
        #expect(usage.totalBytes == 30)
        #expect(usage.eventEntryCount == 2)
        #expect(usage.sizeIndexEntryCount == 1)
    }

    @Test("reset removes activity storage journals")
    func resetRemovesActivityStorageJournals() async throws {
        let directoryURL = makeTemporaryStorageDirectory()
        let eventsURL = directoryURL.appending(path: "activity-events.jsonl")
        let sizeIndexURL = directoryURL.appending(path: "activity-size-index.jsonl")
        try Data("event\n".utf8).write(to: eventsURL)
        try Data("index\n".utf8).write(to: sizeIndexURL)
        let settingsURL = directoryURL.appending(path: "watches.json")
        try Data(#"{"roots":[{"path":"/watched","enabled":true}],"paused":true,"records_epoch":7}"#.utf8)
            .write(to: settingsURL)
        let service = ActivityStorageUsageService(
            eventJournalURL: eventsURL,
            sizeIndexJournalURL: sizeIndexURL
        )

        try await service.resetStorage()

        #expect(!FileManager.default.fileExists(atPath: eventsURL.path))
        #expect(!FileManager.default.fileExists(atPath: sizeIndexURL.path))
        #expect(await service.loadUsage() == .empty)
        let settings = try #require(
            JSONSerialization.jsonObject(with: Data(contentsOf: settingsURL)) as? [String: Any]
        )
        #expect((settings["records_epoch"] as? NSNumber)?.uint64Value == 8)
        #expect(((settings["records_reset_at_ns"] as? NSNumber)?.uint64Value ?? 0) > 0)
        #expect((settings["paused"] as? Bool) == true)
        #expect((settings["roots"] as? [[String: Any]])?.first?["path"] as? String == "/watched")
    }

    @Test("compact preserves encrypted size index entries")
    func compactPreservesEncryptedSizeIndexEntries() async throws {
        let directoryURL = makeTemporaryStorageDirectory()
        let eventsURL = directoryURL.appending(path: "activity-events.jsonl")
        let sizeIndexURL = directoryURL.appending(path: "activity-size-index.jsonl")
        let lineCodec = makeSealedActivityStorageLineCodec()
        let file = URL(filePath: "/Users/example/Downloads/current.iso")
        let index = ActivitySizeIndex(journalURL: sizeIndexURL, lineCodec: lineCodec)
        index.recordKnownSize(4_096, for: file)
        index.flushPendingJournalWrites()
        let service = ActivityStorageUsageService(
            eventJournalURL: eventsURL,
            sizeIndexJournalURL: sizeIndexURL,
            lineCodec: lineCodec
        )

        await service.compactSizeIndex()
        let reloadedIndex = ActivitySizeIndex(journalURL: sizeIndexURL, lineCodec: lineCodec)
        let contents = String(decoding: try Data(contentsOf: sizeIndexURL), as: UTF8.self)

        #expect(reloadedIndex.takeKnownSize(for: file) == 4_096)
        #expect(contents.hasPrefix("pathlight:v1:aes-gcm:"))
        #expect(!contents.contains("current.iso"))
    }

    @Test("reclaiming an oversized index waits for the storage transaction")
    func oversizedIndexReclaimWaitsForStorageTransaction() async throws {
        let eventDirectoryURL = makeTemporaryStorageDirectory()
        let indexDirectoryURL = makeTemporaryStorageDirectory()
        let eventsURL = eventDirectoryURL.appending(path: "activity-events.jsonl")
        let sizeIndexURL = indexDirectoryURL.appending(path: "activity-size-index.jsonl")
        let index = ActivitySizeIndex(journalURL: sizeIndexURL)
        index.recordKnownSize(
            4_096,
            for: URL(filePath: "/Users/example/Downloads/current.iso")
        )
        index.flushPendingJournalWrites()
        let service = ActivityStorageUsageService(
            eventJournalURL: eventsURL,
            sizeIndexJournalURL: sizeIndexURL
        )
        let lockURL = eventDirectoryURL.appending(path: "pathlight.lock")
        let descriptor = open(lockURL.path, O_RDWR | O_CREAT, 0o600)
        #expect(descriptor >= 0)
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        #expect(flock(descriptor, LOCK_EX) == 0)

        let reclaim = Task {
            await service.availableEventJournalBytes(storageLimitBytes: 0)
        }
        try await Task.sleep(for: .milliseconds(100))

        #expect(FileManager.default.fileExists(atPath: sizeIndexURL.path))
        flock(descriptor, LOCK_UN)
        #expect(await reclaim.value == 0)
        #expect(!FileManager.default.fileExists(atPath: sizeIndexURL.path))
    }
}

private func makeTemporaryStorageDirectory() -> URL {
    let directoryURL = FileManager.default.temporaryDirectory
        .appending(path: "PathlightActivityStorageUsageTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try? FileManager.default.createDirectory(at: directoryURL, withIntermediateDirectories: true)
    return directoryURL
}
