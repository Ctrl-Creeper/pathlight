import Foundation

nonisolated enum ActivityStorageFileProtection {
    private static let lockFileName = "pathlight.lock"
    private static let settingsFileName = "watches.json"

    static func createProtectedDirectory(at url: URL) throws {
        try FileManager.default.createDirectory(
            at: url,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: url.path
        )
    }

    static func ensureProtectedFile(at url: URL) throws {
        let fileManager = FileManager.default
        if !fileManager.fileExists(atPath: url.path) {
            fileManager.createFile(
                atPath: url.path,
                contents: nil,
                attributes: [.posixPermissions: 0o600]
            )
        }
        try applyProtectedFilePermissions(to: url)
    }

    static func applyProtectedFilePermissions(to url: URL) throws {
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: url.path
        )
    }

    static func withStorageLock<T>(
        in directory: URL,
        _ operation: () throws -> T
    ) throws -> T {
        try createProtectedDirectory(at: directory)
        let lockURL = directory.appending(path: lockFileName)
        let descriptor = open(lockURL.path, O_RDWR | O_CREAT, 0o600)
        guard descriptor >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        defer { close(descriptor) }
        guard flock(descriptor, LOCK_EX) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        defer { flock(descriptor, LOCK_UN) }
        return try operation()
    }

    /// Advances the reset boundary read by Rust workers. Call only while
    /// holding `withStorageLock`; preserving the JSON object keeps settings
    /// owned by the CLI/GUI intact.
    static func advanceRecordsGenerationWhileLocked(in directory: URL) throws {
        let settingsURL = directory.appending(path: settingsFileName)
        var settings: [String: Any] = [:]
        if let data = try? Data(contentsOf: settingsURL),
           let object = try? JSONSerialization.jsonObject(with: data),
           let stored = object as? [String: Any] {
            settings = stored
        }
        let epoch = (settings["records_epoch"] as? NSNumber)?.uint64Value ?? 0
        settings["records_epoch"] = NSNumber(value: epoch &+ 1)
        settings["records_reset_at_ns"] = NSNumber(
            value: UInt64(max(0, Date().timeIntervalSince1970) * 1_000_000_000)
        )
        let data = try JSONSerialization.data(
            withJSONObject: settings,
            options: [.prettyPrinted, .sortedKeys]
        )
        try data.write(to: settingsURL, options: .atomic)
        try applyProtectedFilePermissions(to: settingsURL)
    }
}
