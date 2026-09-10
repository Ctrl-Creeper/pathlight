import Foundation
import PathlightRustCore

/// The journal line format, from the core that owns it. macOS keeps its key in
/// the Keychain, which no other process can open, so the key is handed to the
/// core per call rather than stored there.
nonisolated struct RustActivityStorageCryptor: ActivityStorageLineCrypting {
    /// Cached: the marker is a constant, and this is asked once per journal row.
    private static let marker = storageLineMarker()

    let keyProvider: any ActivityStorageKeyProviding

    static let live = RustActivityStorageCryptor(
        keyProvider: CachingActivityStorageKeyProvider(
            wrapping: KeychainActivityStorageKeyProvider.live
        )
    )

    func seal(_ payload: Data) throws -> String {
        try sealStorageLine(payload: payload, key: keyProvider.loadOrCreateKey())
    }

    func open(_ line: String) throws -> Data? {
        // The marker first, then the key: a journal written before encryption
        // was turned on is read without ever touching the Keychain.
        guard line.hasPrefix(Self.marker) else {
            return nil
        }
        return try openStorageLine(line: line, key: keyProvider.loadOrCreateKey())
    }
}
