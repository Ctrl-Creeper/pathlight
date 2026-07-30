import CryptoKit
import Foundation
import Security

nonisolated enum ActivityStorageLineCodecError: Error {
    case invalidEncryptedLine
    case missingCombinedRepresentation
    case randomGenerationFailed(OSStatus)
    case keychainReadFailed(OSStatus)
    case keychainWriteFailed(OSStatus)
}

nonisolated protocol ActivityStorageLineCrypting: Sendable {
    func encrypt(_ plaintext: Data) throws -> Data
    func decrypt(_ ciphertext: Data) throws -> Data
}

nonisolated protocol ActivityStorageKeyProviding: Sendable {
    func loadOrCreateKey() throws -> Data
}

nonisolated final class ActivityStorageLineCodec: @unchecked Sendable {
    private static let encryptedPrefix = "pathlight:v1:aes-gcm:"

    private let preferencesStore: (any ActivityStoragePreferencesPersisting)?
    private let cryptor: any ActivityStorageLineCrypting

    init(
        preferencesStore: (any ActivityStoragePreferencesPersisting)? = nil,
        cryptor: any ActivityStorageLineCrypting = AESGCMActivityStorageCryptor.live
    ) {
        self.preferencesStore = preferencesStore
        self.cryptor = cryptor
    }

    static let plaintext = ActivityStorageLineCodec(cryptor: PassthroughActivityStorageCryptor())

    func encode(_ payload: Data) throws -> String {
        guard preferencesStore?.loadPreferences().encryptNewData == true else {
            return String(decoding: payload, as: UTF8.self)
        }

        let encryptedPayload = try cryptor.encrypt(payload)
        return Self.encryptedPrefix + encryptedPayload.base64EncodedString()
    }

    func decode(_ line: some StringProtocol) throws -> Data {
        let text = String(line)
        guard text.hasPrefix(Self.encryptedPrefix) else {
            return Data(text.utf8)
        }

        let encodedPayload = text.dropFirst(Self.encryptedPrefix.count)
        guard let encryptedPayload = Data(base64Encoded: String(encodedPayload)) else {
            throw ActivityStorageLineCodecError.invalidEncryptedLine
        }
        return try cryptor.decrypt(encryptedPayload)
    }
}

nonisolated struct AESGCMActivityStorageCryptor: ActivityStorageLineCrypting {
    let keyProvider: any ActivityStorageKeyProviding

    static let live = AESGCMActivityStorageCryptor(
        keyProvider: CachingActivityStorageKeyProvider(
            wrapping: KeychainActivityStorageKeyProvider(
                service: "com.ctrlcreeper.Pathlight.activity-storage",
                account: "pathlight-aes-gcm-v1"
            )
        )
    )

    func encrypt(_ plaintext: Data) throws -> Data {
        let key = SymmetricKey(data: try keyProvider.loadOrCreateKey())
        let sealedBox = try AES.GCM.seal(plaintext, using: key)
        guard let combined = sealedBox.combined else {
            throw ActivityStorageLineCodecError.missingCombinedRepresentation
        }
        return combined
    }

    func decrypt(_ ciphertext: Data) throws -> Data {
        let key = SymmetricKey(data: try keyProvider.loadOrCreateKey())
        let sealedBox = try AES.GCM.SealedBox(combined: ciphertext)
        return try AES.GCM.open(sealedBox, using: key)
    }
}

nonisolated final class CachingActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    private let lock = NSLock()
    private let underlying: any ActivityStorageKeyProviding
    private var cachedKey: Data?

    init(wrapping underlying: any ActivityStorageKeyProviding) {
        self.underlying = underlying
    }

    func loadOrCreateKey() throws -> Data {
        lock.lock()
        defer {
            lock.unlock()
        }
        if let cachedKey {
            return cachedKey
        }
        // Failures are never cached; the next call retries the underlying provider.
        let key = try underlying.loadOrCreateKey()
        cachedKey = key
        return key
    }
}

nonisolated final class KeychainActivityStorageKeyProvider: @unchecked Sendable, ActivityStorageKeyProviding {
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

nonisolated private struct PassthroughActivityStorageCryptor: ActivityStorageLineCrypting {
    func encrypt(_ plaintext: Data) throws -> Data {
        plaintext
    }

    func decrypt(_ ciphertext: Data) throws -> Data {
        ciphertext
    }
}
