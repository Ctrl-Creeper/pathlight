import Foundation

protocol ActivityEventStoring: Sendable {
    func append(_ events: [DiskActivityEvent]) async throws
    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent]
}

actor JSONLActivityEventStore: ActivityEventStoring {
    private let journalURL: URL
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    init(journalURL: URL) {
        self.journalURL = journalURL

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        self.encoder = encoder

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        self.decoder = decoder
    }

    static func live() -> JSONLActivityEventStore {
        JSONLActivityEventStore(journalURL: defaultJournalURL())
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

    func append(_ events: [DiskActivityEvent]) async throws {
        guard !events.isEmpty else {
            return
        }

        let fileManager = FileManager.default
        try fileManager.createDirectory(
            at: journalURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )

        if !fileManager.fileExists(atPath: journalURL.path) {
            fileManager.createFile(atPath: journalURL.path, contents: nil)
        }

        let lines = try events
            .map { event in
                let data = try encoder.encode(event)
                return String(decoding: data, as: UTF8.self)
            }
            .joined(separator: "\n")
        let payload = Data((lines + "\n").utf8)

        let handle = try FileHandle(forWritingTo: journalURL)
        defer {
            try? handle.close()
        }
        try handle.seekToEnd()
        try handle.write(contentsOf: payload)
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] {
        let fileManager = FileManager.default
        guard limit > 0, fileManager.fileExists(atPath: journalURL.path) else {
            return []
        }

        let root = rootPath.standardizedFileURL.path
        let data = try Data(contentsOf: journalURL)
        let contents = String(decoding: data, as: UTF8.self)

        return contents
            .split(separator: "\n", omittingEmptySubsequences: true)
            .compactMap { line -> DiskActivityEvent? in
                guard let data = String(line).data(using: .utf8),
                      let event = try? decoder.decode(DiskActivityEvent.self, from: data),
                      event.rootPath.standardizedFileURL.path == root else {
                    return nil
                }
                return event
            }
            .sorted { lhs, rhs in
                if lhs.timestamp == rhs.timestamp {
                    return lhs.path.path > rhs.path.path
                }
                return lhs.timestamp > rhs.timestamp
            }
            .prefix(limit)
            .map { $0 }
    }
}
