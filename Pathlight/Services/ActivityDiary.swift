import Foundation

/// The watch's own diary: when it opened, what it caught up on after a gap,
/// what it warned about, and what failed.
///
/// `Storage::note` in `core/src/store.rs` writes the same file for the other
/// two hosts — and on a mac it is literally the same file, since the app's
/// storage directory is the core's. A line here is the only answer to "it
/// missed something last night" that does not need somebody to have been
/// looking at the window when it happened.
nonisolated struct ActivityDiary: Sendable {
    /// Matches `LOG_MAX_BYTES` in `core/src/store.rs`: a diary that grows
    /// without a bound is the disk problem it was written to explain.
    static let maximumBytes = 256 * 1024

    static let live = ActivityDiary(fileURL: defaultFileURL())

    let fileURL: URL

    static func defaultFileURL() -> URL {
        JSONLActivityEventStore.defaultJournalURL()
            .deletingLastPathComponent()
            .appending(path: "pathlight.log")
    }

    /// Writes one line, best effort on purpose: a monitor that stopped
    /// watching because it could not write its own diary would be worse than
    /// a monitor with a gap in the diary.
    func note(_ line: String) {
        try? ActivityStorageFileProtection.createProtectedDirectory(
            at: fileURL.deletingLastPathComponent()
        )
        trimIfOversized()
        let stamped = "\(Self.stamp(Date())) \(line)\n"
        // O_APPEND, not seek-then-write: two watches and the terminal host
        // append to this file, and a short append to an O_APPEND descriptor
        // is the one write that cannot interleave with theirs.
        let descriptor = open(fileURL.path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        _ = Array(stamped.utf8).withUnsafeBytes { bytes in
            write(descriptor, bytes.baseAddress, bytes.count)
        }
    }

    /// The last `lines` lines, newest last, as a person reads them. Empty when
    /// nothing has been written yet, which is itself an answer.
    func tail(lines: Int) -> String {
        guard let text = try? String(contentsOf: fileURL, encoding: .utf8) else {
            return ""
        }
        let kept = text.split(separator: "\n", omittingEmptySubsequences: true)
        return kept.suffix(lines).joined(separator: "\n")
    }

    /// Drops the older half when the file passes its cap, cutting on a line
    /// boundary so the oldest surviving line is whole. Halving rather than
    /// shaving, so this happens once per doubling instead of on every write.
    private func trimIfOversized() {
        let size = (try? FileManager.default.attributesOfItem(atPath: fileURL.path)[.size]) as? Int
        guard let size, size > Self.maximumBytes else { return }
        guard let text = try? String(contentsOf: fileURL, encoding: .utf8) else { return }
        let middle = text.index(text.startIndex, offsetBy: text.count / 2)
        guard let boundary = text[middle...].firstIndex(of: "\n") else { return }
        try? String(text[text.index(after: boundary)...]).write(
            to: fileURL,
            atomically: true,
            encoding: .utf8
        )
    }

    /// The same RFC 3339 stamp in UTC `swift_date::text` writes, so one file
    /// read top to bottom is in one order however many hosts wrote it. Built
    /// per line rather than kept around: lines are rare, and a shared
    /// formatter is mutable state two watches would share.
    private static func stamp(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.string(from: date)
    }
}
