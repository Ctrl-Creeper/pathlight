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

    /// The threshold is the setting that decides how close to the filesystem a
    /// watch reports, so it has to be editable while the watch runs — not only
    /// at the sheet that started it.
    func testEditingRecordingFiltersChangesTheThresholdOnARunningTarget() {
        let persistence = UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
        let store = LongTermWatchTargetStore(persistence: persistence)
        let rootPath = URL(filePath: "/tmp/pathlight-tests/threshold", directoryHint: .isDirectory)
        _ = store.upsert(LongTermWatchTarget(rootPath: rootPath, isEnabled: false), currentTargets: [])
        let model = AppModel(dependencies: makeDependencies(longTermWatchTargets: store))
        defer { model.cleanup() }
        let before = model.longTermWatchTargets.first?.options.minimumRecordedByteDelta
        XCTAssertEqual(before, LongTermWatchTargetOptions.defaultMinimumRecordedByteDelta)

        model.setRecordingFilters(
            patterns: [],
            minimumRecordedByteDelta: 0,
            monitorLatency: 30,
            minimumFileBytes: nil,
            maximumFileBytes: nil,
            rootPath: rootPath
        )

        XCTAssertEqual(model.longTermWatchTargets.first?.options.minimumRecordedByteDelta, 0)
        XCTAssertEqual(model.longTermWatchTargets.first?.options.monitorLatency, 30)
        // Saved, not only held: the next launch has to open on it.
        let reloaded = LongTermWatchTargetStore(persistence: persistence).loadTargets().first
        XCTAssertEqual(reloaded?.options.minimumRecordedByteDelta, 0)
        XCTAssertEqual(reloaded?.options.monitorLatency, 30)
        // A negative threshold is not a smaller one; it is nonsense that would
        // read as "record everything" by accident.
        model.setRecordingFilters(
            patterns: [],
            minimumRecordedByteDelta: -5,
            monitorLatency: 10_000,
            minimumFileBytes: nil,
            maximumFileBytes: nil,
            rootPath: rootPath
        )
        XCTAssertEqual(model.longTermWatchTargets.first?.options.minimumRecordedByteDelta, 0)
        XCTAssertEqual(
            model.longTermWatchTargets.first?.options.monitorLatency,
            MonitoringStartConfiguration.maximumMonitorLatency
        )
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

    /// Every row's switch at once: the folder stays listed, switched off,
    /// which is what `stop_all_switches_every_watch_off` promises of `gui/`.
    func testStopAllSwitchesEveryWatchOff() {
        let store = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence(defaults: makeDefaults())
        )
        let rootPath = FileManager.default.temporaryDirectory
            .appending(path: "PathlightStopAll-\(UUID().uuidString)", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: rootPath, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: rootPath) }
        _ = store.upsert(LongTermWatchTarget(rootPath: rootPath, isEnabled: true), currentTargets: [])

        let model = AppModel(dependencies: makeDependencies(longTermWatchTargets: store))
        defer { model.cleanup() }
        XCTAssertNotEqual(model.longTermWatchRuntimeStatuses[rootPath.path]?.state, .paused)

        model.stopAllMonitoring()

        XCTAssertEqual(model.longTermWatchTargets.map(\.isEnabled), [false])
        XCTAssertEqual(model.longTermWatchRuntimeStatuses[rootPath.path]?.state, .paused)
    }

    func testFinishedLiveMonitorIsNotLeftActive() async throws {
        let model = AppModel(dependencies: makeDependencies())
        defer { model.cleanup() }

        model.startShortTermWatch(
            rootPath: URL(filePath: "/tmp/pathlight-tests/finished", directoryHint: .isDirectory)
        )
        try await Task.sleep(for: .milliseconds(100))

        XCTAssertNil(model.liveWatchSession)
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
