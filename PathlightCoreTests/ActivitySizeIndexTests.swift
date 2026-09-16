import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity size index")
struct ActivitySizeIndexTests {
    @Test("returns and clears the last known size for a deleted path")
    func returnsAndClearsLastKnownSize() {
        let index = ActivitySizeIndex()
        let file = URL(filePath: "/Users/example/Downloads/archive.zip")

        #expect(index.recordKnownSize(1_024, for: file) == 1_024)
        #expect(index.takeKnownSize(for: file) == 1_024)
        #expect(index.takeKnownSize(for: file) == nil)
    }

    /// A whole-disk watch and a folder watch over the same file are separate
    /// observers. They shared one entry, so whichever handled an event first
    /// recorded the new size and the other measured a delta of zero.
    @Test("keeps one watch's baseline out of another's")
    func scopesBaselinesPerWatch() {
        let index = ActivitySizeIndex()
        let file = URL(filePath: "/Users/example/Downloads/archive.zip")
        let disk = ActivitySizeProviders.scope(kind: "long-term", rootPath: URL(filePath: "/"))
        let folder = ActivitySizeProviders.scope(
            kind: "long-term",
            rootPath: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        )

        index.recordKnownSize(1_024, for: file, scope: disk)

        #expect(index.knownSize(for: file, scope: folder) == nil)
        #expect(index.knownSize(for: file) == nil, "an unscoped lookup is its own namespace too")

        index.recordKnownSize(4_096, for: file, scope: folder)

        #expect(index.knownSize(for: file, scope: disk) == 1_024)
        #expect(index.takeKnownSize(for: file, scope: folder) == 4_096)
        #expect(index.knownSize(for: file, scope: disk) == 1_024, "taking one scope must not clear another")
    }

    @Test("uses standardized file paths as cache keys")
    func standardizesCacheKeys() {
        let index = ActivitySizeIndex()
        let original = URL(filePath: "/Users/example/Downloads/../Downloads/movie.dmg")
        let equivalent = URL(filePath: "/Users/example/Downloads/movie.dmg")

        index.recordKnownSize(4_096, for: original)

        #expect(index.takeKnownSize(for: equivalent) == 4_096)
    }

    @Test("reloads last known sizes from disk")
    func reloadsLastKnownSizesFromDisk() {
        let journalURL = makeTemporaryJournalURL()
        let file = URL(filePath: "/Users/example/Downloads/old.iso")

        let firstLaunchIndex = ActivitySizeIndex(journalURL: journalURL)
        firstLaunchIndex.recordKnownSize(8_192, for: file)
        firstLaunchIndex.flushPendingJournalWrites()

        let secondLaunchIndex = ActivitySizeIndex(journalURL: journalURL)
        #expect(secondLaunchIndex.takeKnownSize(for: file) == 8_192)
        secondLaunchIndex.flushPendingJournalWrites()

        let thirdLaunchIndex = ActivitySizeIndex(journalURL: journalURL)
        #expect(thirdLaunchIndex.takeKnownSize(for: file) == nil)
    }

    @Test("compacts journal to the current size snapshot")
    func compactsJournalToCurrentSnapshot() throws {
        let journalURL = makeTemporaryJournalURL()
        let oldFile = URL(filePath: "/Users/example/Downloads/old.iso")
        let currentFile = URL(filePath: "/Users/example/Downloads/current.iso")

        let index = ActivitySizeIndex(journalURL: journalURL, compactionEntryThreshold: 3)
        index.recordKnownSize(1_024, for: oldFile)
        index.recordKnownSize(2_048, for: currentFile)
        #expect(index.takeKnownSize(for: oldFile) == 1_024)
        index.flushPendingJournalWrites()

        let contents = String(decoding: try Data(contentsOf: journalURL), as: UTF8.self)
        let lines = contents.split(separator: "\n", omittingEmptySubsequences: true)
        #expect(lines.count == 1)

        let reloadedIndex = ActivitySizeIndex(journalURL: journalURL)
        #expect(reloadedIndex.takeKnownSize(for: currentFile) == 2_048)
        #expect(reloadedIndex.takeKnownSize(for: oldFile) == nil)
    }

    @Test("compaction preserves entries appended by another host")
    func compactionPreservesEntriesFromAnotherHost() throws {
        let journalURL = makeTemporaryJournalURL()
        let firstFile = URL(filePath: "/Users/example/Downloads/first.iso")
        let secondFile = URL(filePath: "/Users/example/Downloads/second.iso")
        let first = ActivitySizeIndex(journalURL: journalURL)
        let second = ActivitySizeIndex(journalURL: journalURL, compactionEntryThreshold: 1)

        first.recordKnownSize(1_024, for: firstFile)
        first.flushPendingJournalWrites()
        second.recordKnownSize(2_048, for: secondFile)
        second.flushPendingJournalWrites()

        let reopened = ActivitySizeIndex(journalURL: journalURL)
        #expect(reopened.knownSize(for: firstFile) == 1_024)
        #expect(reopened.knownSize(for: secondFile) == 2_048)
    }

    @Test("encrypted journal hides paths and uses owner-only permissions")
    func encryptedJournalHidesPathsAndUsesOwnerOnlyPermissions() throws {
        let journalURL = makeTemporaryJournalURL()
        let file = URL(filePath: "/Users/example/Downloads/sensitive.iso")
        let index = ActivitySizeIndex(
            journalURL: journalURL,
            lineCodec: makeSealedActivityStorageLineCodec()
        )

        index.recordKnownSize(8_192, for: file)
        index.flushPendingJournalWrites()
        let contents = String(decoding: try Data(contentsOf: journalURL), as: UTF8.self)
        let reloadedIndex = ActivitySizeIndex(
            journalURL: journalURL,
            lineCodec: makeSealedActivityStorageLineCodec()
        )

        #expect(contents.hasPrefix("pathlight:v1:aes-gcm:"))
        #expect(!contents.contains("sensitive.iso"))
        #expect(reloadedIndex.takeKnownSize(for: file) == 8_192)
        #expect(try posixPermissions(at: journalURL.deletingLastPathComponent()) & 0o777 == 0o700)
        #expect(try posixPermissions(at: journalURL) & 0o777 == 0o600)
    }

    @Test("persists concurrent mutations")
    func persistsConcurrentMutations() async {
        let journalURL = makeTemporaryJournalURL()
        let index = ActivitySizeIndex(journalURL: journalURL)

        await withTaskGroup(of: Void.self) { group in
            for taskIndex in 0..<8 {
                group.addTask {
                    let file = URL(filePath: "/Users/example/Downloads/file-\(taskIndex).bin")
                    index.recordKnownSize(Int64(taskIndex + 1) * 100, for: file)
                    _ = index.knownSize(for: file)
                    if taskIndex.isMultiple(of: 2) {
                        _ = index.takeKnownSize(for: file)
                    }
                }
            }
        }
        index.flushPendingJournalWrites()

        let reloaded = ActivitySizeIndex(journalURL: journalURL)
        for taskIndex in 0..<8 {
            let file = URL(filePath: "/Users/example/Downloads/file-\(taskIndex).bin")
            let expected: Int64? = taskIndex.isMultiple(of: 2) ? nil : Int64(taskIndex + 1) * 100
            #expect(reloaded.knownSize(for: file) == expected)
        }
    }

    /// A baseline nobody has touched inside the window goes, and a compaction
    /// carries the stamp it found rather than restamping what it rewrites.
    /// Restamping would make every survivor look new, and the index would grow
    /// for the life of the machine — one entry per file ever changed.
    @Test("ages out untouched baselines, and a compaction does not reset their age")
    func agesOutUntouchedBaselines() throws {
        let journalURL = makeTemporaryJournalURL()
        let ancient = URL(filePath: "/Users/example/Downloads/ancient.bin")
        let older = URL(filePath: "/Users/example/Downloads/older.bin")
        let index = ActivitySizeIndex(journalURL: journalURL)
        index.recordKnownSize(1, for: ancient)
        index.recordKnownSize(2, for: older)
        index.flushPendingJournalWrites()
        try backdate(journalURL, byDays: [0: 400, 1: 100])

        let loaded = ActivitySizeIndex(journalURL: journalURL, keepDays: 180)
        #expect(loaded.knownSize(for: ancient) == nil)
        #expect(loaded.knownSize(for: older) == 2)

        // Rewrite, then reopen with a window the survivor is outside of: it can
        // only still be there if the rewrite restamped it.
        loaded.compactNow()
        let reopened = ActivitySizeIndex(journalURL: journalURL, keepDays: 50)
        #expect(
            reopened.knownSize(for: older) == nil,
            "the compaction stamped a rewrite it did not measure"
        )
    }
}

/// Backdates lines in place: the age of a baseline is the one thing a test
/// cannot produce by waiting.
private func backdate(_ journalURL: URL, byDays: [Int: Double]) throws {
    let formatter = ISO8601DateFormatter()
    let lines = try String(contentsOf: journalURL, encoding: .utf8)
        .split(separator: "\n", omittingEmptySubsequences: true)
    let rewritten = try lines.enumerated().map { position, line -> String in
        guard let days = byDays[position],
              var object = try JSONSerialization.jsonObject(with: Data(line.utf8))
                  as? [String: Any] else {
            return String(line)
        }
        object["recordedAt"] = formatter.string(from: Date(timeIntervalSinceNow: -days * 86_400))
        return String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
    }
    try Data((rewritten.joined(separator: "\n") + "\n").utf8).write(to: journalURL)
}

private func makeTemporaryJournalURL() -> URL {
    let directoryURL = FileManager.default.temporaryDirectory
        .appending(path: "PathlightSizeIndexTests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try? FileManager.default.createDirectory(at: directoryURL, withIntermediateDirectories: true)
    return directoryURL.appending(path: "activity-size-index.jsonl")
}
