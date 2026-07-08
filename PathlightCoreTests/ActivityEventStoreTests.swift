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
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appending(path: "PathlightTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}
