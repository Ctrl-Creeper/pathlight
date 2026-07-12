import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity event store")
struct ActivityEventStoreTests {
    @Test("persists and reloads events for a root")
    func persistsAndReloadsEventsForRoot() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let otherRoot = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let first = DiskActivityEvent(
            kind: .created,
            path: root.appending(path: "a.dmg"),
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100),
            byteDelta: 2_048,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
        let second = DiskActivityEvent(
            kind: .deleted,
            path: root.appending(path: "old.zip"),
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 110),
            byteDelta: -1_024,
            confidence: .estimated,
            previousPath: nil,
            affectedItemCount: 1
        )
        let unrelated = DiskActivityEvent(
            kind: .modified,
            path: otherRoot.appending(path: "note.txt"),
            rootPath: otherRoot,
            timestamp: Date(timeIntervalSince1970: 120),
            byteDelta: 512,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )

        let writer = JSONLActivityEventStore(journalURL: journalURL)
        try await writer.append([first, second, unrelated])

        let reader = JSONLActivityEventStore(journalURL: journalURL)
        let loaded = try await reader.loadEvents(rootPath: root, limit: 10)

        #expect(loaded == [second, first])
    }

    @Test("limits loaded events")
    func limitsLoadedEvents() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let events = (0..<3).map { index in
            DiskActivityEvent(
                kind: .created,
                path: root.appending(path: "\(index).bin"),
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: TimeInterval(100 + index)),
                byteDelta: 1_024,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        }

        let store = JSONLActivityEventStore(journalURL: journalURL)
        try await store.append(events)

        let loaded = try await store.loadEvents(rootPath: root, limit: 2)

        #expect(loaded.map(\.path.lastPathComponent) == ["2.bin", "1.bin"])
    }

    @Test("encrypted journal hides paths and uses owner-only permissions")
    func encryptedJournalHidesPathsAndUsesOwnerOnlyPermissions() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let event = DiskActivityEvent(
            kind: .created,
            path: root.appending(path: "sensitive.zip"),
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100),
            byteDelta: 2_048,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
        let store = JSONLActivityEventStore(
            journalURL: journalURL,
            lineCodec: makeEncryptedActivityStorageLineCodec()
        )

        try await store.append([event])
        let contents = String(decoding: try Data(contentsOf: journalURL), as: UTF8.self)
        let loaded = try await store.loadEvents(rootPath: root, limit: 10)

        #expect(contents.hasPrefix("pathlight:v1:aes-gcm:"))
        #expect(!contents.contains("sensitive.zip"))
        #expect(loaded == [event])
        #expect(try posixPermissions(at: tempDirectory) & 0o777 == 0o700)
        #expect(try posixPermissions(at: journalURL) & 0o777 == 0o600)
    }

    @Test("rolls expired detailed events into path-free daily aggregates")
    func rollsExpiredDetailedEventsIntoDailyAggregates() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let now = Date(timeIntervalSince1970: 3_000_000)
        let oldFirst = makeEvent(
            root: root,
            name: "old-installer.dmg",
            timestamp: now.addingTimeInterval(-10 * 86_400),
            byteDelta: 2_048
        )
        let oldSecond = makeEvent(
            root: root,
            name: "old-cache.zip",
            timestamp: now.addingTimeInterval(-10 * 86_400 + 60),
            byteDelta: -512
        )
        let current = makeEvent(
            root: root,
            name: "current-video.mov",
            timestamp: now.addingTimeInterval(-86_400),
            byteDelta: 4_096
        )
        let expired = makeEvent(
            root: root,
            name: "expired.iso",
            timestamp: now.addingTimeInterval(-40 * 86_400),
            byteDelta: 1_024
        )
        let store = JSONLActivityEventStore(journalURL: journalURL)
        try await store.append([oldFirst, oldSecond, current, expired])

        try await store.enforceStoragePolicy(
            storagePreferences(detailedRetentionDays: 7, aggregateRetentionDays: 30),
            eventJournalLimitBytes: 1_024 * 1_024,
            now: now
        )
        let events = try await store.loadEvents(rootPath: root, limit: 10)

        #expect(events.count == 2)
        #expect(events.first == current)
        #expect(events.last?.kind == .aggregate)
        #expect(events.last?.path == root.standardizedFileURL)
        #expect(events.last?.byteDelta == 1_536)
        #expect(events.last?.affectedItemCount == 2)
        let contents = String(decoding: try Data(contentsOf: journalURL), as: UTF8.self)
        #expect(!contents.contains("old-installer.dmg"))
        #expect(!contents.contains("old-cache.zip"))
        #expect(!contents.contains("expired.iso"))
    }

    @Test("enforces the event journal byte budget by retaining newest records")
    func enforcesEventJournalByteBudgetByRetainingNewestRecords() async throws {
        let tempDirectory = try makeTemporaryDirectory()
        let journalURL = tempDirectory.appending(path: "activity-events.jsonl")
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let oldest = makeEvent(root: root, name: "oldest.bin", timestamp: Date(timeIntervalSince1970: 100), byteDelta: 1)
        let middle = makeEvent(root: root, name: "middle.bin", timestamp: Date(timeIntervalSince1970: 200), byteDelta: 2)
        let newest = makeEvent(root: root, name: "newest.bin", timestamp: Date(timeIntervalSince1970: 300), byteDelta: 3)
        let store = JSONLActivityEventStore(journalURL: journalURL)
        try await store.append([oldest, middle, newest])
        let originalLines = String(decoding: try Data(contentsOf: journalURL), as: UTF8.self)
            .split(separator: "\n", omittingEmptySubsequences: true)
        let newestLineBytes = Int64((originalLines.last?.utf8.count ?? 0) + 1)

        try await store.enforceStoragePolicy(
            storagePreferences(detailedRetentionDays: 36_500, aggregateRetentionDays: 36_500),
            eventJournalLimitBytes: newestLineBytes,
            now: Date(timeIntervalSince1970: 400)
        )
        let events = try await store.loadEvents(rootPath: root, limit: 10)

        #expect(events == [newest])
        let finalSize = try journalURL.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
        #expect(Int64(finalSize) <= newestLineBytes)
    }
}

private func storagePreferences(
    detailedRetentionDays: Int,
    aggregateRetentionDays: Int
) -> ActivityStoragePreferences {
    ActivityStoragePreferences(
        detailedRetentionDays: detailedRetentionDays,
        aggregateRetentionDays: aggregateRetentionDays,
        storageLimitBytes: 1_024 * 1_024 * 1_024,
        encryptNewData: false
    )
}

private func makeEvent(
    root: URL,
    name: String,
    timestamp: Date,
    byteDelta: Int64
) -> DiskActivityEvent {
    DiskActivityEvent(
        kind: .created,
        path: root.appending(path: name),
        rootPath: root,
        timestamp: timestamp,
        byteDelta: byteDelta,
        confidence: .confirmed,
        previousPath: nil,
        affectedItemCount: 1
    )
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appending(path: "PathlightTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}
