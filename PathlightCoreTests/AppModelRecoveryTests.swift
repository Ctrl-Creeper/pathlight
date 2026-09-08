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

    func testDisconnectCancelsBaselineBeforeReconnectBackoff() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: 1, monitor: monitor)
        let (model, _, root) = makeModel(monitor: monitor, scan: scan)
        defer { scan.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("initial baseline started") { scan.invocationCount == 1 }

        monitor.finish()
        try await eventually("watch is reconnecting") {
            model.longTermWatchRuntimeStatuses[root.path]?.state == .reconnecting
        }
        scan.release()
        try await eventually("disconnected scan returned") { scan.completedCount == 1 }
        try await Task.sleep(for: .milliseconds(100))

        XCTAssertNil(model.longTermWatchTargets.first?.baseline,
                     "a scan whose source ended cannot publish during reconnect backoff")
    }

    func testCheckpointWaitsForJournalDurability() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = BlockingRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { store.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "durable.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 42
        ))
        try await eventually("journal append started") { store.appendStarted }

        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1,
                       "a crash before append completes must replay event 42")
        store.release()
        try await eventually("durable cursor advanced") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 42
        }
    }

    func testFailedJournalAppendIsRetriedBeforeCheckpointAdvances() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = FailOnceRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "retry.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 43
        ))
        try await eventually("first append failed") { store.appendCallCount >= 1 }
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1)

        try await eventually("failed batch retried", timeout: .seconds(4)) {
            store.persistedEventCount == 1
                && model.longTermWatchTargets.first?.checkpoint?.eventID == 43
        }
    }

    func testDeniedKeychainAccessIsReportedAtLaunch() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let attempts = RecoveryCallCounter()
        let (model, _, _) = makeModel(monitor: monitor, scan: scan, keyWarmUp: {
            attempts.increment()
            throw ActivityStorageLineCodecError.keychainReadFailed(-25293)
        })
        defer { model.cleanup() }

        try await eventually("keychain access reported") {
            model.monitoringStatusMessage?.contains("needs keychain access") == true
        }
        // A denial must not re-prompt; only a timeout is worth waiting out.
        XCTAssertEqual(attempts.count, 1)
    }

    func testSlowKeychainPromptIsWaitedOutWithoutComplaining() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let attempts = RecoveryCallCounter()
        let (model, _, _) = makeModel(monitor: monitor, scan: scan, keyWarmUp: {
            if attempts.increment() < 2 {
                throw ActivityStorageLineCodecError.keyUnavailable
            }
        })
        defer { model.cleanup() }

        try await eventually("warm-up retried past the timeout") { attempts.count == 2 }
        XCTAssertNil(model.monitoringStatusMessage)
    }

    func testJournalRetryBacksOffWhileFlushesKeepFailing() {
        XCTAssertEqual(AppModel.journalRetryDelay(failureCount: 0), 1)
        XCTAssertEqual(AppModel.journalRetryDelay(failureCount: 1), 2)
        XCTAssertEqual(AppModel.journalRetryDelay(failureCount: 3), 8)
        XCTAssertEqual(AppModel.journalRetryDelay(failureCount: 9), 30)
    }

    func testRepeatedJournalFailuresAreReportedInsteadOfRetriedSilently() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = AlwaysFailRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "unwritable.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 44
        ))

        try await eventually("failure reported", timeout: .seconds(12)) {
            model.monitoringStatusMessage?.contains("could not write activity history") == true
        }
        XCTAssertGreaterThanOrEqual(store.appendCallCount, 3)
        // Recording keeps retrying, so the cursor must stay behind the batch.
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1)
    }

    func testImmediateStopFlushKeepsCheckpointBehindInFlightAppend() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = BlockingRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { store.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        let changedAt = Date()
        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "stop-flush.bin"),
                rootPath: root,
                timestamp: changedAt
            ),
            eventID: 42
        ))
        try await eventually("event accepted") {
            model.longTermWatchRuntimeStatuses[root.path]?.lastActivityAt == changedAt
        }
        model.stopShortTermWatch()
        try await eventually("immediate append started") { store.appendStarted }

        monitor.send(.historyCaughtUp(eventID: 43))
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1,
                       "a stop-triggered append must remain an in-flight checkpoint barrier")

        store.release()
        try await eventually("later cursor committed") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 43
        }
    }

    func testReconnectResumesFromAcceptedInMemoryCursor() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = BlockingRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { store.release(); model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "before-disconnect.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 42
        ))
        try await eventually("journal append started") { store.appendStarted }
        monitor.finish()

        try await eventually("watcher reconnected", timeout: .seconds(4)) {
            monitor.subscriptionCount >= 2
        }
        XCTAssertEqual(monitor.mostRecentSinceEventID, 42,
                       "same-process reconnect must not replay the buffered event")
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1,
                       "the disk checkpoint still waits for the append")
    }

    func testUnknownCommitStatePausesRecordingWithoutAdvancingOrRetrying() async throws {
        let monitor = RecoveryTestMonitor()
        let scan = RecoveryScanGate(blockingScan: .max, monitor: monitor)
        let store = UnknownCommitRecoveryEventStore()
        let (model, _, root) = makeModel(monitor: monitor, scan: scan, eventStore: store)
        defer { model.cleanup() }
        model.enableLongTermWatch(rootPath: root, options: detailedOptions)
        try await eventually("watcher ready") {
            model.longTermWatchTargets.first?.checkpoint?.eventID == 1
        }

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "unknown-commit.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 42
        ))
        try await eventually("uncertain commit surfaced") {
            model.monitoringStatusMessage?.contains("recording is paused") == true
        }
        try await Task.sleep(for: .milliseconds(1_200))

        XCTAssertEqual(store.appendCallCount, 1, "an uncertain commit cannot be retried blindly")
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1)

        monitor.send(.change(
            DiskActivityChange(
                kind: .created,
                path: root.appending(path: "after-failure.bin"),
                rootPath: root,
                timestamp: Date()
            ),
            eventID: 43
        ))
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(store.appendCallCount, 1)
        XCTAssertEqual(model.longTermWatchTargets.first?.checkpoint?.eventID, 1)
    }

    private var detailedOptions: LongTermWatchTargetOptions {
        LongTermWatchTargetOptions(minimumRecordedByteDelta: 1, aggregationWindow: 0, recordsFileNames: true, exclusionPatterns: [])
    }

    private func makeModel(
        monitor: RecoveryTestMonitor,
        scan: RecoveryScanGate,
        eventStore: (any ActivityEventStoring)? = nil,
        keyWarmUp: @escaping @Sendable () throws -> Void = {}
    ) -> (AppModel, UserDefaultsLongTermWatchTargetPersistence, URL) {
        let suite = "AppModelRecoveryTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        let persistence = UserDefaultsLongTermWatchTargetPersistence(defaults: defaults)
        // The root has to exist: a watch over a path that is not there reports
        // `.rootMissing`, not `.reconnecting`.
        let root = URL(filePath: "/tmp/pathlight-recovery-\(UUID().uuidString)", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let model = AppModel(dependencies: AppDependencies(
            systemActions: .inert,
            activityMonitor: monitor,
            activitySizeProviders: { _ in ActivitySizeProviders(size: { _ in 4_096 }) },
            activityEventStore: eventStore,
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
            launchAtLoginService: RecoveryTestLoginService(),
            activityStorageKeyWarmUp: keyWarmUp
        ))
        return (model, persistence, root)
    }

    private func eventually(
        _ description: String,
        timeout: Duration = .seconds(2),
        _ predicate: @MainActor () -> Bool
    ) async throws {
        let deadline = ContinuousClock.now.advanced(by: timeout)
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
    private var sinceEventIDs: [UInt64?] = []

    var isSubscribed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return continuation != nil
    }

    var subscriptionCount: Int { lock.withLock { sinceEventIDs.count } }
    var mostRecentSinceEventID: UInt64? { lock.withLock { sinceEventIDs.last ?? nil } }

    func events(for root: URL, since eventID: UInt64?, latency: TimeInterval) -> AsyncStream<DiskActivityStreamEvent> {
        AsyncStream { continuation in
            lock.lock()
            self.continuation = continuation
            sinceEventIDs.append(eventID)
            lock.unlock()
            continuation.yield(.historyCaughtUp(eventID: eventID ?? 1))
        }
    }

    func send(_ event: DiskActivityStreamEvent) {
        lock.lock()
        let current = continuation
        lock.unlock()
        current?.yield(event)
    }

    func finish() {
        let current = lock.withLock { continuation }
        current?.finish()
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

private final class BlockingRecoveryEventStore: ActivityEventStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var started = false
    private var appendContinuation: CheckedContinuation<Void, Never>?

    var appendStarted: Bool { lock.withLock { started } }

    func append(_ events: [DiskActivityEvent]) async throws {
        await withCheckedContinuation { continuation in
            lock.withLock {
                started = true
                appendContinuation = continuation
            }
        }
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] { [] }
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws {}

    func release() {
        let continuation = lock.withLock {
            let current = appendContinuation
            appendContinuation = nil
            return current
        }
        continuation?.resume()
    }
}

private final class FailOnceRecoveryEventStore: ActivityEventStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var calls = 0
    private var persisted = 0

    var appendCallCount: Int { lock.withLock { calls } }
    var persistedEventCount: Int { lock.withLock { persisted } }

    func append(_ events: [DiskActivityEvent]) async throws {
        let call = lock.withLock {
            calls += 1
            return calls
        }
        if call == 1 { throw RecoveryStoreError.injectedFailure }
        lock.withLock { persisted += events.count }
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] { [] }
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws {}
}

private enum RecoveryStoreError: Error { case injectedFailure }

private final class UnknownCommitRecoveryEventStore: ActivityEventStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var calls = 0

    var appendCallCount: Int { lock.withLock { calls } }

    func append(_ events: [DiskActivityEvent]) async throws {
        lock.withLock { calls += 1 }
        throw ActivityEventStoreError.commitStateUnknown
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] { [] }
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws {}
}

private final class AlwaysFailRecoveryEventStore: ActivityEventStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var calls = 0

    var appendCallCount: Int { lock.withLock { calls } }

    func append(_ events: [DiskActivityEvent]) async throws {
        lock.withLock { calls += 1 }
        throw RecoveryStoreError.injectedFailure
    }

    func loadEvents(rootPath: URL, limit: Int) async throws -> [DiskActivityEvent] { [] }
    func enforceStoragePolicy(
        _ preferences: ActivityStoragePreferences,
        eventJournalLimitBytes: Int64,
        now: Date
    ) async throws {}
}

private final class RecoveryCallCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var calls = 0

    var count: Int { lock.withLock { calls } }

    @discardableResult
    func increment() -> Int {
        lock.withLock {
            calls += 1
            return calls
        }
    }
}
