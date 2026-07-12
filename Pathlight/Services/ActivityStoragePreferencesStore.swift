import Foundation

nonisolated struct ActivityStoragePreferences: Equatable, Sendable {
    var detailedRetentionDays: Int
    var aggregateRetentionDays: Int
    var storageLimitBytes: Int64
    var encryptNewData: Bool

    static let defaults = ActivityStoragePreferences(
        detailedRetentionDays: 180,
        aggregateRetentionDays: 730,
        storageLimitBytes: 1_024 * 1_024 * 1_024,
        encryptNewData: true
    )
}

nonisolated protocol ActivityStoragePreferencesPersisting: AnyObject {
    func loadPreferences() -> ActivityStoragePreferences
    func savePreferences(_ preferences: ActivityStoragePreferences)
}

nonisolated final class UserDefaultsActivityStoragePreferencesStore: ActivityStoragePreferencesPersisting {
    private enum Key {
        static let detailedRetentionDays = "activityDetailedRetentionDays"
        static let aggregateRetentionDays = "activityAggregateRetentionDays"
        static let storageLimitBytes = "activityStorageLimitBytes"
        static let encryptNewData = "activityEncryptNewData"
    }

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func loadPreferences() -> ActivityStoragePreferences {
        ActivityStoragePreferences(
            detailedRetentionDays: validDays(
                defaults.integer(forKey: Key.detailedRetentionDays),
                fallback: ActivityStoragePreferences.defaults.detailedRetentionDays
            ),
            aggregateRetentionDays: validDays(
                defaults.integer(forKey: Key.aggregateRetentionDays),
                fallback: ActivityStoragePreferences.defaults.aggregateRetentionDays
            ),
            storageLimitBytes: validStorageLimit(
                Int64(defaults.integer(forKey: Key.storageLimitBytes)),
                fallback: ActivityStoragePreferences.defaults.storageLimitBytes
            ),
            encryptNewData: boolValue(
                forKey: Key.encryptNewData,
                fallback: ActivityStoragePreferences.defaults.encryptNewData
            )
        )
    }

    func savePreferences(_ preferences: ActivityStoragePreferences) {
        defaults.set(preferences.detailedRetentionDays, forKey: Key.detailedRetentionDays)
        defaults.set(preferences.aggregateRetentionDays, forKey: Key.aggregateRetentionDays)
        defaults.set(preferences.storageLimitBytes, forKey: Key.storageLimitBytes)
        defaults.set(preferences.encryptNewData, forKey: Key.encryptNewData)
    }

    private func validDays(_ value: Int, fallback: Int) -> Int {
        (1...36_500).contains(value) ? value : fallback
    }

    private func validStorageLimit(_ value: Int64, fallback: Int64) -> Int64 {
        (100 * 1_024 * 1_024...Int64.max).contains(value) ? value : fallback
    }

    private func boolValue(forKey key: String, fallback: Bool) -> Bool {
        guard defaults.object(forKey: key) != nil else {
            return fallback
        }
        return defaults.bool(forKey: key)
    }
}
