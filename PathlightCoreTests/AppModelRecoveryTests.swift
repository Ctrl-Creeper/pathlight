import Foundation
import XCTest
@testable import PathlightCore

@MainActor
final class AppModelRecoveryTests: XCTestCase {
    func testSubscribesBeforeStartingInitialBaseline() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: 1, monitor: monitor)
        let (model, _, root) = makeModel(monitor: monitor, scan: scan)
        defer { scan.release(); model.cleanup() }

        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("initial baseline started") { scan.invocationCount == 1 }

        XCTAssertTrue(scan.wasSubscribedAtFirstScan, "register monitoring before enumerating the tree")
        XCTAssertEqual(model.longTermWatchTargets.first?.rootPath, root)
        XCTAssertNil(model.longTermWatchTargets.first?.baseline, "target must exist while its scan is still running")
    }

    func testGapScanDoesNotBlockEventsAndLaterGapCanRescan() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: 2, monitor: monitor)
        let (model, _, root) = makeModel(monitor: monitor, scan: scan)
        defer { scan.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("initial baseline stored") { model.longTermWatchTargets.first?.baseline != nil }
        let initialCapture = try XCTUnwrap(model.longTermWatchTargets.first?.baseline?.capturedAt)

        monitor.send(.requiresRescan(eventID: 10))
        try await eventually("gap baseline started") { scan.invocationCount == 2 }
        let changedAt = Date()
        monitor.send(.change(
            DiskActivityChange(kind: .created, path: root.appending(path: "during-scan.bin"), rootPath: root, timestamp: changedAt),
            eventID: 11
        ))

        try await eventually("event consumed while gap baseline remains blocked") {
            model.longTermWatchRuntimeStatuses[root.path]?.lastActivityAt == changedAt
        }
        XCTAssertEqual(scan.completedCount, 1, "the new event must arrive before the blocked scan finishes")
        scan.release()
        try await eventually("gap baseline stored") {
            model.longTermWatchTargets.first?.baseline?.capturedAt != initialCapture
        }
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.hasHistoryGap, true)
        XCTAssertFalse(model.longTermWatchTargets.first?.baseline?.isUsableForReconciliation ?? true,
                       "delayed notifications do not establish a verified scan boundary")

        monitor.send(.requiresRescan(eventID: 20))
        try await eventually("a separate later gap starts another scan") { scan.invocationCount == 3 }
    }

    func testRemovingTargetDuringScanPreventsLateResurrection() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: 1, monitor: monitor)
        let (model, persistence, root) = makeModel(monitor: monitor, scan: scan)
        defer { scan.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("initial baseline started") { scan.invocationCount == 1 }

        model.removeLongTermWatchTarget(rootPath: root)
        scan.release()
        try await eventually("cancelled enumeration returned") { scan.completedCount == 1 }
        // Allow the returned scan result to reach the model's main-actor continuation.
        try await Task.sleep(for: .milliseconds(100))

        XCTAssertTrue(model.longTermWatchTargets.isEmpty)
        XCTAssertTrue(persistence.loadTargets().isEmpty)
        XCTAssertNil(model.longTermWatchRuntimeStatuses[root.path])
    }

    private var detailedOptions: LongTermWatchTargetOptions {
        LongTermWatchTargetOptions(minimumRecordedByteDelta: 1, aggregationWindow: 0, recordsFileNames: true, exclusionPatterns: [])
    }

    private func makeModel(monitor: RecoveryTestMonitor, scan: RecoveryScanGate) -> (AppModel, UserDefaultsLongTermWatchTargetPersistence, URL) {
        let suite = "AppModelRecoveryTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        let persistence = UserDefaultsLongTermWatchTargetPersistence(defaults: defaults)
        let root = URL(filePath: "/tmp/pathlight-recovery-\(UUID().uuidString)", directoryHint: .isDirectory)
        let model = AppModel(dependencies: AppDependencies(
            systemActions: .inert,
            activityMonitor: monitor,
            activitySizeProvider: { _ in 4_096 },
            activityEventStore: nil,
            longTermWatchTargets: LongTermWatchTargetStore(persistence: persistence),
            activityBaselineService: ActivityBaselineService(
                measurementProvider: { _ in
                    ActivityBaselineService.Measurement(
                        allocatedSize: 0,
                        identity: ActivityBaselineService.ObjectIdentity(device: 1, inode: 1)
                    )
                },
                contentsProvider: { scan.contents(at: $0) },
            ),
            activityStoragePreferences: FixedActivityStoragePreferencesStore(encryptNewData: false),
            launchAtLoginService: RecoveryTestLoginService()
        ))
        return (model, persistence, root)
    }

    private func eventually(_ description: String, _ predicate: @MainActor () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while !predicate() {
            guard ContinuousClock.now < deadline else {
                XCTFail("Timed out waiting for \(description)")
                throw RecoveryTestError.timedOut
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}

private enum RecoveryTestError: Error { case timedOut }

private final class RecoveryTestMonitor: DiskActivityMonitoring, @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: AsyncStream<DiskActivityStreamEvent>.Continuation?

    var isSubscribed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return continuation != nil
    }

    func events(for root: URL, since eventID: UInt64?, latency: TimeInterval) -> AsyncStream<DiskActivityStreamEvent> {
        AsyncStream { continuation in
            lock.lock()
            self.continuation = continuation
            lock.unlock()
            continuation.yield(.historyCaughtUp(eventID: 1))
        }
    }

    func send(_ event: DiskActivityStreamEvent) {
        lock.lock()
        let current = continuation
        lock.unlock()
        current?.yield(event)
    }
}

/// Blocks only the selected traversal, outside the main actor, with a hard
/// deadline so a regression cannot leave the test process indefinitely hung.
private final class RecoveryScanGate: @unchecked Sendable {
    private let lock = NSLock()
    private let semaphore = DispatchSemaphore(value: 0)
    private let blockingScan: Int
    private let monitor: RecoveryTestMonitor
    private var invocations = 0
    private var completed = 0
    private var subscribedAtFirstScan = false

    init(blockingScan: Int, monitor: RecoveryTestMonitor) {
        self.blockingScan = blockingScan
        self.monitor = monitor
    }

    var invocationCount: Int { lock.withLock { invocations } }
    var completedCount: Int { lock.withLock { completed } }
    var wasSubscribedAtFirstScan: Bool { lock.withLock { subscribedAtFirstScan } }

    func contents(at url: URL) -> [URL] {
        let subscribed = monitor.isSubscribed
        let scan = lock.withLock {
            invocations += 1
            if invocations == 1 { subscribedAtFirstScan = subscribed }
            return invocations
        }
        if scan == blockingScan { _ = semaphore.wait(timeout: .now() + 3) }
        lock.withLock { completed += 1 }
        return []
    }

    func release() { semaphore.signal() }
}

private final class RecoveryTestLoginService: LaunchAtLoginControlling {
    func currentStatus() -> LaunchAtLoginStatus { .disabled }
    func setEnabled(_ enabled: Bool) throws {}
    func openLoginItemsSettings() {}
}
