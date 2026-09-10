import XCTest
@testable import PathlightCore

@MainActor
final class AppModelDependencyTests: XCTestCase {
    func testBaselineDoesNotOverwriteEventAttributionIndex() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "PathlightBaselineIndex-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appending(path: "changed.bin")
        try Data(repeating: 0x42, count: 16_384).write(to: file)
        let sizeIndex = ActivitySizeIndex()
        let scope = ActivitySizeProviders.scope(kind: "long-term", rootPath: root)
        sizeIndex.recordKnownSize(4_096, for: file, scope: scope)
        let dependencies = AppDependencies.live(
            activityMonitor: InertDiskActivityMonitor(),
            activityAttribution: stubActivityAttribution,
            // Nothing is excluded and nothing is an anomaly here: both live
            // in the Rust core, which this package does not link.
            activityExclusion: { _, _ in nil },
            activityAnomalies: { _, _, _ in [] },
            activityStorageCryptor: PlaintextActivityStorageCryptor(),
            activitySizeIndex: sizeIndex
        )

        let baseline = await dependencies.activityBaselineService.captureBaseline(rootPath: root)

        XCTAssertGreaterThanOrEqual(baseline.allocatedSize, 16_384)
        XCTAssertEqual(dependencies.activitySizeProviders(scope).known(file), 4_096,
                       "a delayed modification still needs the pre-scan size for attribution")
        XCTAssertNil(sizeIndex.knownSize(for: root, scope: scope),
                     "enumeration must not seed paths into the live event index")
    }

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

    /// One switch for every watch: pausing ends what is running without
    /// forgetting the folder, and resuming starts the same one again — the
    /// promise `pausing_ends_every_watch_and_resuming_brings_them_back` makes
    /// of the other two hosts.
    func testPausingStopsEveryWatchAndResumingStartsTheSameOnes() {
        let store = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
        )
        let rootPath = FileManager.default.temporaryDirectory
            .appending(path: "PathlightPause-\(UUID().uuidString)", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: rootPath, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: rootPath) }
        _ = store.upsert(LongTermWatchTarget(rootPath: rootPath, isEnabled: true), currentTargets: [])

        let model = AppModel(dependencies: makeDependencies(longTermWatchTargets: store))
        defer {
            model.setMonitoringPaused(false)
            model.cleanup()
        }
        XCTAssertNotEqual(model.longTermWatchRuntimeStatuses[rootPath.path]?.state, .paused)

        model.setMonitoringPaused(true)

        XCTAssertTrue(model.isMonitoringPaused)
        XCTAssertEqual(model.longTermWatchRuntimeStatuses[rootPath.path]?.state, .paused)
        // The folder is still one this install watches, which is what makes
        // resuming the same set possible.
        XCTAssertEqual(model.longTermWatchTargets.map(\.isEnabled), [true])

        model.setMonitoringPaused(false)

        XCTAssertFalse(model.isMonitoringPaused)
        XCTAssertNotEqual(model.longTermWatchRuntimeStatuses[rootPath.path]?.state, .paused)
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
            activityMonitor: InertDiskActivityMonitor(),
            activityAttribution: stubActivityAttribution,
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
