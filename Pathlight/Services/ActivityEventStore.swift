import Foundation

nonisolated protocol ActivityEventStoring: Sendable {
    func append(_ events: [DiskActivityEvent]) async throws
    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent]
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws
}

actor JSONLActivityEventStore: ActivityEventStoring {
    private let journalURL: URL
    private let lineCodec: ActivityStorageLineCodec
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private var hasScannedJournal = false
    private var oldestKnownEventTimestamp: Date?

    init(
        journalURL: URL,
        lineCodec: ActivityStorageLineCodec = .plaintext
    ) {
        self.journalURL = journalURL
        self.lineCodec = lineCodec

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        self.encoder = encoder

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        self.decoder = decoder
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

    func append(_ events: [DiskActivityEvent]) async throws {
        guard !events.isEmpty else {
            return
        }

        let fileManager = FileManager.default
        try ActivityStorageFileProtection.createProtectedDirectory(
            at: journalURL.deletingLastPathComponent()
        )

        if !fileManager.fileExists(atPath: journalURL.path) {
            try ActivityStorageFileProtection.ensureProtectedFile(at: journalURL)
        }
        try ActivityStorageFileProtection.applyProtectedFilePermissions(to: journalURL)

        let lines = try events
            .map { event in
                let data = try encoder.encode(event)
                return try lineCodec.encode(data)
            }
            .joined(separator: "\n")
        let payload = Data((lines + "\n").utf8)

        let handle = try FileHandle(forWritingTo: journalURL)
        defer {
            try? handle.close()
        }
        try handle.seekToEnd()
        try handle.write(contentsOf: payload)
        try ActivityStorageFileProtection.applyProtectedFilePermissions(to: journalURL)
        if let minTimestamp = events.map(\.timestamp).min() {
            oldestKnownEventTimestamp = min(oldestKnownEventTimestamp ?? .distantFuture, minTimestamp)
        }
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
                guard let data = try? lineCodec.decode(line),
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

    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date = Date()
    ) async throws {
        guard FileManager.default.fileExists(atPath: journalURL.path) else {
            return
        }

        if canSkipEnforcement(preferences: preferences, eventJournalLimitBytes: eventJournalLimitBytes, now: now) {
            return
        }

        let events = try readAllEvents()
        let retainedEvents = retainedEvents(
            from: events,
            preferences: preferences,
            eventJournalLimitBytes: eventJournalLimitBytes,
            now: now
        )
        try rewriteJournal(with: retainedEvents)
        hasScannedJournal = true
        oldestKnownEventTimestamp = retainedEvents.first?.timestamp
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

    private func readAllEvents() throws -> [DiskActivityEvent] {
        let data = try Data(contentsOf: journalURL)
        let contents = String(decoding: data, as: UTF8.self)

        return contents
            .split(separator: "\n", omittingEmptySubsequences: true)
            .compactMap { line in
                guard let data = try? lineCodec.decode(line) else {
                    return nil
                }
                return try? decoder.decode(DiskActivityEvent.self, from: data)
            }
    }

    private func retainedEvents(
        from events: [DiskActivityEvent],
        preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) -> [DiskActivityEvent] {
        let detailedCutoff = Self.detailedCutoff(preferences: preferences, now: now)
        let aggregateRetentionDays = max(
            preferences.detailedRetentionDays,
            preferences.aggregateRetentionDays
        )
        let aggregateCutoff = now.addingTimeInterval(-TimeInterval(aggregateRetentionDays) * 86_400)
        let survivingEvents = events.filter { $0.timestamp >= aggregateCutoff }
        let olderDetailedEvents = survivingEvents.filter {
            $0.kind != .aggregate && $0.timestamp < detailedCutoff
        }
        let detailedOrExistingAggregateEvents = survivingEvents.filter {
            $0.kind == .aggregate || $0.timestamp >= detailedCutoff
        }
        let dailyAggregates = Dictionary(grouping: olderDetailedEvents, by: DailyAggregationKey.init)
            .values
            .map(Self.dailyAggregate)

        let candidates = (detailedOrExistingAggregateEvents + dailyAggregates)
            .sorted(by: Self.newestFirst)
        guard eventJournalLimitBytes > 0 else {
            return []
        }

        var retained: [DiskActivityEvent] = []
        var usedBytes: Int64 = 0
        for event in candidates {
            guard let line = try? encodedLine(for: event) else {
                continue
            }
            let lineBytes = Int64(line.utf8.count + 1)
            guard usedBytes + lineBytes <= eventJournalLimitBytes else {
                break
            }
            retained.append(event)
            usedBytes += lineBytes
        }

        return retained.sorted(by: Self.oldestFirst)
    }

    private func rewriteJournal(with events: [DiskActivityEvent]) throws {
        try ActivityStorageFileProtection.createProtectedDirectory(
            at: journalURL.deletingLastPathComponent()
        )
        let contents = try events
            .map(encodedLine(for:))
            .joined(separator: "\n")
        let data = contents.isEmpty ? Data() : Data((contents + "\n").utf8)
        try data.write(to: journalURL, options: .atomic)
        try ActivityStorageFileProtection.applyProtectedFilePermissions(to: journalURL)
    }

    private func encodedLine(for event: DiskActivityEvent) throws -> String {
        try lineCodec.encode(encoder.encode(event))
    }

    nonisolated private static func dailyAggregate(_ events: [DiskActivityEvent]) -> DiskActivityEvent {
        let sortedEvents = events.sorted(by: oldestFirst)
        let firstEvent = sortedEvents[0]
        let hasUnknownSize = sortedEvents.contains { $0.byteDelta == nil }
        let byteDelta = hasUnknownSize ? nil : sortedEvents.compactMap(\.byteDelta).reduce(Int64(0), +)
        let confidence: DiskActivityEventConfidence
        if byteDelta == nil {
            confidence = .unknown
        } else if sortedEvents.allSatisfy({ $0.confidence == .confirmed }) {
            confidence = .confirmed
        } else {
            confidence = .estimated
        }

        return DiskActivityEvent(
            kind: .aggregate,
            path: firstEvent.rootPath.standardizedFileURL,
            rootPath: firstEvent.rootPath.standardizedFileURL,
            timestamp: dayStart(for: firstEvent.timestamp),
            byteDelta: byteDelta,
            confidence: confidence,
            previousPath: nil,
            affectedItemCount: sortedEvents.reduce(0) { $0 + $1.affectedItemCount }
        )
    }

    nonisolated private static func dayStart(for date: Date) -> Date {
        Date(timeIntervalSince1970: floor(date.timeIntervalSince1970 / 86_400) * 86_400)
    }

    nonisolated private static func newestFirst(lhs: DiskActivityEvent, rhs: DiskActivityEvent) -> Bool {
        if lhs.timestamp == rhs.timestamp {
            return lhs.path.path > rhs.path.path
        }
        return lhs.timestamp > rhs.timestamp
    }

    nonisolated private static func oldestFirst(lhs: DiskActivityEvent, rhs: DiskActivityEvent) -> Bool {
        if lhs.timestamp == rhs.timestamp {
            return lhs.path.path < rhs.path.path
        }
        return lhs.timestamp < rhs.timestamp
    }

    nonisolated private struct DailyAggregationKey: Hashable {
        let rootPath: String
        let dayStart: Date

        init(event: DiskActivityEvent) {
            rootPath = event.rootPath.standardizedFileURL.path
            dayStart = JSONLActivityEventStore.dayStart(for: event.timestamp)
        }
    }
}
