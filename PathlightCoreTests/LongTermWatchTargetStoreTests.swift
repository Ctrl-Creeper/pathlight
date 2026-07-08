import XCTest
@testable import PathlightCore

final class LongTermWatchTargetStoreTests: XCTestCase {
    func testUserDefaultsPersistenceRoundTripsTargets() {
        let defaults = makeIsolatedLongTermWatchDefaults()
        let persistence = UserDefaultsLongTermWatchTargetPersistence(defaults: defaults)
        let target = LongTermWatchTarget(
            rootPath: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory),
            isEnabled: true,
            options: LongTermWatchTargetOptions(
                minimumRecordedByteDelta: 512 * 1_024,
                aggregationWindow: 120,
                recordsFileNames: false
            ),
            baseline: ActivityBaselineSnapshot(
                rootPath: URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory),
                capturedAt: Date(timeIntervalSince1970: 100),
                allocatedSize: 2_048,
                measuredItemCount: 3,
                unreadableItemCount: 1
            )
        )

        persistence.saveTargets([target])

        XCTAssertEqual(persistence.loadTargets(), [target])
    }

    func testStoreUpsertsDeduplicatesAndPersistsTargets() {
        let first = LongTermWatchTarget(rootPath: URL(filePath: "/watched/first", directoryHint: .isDirectory))
        let duplicate = LongTermWatchTarget(rootPath: URL(filePath: "/watched/first", directoryHint: .isDirectory), isEnabled: false)
        let second = LongTermWatchTarget(rootPath: URL(filePath: "/watched/second", directoryHint: .isDirectory))
        let persistence = RecordingLongTermWatchTargetPersistence(targets: [first])
        let store = LongTermWatchTargetStore(persistence: persistence)

        let updatedTargets = store.upsert(duplicate, currentTargets: [first, second])

        XCTAssertEqual(updatedTargets, [duplicate, second])
        XCTAssertEqual(persistence.savedTargets, [[duplicate, second]])
    }

    func testStoreUpdatesEnabledState() {
        let enabled = LongTermWatchTarget(rootPath: URL(filePath: "/watched/downloads", directoryHint: .isDirectory))
        let persistence = RecordingLongTermWatchTargetPersistence(targets: [enabled])
        let store = LongTermWatchTargetStore(persistence: persistence)

        let updatedTargets = store.setEnabled(
            false,
            forRootPath: enabled.rootPath,
            currentTargets: [enabled]
        )

        XCTAssertEqual(updatedTargets.first?.isEnabled, false)
        XCTAssertEqual(persistence.savedTargets, [updatedTargets])
    }
}

private final class RecordingLongTermWatchTargetPersistence: LongTermWatchTargetPersisting {
    var targets: [LongTermWatchTarget]
    var savedTargets: [[LongTermWatchTarget]] = []
    var didClear = false

    init(targets: [LongTermWatchTarget] = []) {
        self.targets = targets
    }

    func loadTargets() -> [LongTermWatchTarget] {
        targets
    }

    func saveTargets(_ targets: [LongTermWatchTarget]) {
        self.targets = targets
        savedTargets.append(targets)
    }

    func clearTargets() {
        didClear = true
        targets = []
    }
}

private func makeIsolatedLongTermWatchDefaults(
    file: StaticString = #filePath,
    line: UInt = #line
) -> UserDefaults {
    let suiteName = "PathlightLongTermWatchTests.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suiteName) else {
        XCTFail("Could not create isolated UserDefaults suite.", file: file, line: line)
        return .standard
    }

    defaults.removePersistentDomain(forName: suiteName)
    return defaults
}
