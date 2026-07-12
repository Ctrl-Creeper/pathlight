import XCTest
@testable import PathlightCore

final class ActivityStoragePreferencesStoreTests: XCTestCase {
    func testLoadPreferencesUsesDefaultsWhenValuesAreMissing() {
        let store = UserDefaultsActivityStoragePreferencesStore(defaults: makeIsolatedActivityDefaults())

        XCTAssertEqual(store.loadPreferences(), .defaults)
    }

    func testSaveAndReloadPreferencesRoundTripsValues() {
        let store = UserDefaultsActivityStoragePreferencesStore(defaults: makeIsolatedActivityDefaults())
        let preferences = ActivityStoragePreferences(
            detailedRetentionDays: 365,
            aggregateRetentionDays: 1_825,
            storageLimitBytes: 5 * 1_024 * 1_024 * 1_024,
            encryptNewData: false
        )

        store.savePreferences(preferences)

        XCTAssertEqual(store.loadPreferences(), preferences)
    }

    func testLoadPreferencesClampsInvalidValues() {
        let defaults = makeIsolatedActivityDefaults()
        defaults.set(-10, forKey: "activityDetailedRetentionDays")
        defaults.set(0, forKey: "activityAggregateRetentionDays")
        defaults.set(10, forKey: "activityStorageLimitBytes")
        let store = UserDefaultsActivityStoragePreferencesStore(defaults: defaults)

        XCTAssertEqual(store.loadPreferences(), .defaults)
    }
}

private func makeIsolatedActivityDefaults(
    file: StaticString = #filePath,
    line: UInt = #line
) -> UserDefaults {
    let suiteName = "PathlightActivityStorageTests.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suiteName) else {
        XCTFail("Could not create isolated UserDefaults suite.", file: file, line: line)
        return .standard
    }

    defaults.removePersistentDomain(forName: suiteName)
    return defaults
}
