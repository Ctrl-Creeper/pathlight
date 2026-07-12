import Foundation
@testable import PathlightCore

func makeEncryptedActivityStorageLineCodec(encryptNewData: Bool = true) -> ActivityStorageLineCodec {
    ActivityStorageLineCodec(
        preferencesStore: FixedActivityStoragePreferencesStore(encryptNewData: encryptNewData),
        cryptor: AESGCMActivityStorageCryptor(keyProvider: FixedActivityStorageKeyProvider())
    )
}

func posixPermissions(at url: URL) throws -> Int {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return attributes[.posixPermissions] as? Int ?? 0
}

final class FixedActivityStoragePreferencesStore: ActivityStoragePreferencesPersisting {
    private let encryptNewData: Bool

    init(encryptNewData: Bool) {
        self.encryptNewData = encryptNewData
    }

    func loadPreferences() -> ActivityStoragePreferences {
        var preferences = ActivityStoragePreferences.defaults
        preferences.encryptNewData = encryptNewData
        return preferences
    }

    func savePreferences(_ preferences: ActivityStoragePreferences) {}
}

private struct FixedActivityStorageKeyProvider: ActivityStorageKeyProviding {
    func loadOrCreateKey() throws -> Data {
        Data(repeating: 7, count: 32)
    }
}
