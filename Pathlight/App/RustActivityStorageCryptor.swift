import Foundation
import PathlightRustCore

/// The journal line format, from the core that owns it. Every host reads the
/// same protected key file, and Swift hands those bytes to the core per call.
nonisolated struct RustActivityStorageCryptor: ActivityStorageLineCrypting {
    /// Cached: the marker is a constant, and this is asked once per journal row.
    private static let marker = storageLineMarker()

    let keyProvider: any ActivityStorageKeyProviding
    let legacyKeyProvider: (any ActivityStorageKeyProviding)?

    static let live = RustActivityStorageCryptor(
        keyProvider: CachingActivityStorageKeyProvider(
            wrapping: FileActivityStorageKeyProvider.live
        ),
        legacyKeyProvider: CachingActivityStorageKeyProvider(
            wrapping: LegacyKeychainActivityStorageKeyProvider()
        )
    )

    func seal(_ payload: Data) throws -> String {
        try sealStorageLine(payload: payload, key: keyProvider.loadOrCreateKey())
    }

    func open(_ line: String) throws -> Data? {
        // The marker first, then the key: a journal written before encryption
        // was turned on is read without ever touching the key file.
        guard line.hasPrefix(Self.marker) else {
            return nil
        }
        let sharedKey = try keyProvider.loadOrCreateKey()
        do {
            return try openStorageLine(line: line, key: sharedKey)
        } catch {
            // A machine that used both old hosts may already have two key
            // families. Reading through the old Keychain key lets the normal
            // journal/index compaction rewrite those rows under the shared key.
            guard let legacyKeyProvider,
                  let legacyKey = try? legacyKeyProvider.loadOrCreateKey(),
                  legacyKey != sharedKey else {
                throw error
            }
            return try openStorageLine(line: line, key: legacyKey)
        }
    }
}
