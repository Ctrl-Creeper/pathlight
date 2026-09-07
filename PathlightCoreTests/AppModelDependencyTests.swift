import XCTest
@testable import PathlightCore

@MainActor
final class AppModelDependencyTests: XCTestCase {
    func testLoadsPersistedLongTermWatchTargets() {
        let store = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
        )
        let target = LongTermWatchTarget(
            rootPath: URL(filePath: "/tmp/pathlight-tests/watched", directoryHint: .isDirectory),
            isEnabled: false
        )
        _ = store.upsert(target, currentTargets: [])

        let model = AppModel(dependencies: makeDependencies(longTermWatchTargets: store))
        defer { model.cleanup() }

        XCTAssertEqual(model.longTermWatchTargets.map(\.id), [target.id])
        XCTAssertNil(model.liveWatchSession)
        XCTAssertFalse(model.shouldShowLaunchAtLoginNudge)
    }

    func testRemovingTargetPersistsAndClearsRuntimeStatus() {
        let persistence = UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
        let store = LongTermWatchTargetStore(persistence: persistence)
        let rootPath = URL(filePath: "/tmp/pathlight-tests/removed", directoryHint: .isDirectory)
        _ = store.upsert(LongTermWatchTarget(rootPath: rootPath, isEnabled: false), currentTargets: [])

        let model = AppModel(dependencies: makeDependencies(longTermWatchTargets: store))
        defer { model.cleanup() }

        model.removeLongTermWatchTarget(rootPath: rootPath)

        XCTAssertTrue(model.longTermWatchTargets.isEmpty)
        XCTAssertNil(model.longTermWatchRuntimeStatuses[rootPath.path])
        XCTAssertTrue(persistence.loadTargets().isEmpty)
    }

    func testLoadsActivityStoragePreferencesFromInjectedStore() {
        let model = AppModel(
            dependencies: makeDependencies(
                activityStoragePreferences: FixedActivityStoragePreferencesStore(encryptNewData: true)
            )
        )
        defer { model.cleanup() }

        XCTAssertTrue(model.activityEncryptNewData)
    }

    func testLaunchAtLoginUsesInjectedService() {
        let service = SpyLaunchAtLoginService(status: .disabled)
        let model = AppModel(dependencies: makeDependencies(launchAtLoginService: service))
        defer { model.cleanup() }

        XCTAssertEqual(model.launchAtLoginStatus, .disabled)

        model.setLaunchAtLoginEnabled(true)

        XCTAssertEqual(service.enabledRequests, [true])
        XCTAssertEqual(model.launchAtLoginStatus, .enabled)
    }

    private func makeDefaults() -> UserDefaults {
        UserDefaults(suiteName: "AppModelDependencyTests.\(UUID().uuidString)")!
    }

    private func makeDependencies(
        longTermWatchTargets: LongTermWatchTargetStore? = nil,
        activityStoragePreferences: (any ActivityStoragePreferencesPersisting)? = nil,
        launchAtLoginService: (any LaunchAtLoginControlling)? = nil
    ) -> AppDependencies {
        AppDependencies(
            systemActions: .inert,
            activityEventStore: nil,
            longTermWatchTargets: longTermWatchTargets ?? LongTermWatchTargetStore(
                persistence: UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
            ),
            activityStoragePreferences: activityStoragePreferences
                ?? FixedActivityStoragePreferencesStore(encryptNewData: false),
            launchAtLoginService: launchAtLoginService ?? SpyLaunchAtLoginService(status: .disabled)
        )
    }
}

private final class SpyLaunchAtLoginService: LaunchAtLoginControlling {
    private var status: LaunchAtLoginStatus
    private(set) var enabledRequests: [Bool] = []

    init(status: LaunchAtLoginStatus) {
        self.status = status
    }

    func currentStatus() -> LaunchAtLoginStatus {
        status
    }

    func setEnabled(_ enabled: Bool) throws {
        enabledRequests.append(enabled)
        status = enabled ? .enabled : .disabled
    }

    func openLoginItemsSettings() {}
}
