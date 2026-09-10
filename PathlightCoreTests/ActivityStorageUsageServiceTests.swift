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
        let service = ActivityStorageUsageService(
            eventJournalURL: eventsURL,
            sizeIndexJournalURL: sizeIndexURL
        )

        try await service.resetStorage()

        #expect(!FileManager.default.fileExists(atPath: eventsURL.path))
        #expect(!FileManager.default.fileExists(atPath: sizeIndexURL.path))
        #expect(await service.loadUsage() == .empty)
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
}

private func makeTemporaryStorageDirectory() -> URL {
    let directoryURL = FileManager.default.temporaryDirectory
        .appending(path: "PathlightActivityStorageUsageTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try? FileManager.default.createDirectory(at: directoryURL, withIntermediateDirectories: true)
    return directoryURL
}
