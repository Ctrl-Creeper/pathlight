import Foundation
import Security

nonisolated enum ActivityStorageLineCodecError: Error {
    case randomGenerationFailed(OSStatus)
    case keychainReadFailed(OSStatus)
    case keychainWriteFailed(OSStatus)
    case keyUnavailable
}

/// The stored line, not the bytes inside it: the marker, the cipher and the
/// framing are one format, owned by `core/src/crypt.rs`, so a host that keeps
/// its own key implements this and nothing here knows how a row is shaped.
nonisolated protocol ActivityStorageLineCrypting: Sendable {
    /// One journal line, sealed — whatever framing the format calls for included.
    func seal(_ payload: Data) throws -> String
    /// The bytes behind a stored line, or `nil` when that line was never
    /// encrypted. Throws when the line is encrypted and unreadable, so a row
    /// this machine cannot decrypt is never mistaken for plaintext json.
    func open(_ line: String) throws -> Data?
}

nonisolated protocol ActivityStorageKeyProviding: Sendable {
    func loadOrCreateKey() throws -> Data
}

nonisolated final class ActivityStorageLineCodec: @unchecked Sendable {
    private let preferencesStore: (any ActivityStoragePreferencesPersisting)?
    private let cryptor: any ActivityStorageLineCrypting

    init(
        preferencesStore: (any ActivityStoragePreferencesPersisting)? = nil,
        cryptor: any ActivityStorageLineCrypting = PlaintextActivityStorageCryptor()
    ) {
        self.preferencesStore = preferencesStore
        self.cryptor = cryptor
    }

    static let plaintext = ActivityStorageLineCodec()

    /// Loads the storage key without writing a journal row. Called at launch so
    /// file errors or a legacy migration prompt appear while the user is in the
    /// app instead of stalling the first background write minutes later.
    func prepare() throws {
        guard preferencesStore?.loadPreferences().encryptNewData == true else {
            return
        }
        _ = try cryptor.seal(Data())
    }

    func encode(_ payload: Data) throws -> String {
        guard preferencesStore?.loadPreferences().encryptNewData == true else {
            return String(decoding: payload, as: UTF8.self)
        }

        return try cryptor.seal(payload)
    }

    func decode(_ line: some StringProtocol) throws -> Data {
        let text = String(line)
        return try cryptor.open(text) ?? Data(text.utf8)
    }
}

nonisolated final class CachingActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    /// One attempt at the underlying provider, shared by everyone waiting on it.
    private final class Load: @unchecked Sendable {
        let gate = DispatchSemaphore(value: 0)
        var result: Result<Data, any Error>?
    }

    private let lock = NSLock()
    private let underlying: any ActivityStorageKeyProviding
    private let timeout: DispatchTimeInterval
    private let queue = DispatchQueue(label: "com.ctrlcreeper.Pathlight.activity-storage-key")
    private var cachedKey: Data?
    private var inFlight: Load?

    init(
        wrapping underlying: any ActivityStorageKeyProviding,
        timeout: DispatchTimeInterval = .seconds(5)
    ) {
        self.underlying = underlying
        self.timeout = timeout
    }

    func loadOrCreateKey() throws -> Data {
        lock.lock()
        if let cachedKey {
            lock.unlock()
            return cachedKey
        }
        let load = inFlight ?? startLoad()
        lock.unlock()

        // A migration prompt or a wedged securityd may never return, and the
        // journal writer calls this synchronously. Bound the wait so the caller gets an
        // error it can report instead of stalling recording forever; the attempt
        // keeps running on its own queue and a later call picks up its result.
        guard load.gate.wait(timeout: .now() + timeout) == .success else {
            throw ActivityStorageLineCodecError.keyUnavailable
        }
        load.gate.signal()

        lock.lock()
        defer {
            lock.unlock()
        }
        guard let result = load.result else {
            throw ActivityStorageLineCodecError.keyUnavailable
        }
        // Failures are never cached; the next call retries the underlying provider.
        return try result.get()
    }

    private func startLoad() -> Load {
        let load = Load()
        inFlight = load
        let underlying = self.underlying
        queue.async { [self] in
            let result = Result { try underlying.loadOrCreateKey() }
            lock.lock()
            load.result = result
            if case .success(let key) = result {
                cachedKey = key
            }
            if inFlight === load {
                inFlight = nil
            }
            lock.unlock()
            load.gate.signal()
        }
        return load
    }
}

nonisolated final class KeychainActivityStorageKeyProvider: @unchecked Sendable, ActivityStorageKeyProviding {
    static let live = KeychainActivityStorageKeyProvider(
        service: "com.ctrlcreeper.Pathlight.activity-storage",
        account: "pathlight-aes-gcm-v1"
    )

    private let service: String
    private let account: String

    init(service: String, account: String) {
        self.service = service
        self.account = account
    }

    func loadOrCreateKey() throws -> Data {
        if let key = try loadKey() {
            return key
        }

        let key = try generateKey()
        if try saveKey(key) {
            return key
        }
        // Another writer stored a key between our load and save; ours was never
        // persisted, so encrypting with it would make the data undecryptable.
        guard let storedKey = try loadKey() else {
            throw ActivityStorageLineCodecError.keychainReadFailed(errSecItemNotFound)
        }
        return storedKey
    }

    /// Migration-only read. Unlike `loadOrCreateKey`, this never creates a
    /// second authority when the shared key file does not exist yet.
    func loadExistingKey() throws -> Data? {
        try loadKey()
    }

    func deleteKey() throws {
        let status = SecItemDelete(baseQuery() as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw ActivityStorageLineCodecError.keychainWriteFailed(status)
        }
    }

    private func loadKey() throws -> Data? {
        var query = baseQuery()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        switch status {
        case errSecSuccess:
            return item as? Data
        case errSecItemNotFound:
            return nil
        default:
            throw ActivityStorageLineCodecError.keychainReadFailed(status)
        }
    }

    private func saveKey(_ key: Data) throws -> Bool {
        var query = baseQuery()
        query[kSecValueData as String] = key
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

        let status = SecItemAdd(query as CFDictionary, nil)
        switch status {
        case errSecSuccess:
            return true
        case errSecDuplicateItem:
            return false
        default:
            throw ActivityStorageLineCodecError.keychainWriteFailed(status)
        }
    }

    private func generateKey() throws -> Data {
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        guard status == errSecSuccess else {
            throw ActivityStorageLineCodecError.randomGenerationFailed(status)
        }
        return Data(bytes)
    }

    private func baseQuery() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
    }
}

/// The key file shared with the Rust CLI and egui host. An existing Keychain
/// key seeds it once, preserving encrypted rows from older macOS builds; after
/// that the file is the sole authority for every host.
nonisolated final class FileActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    static let live = FileActivityStorageKeyProvider(
        keyURL: JSONLActivityEventStore.defaultJournalURL()
            .deletingPathExtension()
            .appendingPathExtension("key"),
        legacyKeyLoader: { try KeychainActivityStorageKeyProvider.live.loadExistingKey() }
    )

    private let keyURL: URL
    private let legacyKeyLoader: @Sendable () throws -> Data?

    init(
        keyURL: URL,
        legacyKeyLoader: @escaping @Sendable () throws -> Data? = { nil }
    ) {
        self.keyURL = keyURL
        self.legacyKeyLoader = legacyKeyLoader
    }

    func loadOrCreateKey() throws -> Data {
        if let key = try loadFileKey() {
            return key
        }

        // Do not hold the cross-process file lock across a possible Keychain
        // prompt. The file is checked again under the lock, so a CLI writer
        // that wins this race remains authoritative.
        let candidate = try legacyKeyLoader() ?? generateKey()
        return try ActivityStorageFileProtection.withStorageLock(
            in: keyURL.deletingLastPathComponent()
        ) {
            if let key = try loadFileKey() {
                return key
            }
            guard candidate.count == 32 else {
                throw ActivityStorageLineCodecError.keyUnavailable
            }
            guard FileManager.default.createFile(
                atPath: keyURL.path,
                contents: nil,
                attributes: [.posixPermissions: 0o600]
            ) else {
                guard let key = try loadFileKey() else {
                    throw ActivityStorageLineCodecError.keyUnavailable
                }
                return key
            }
            do {
                let handle = try FileHandle(forWritingTo: keyURL)
                defer { try? handle.close() }
                try handle.write(contentsOf: candidate)
                try handle.synchronize()
                try ActivityStorageFileProtection.applyProtectedFilePermissions(to: keyURL)
                return candidate
            } catch {
                try? FileManager.default.removeItem(at: keyURL)
                throw error
            }
        }
    }

    private func loadFileKey() throws -> Data? {
        guard FileManager.default.fileExists(atPath: keyURL.path) else { return nil }
        let key = try Data(contentsOf: keyURL)
        guard key.count == 32 else {
            throw ActivityStorageLineCodecError.keyUnavailable
        }
        try ActivityStorageFileProtection.applyProtectedFilePermissions(to: keyURL)
        return key
    }

    private func generateKey() throws -> Data {
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        guard status == errSecSuccess else {
            throw ActivityStorageLineCodecError.randomGenerationFailed(status)
        }
        return Data(bytes)
    }
}

/// Read-only adapter for the pre-shared-key Keychain item. It exists only so
/// old rows can be opened and rewritten under the shared file key; it never
/// creates another Keychain key.
nonisolated final class LegacyKeychainActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    private let keychain: KeychainActivityStorageKeyProvider

    init(keychain: KeychainActivityStorageKeyProvider = .live) {
        self.keychain = keychain
    }

    func loadOrCreateKey() throws -> Data {
        guard let key = try keychain.loadExistingKey() else {
            throw ActivityStorageLineCodecError.keyUnavailable
        }
        return key
    }
}

/// What a host without encryption does: write the line as it is, and treat an
/// encrypted row as one it cannot read rather than pretending to decode it.
nonisolated struct PlaintextActivityStorageCryptor: ActivityStorageLineCrypting {
    func seal(_ payload: Data) throws -> String {
        String(decoding: payload, as: UTF8.self)
    }

    func open(_ line: String) throws -> Data? {
        nil
    }
}
