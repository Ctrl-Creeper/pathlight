//
//  AppModel.swift
//  Pathlight
//

import Combine
import Foundation

/// Central state for folder monitoring: live (short-term) watches, persisted
/// long-term watches, activity history, storage policy, and launch-at-login.
@MainActor
final class AppModel: ObservableObject {
    @Published var activityDetailedRetentionDays = ActivityStoragePreferences.defaults.detailedRetentionDays
    @Published var activityAggregateRetentionDays = ActivityStoragePreferences.defaults.aggregateRetentionDays
    @Published var activityStorageLimitBytes = ActivityStoragePreferences.defaults.storageLimitBytes
    @Published var activityEncryptNewData = ActivityStoragePreferences.defaults.encryptNewData
    @Published var lastErrorMessage: String?
    /// Recording health. Shown as a banner beside the data it affects, because
    /// a modal alert interrupts without offering anything to act on.
    @Published var monitoringStatusMessage: String?
    @Published private(set) var fullDiskAccessStatus: FullDiskAccessStatus = .unknown
    @Published private(set) var liveWatchSession: WatchSessionModel?
    @Published private(set) var activityHistory: ActivityHistorySnapshot?
    @Published private(set) var activityDashboardHistories: [String: ActivityHistorySnapshot] = [:]
    @Published private(set) var activityStorageUsage = ActivityStorageUsageSnapshot.empty
    @Published private(set) var longTermWatchTargets: [LongTermWatchTarget] = []
    @Published private(set) var longTermWatchRuntimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus] = [:]
    @Published private(set) var launchAtLoginStatus: LaunchAtLoginStatus = .disabled
    @Published private(set) var launchAtLoginNudgeDismissed =
        UserDefaults.standard.bool(forKey: AppModel.launchAtLoginNudgeDismissedKey)

    private static let launchAtLoginNudgeDismissedKey = "launchAtLoginNudgeDismissed"
    private static let preferencePersistenceDebounce: RunLoop.SchedulerTimeType.Stride = .milliseconds(50)

    private let dependencies: AppDependencies
    private var lastPersistedActivityStoragePreferences: ActivityStoragePreferences?
    private var cancellables = Set<AnyCancellable>()
    private var fullDiskAccessRefreshTask: Task<Void, Never>?
    private var liveWatchTask: Task<Void, Never>?
    private var liveWatchTaskID: UUID?
    private var liveWatchBaselineTask: Task<Void, Never>?
    private var activityHistoryTask: Task<Void, Never>?
    private var activityHistoryTaskID: UUID?
    private var lastEventDrivenHistoryRefreshAt: [String: Date] = [:]
    private var pendingEventDrivenHistoryRefreshTasks: [String: Task<Void, Never>] = [:]
    private var growthAlertLastPostedAt: [String: Date] = [:]
    private var anomalyAlertLastPostedAt: [String: Date] = [:]
    private static let anomalyAlertInterval: TimeInterval = 60 * 60
    private var activityDashboardHistoryTask: Task<Void, Never>?
    private var activityDashboardHistoryTaskID: UUID?
    private var activityStorageUsageTask: Task<Void, Never>?
    private var activityStorageUsageTaskID: UUID?
    private var activityStoragePolicyTask: Task<Void, Never>?
    private var activityStoragePolicyTaskID: UUID?
    private var longTermWatchBaselineTasks: [LongTermWatchTarget.ID: Task<Void, Never>] = [:]
    private var longTermWatchBaselineIDs: [LongTermWatchTarget.ID: UUID] = [:]
    private var longTermWatchTasks: [LongTermWatchTarget.ID: Task<Void, Never>] = [:]
    private var longTermWatchTaskIDs: [LongTermWatchTarget.ID: UUID] = [:]
    private var pendingJournalEvents: [UUID: DiskActivityEvent] = [:]
    private var pendingJournalOrder: [UUID] = []
    private var pendingJournalRoots: Set<String> = []
    private var pendingJournalCheckpoints: [String: LongTermWatchCheckpoint] = [:]
    private var committedJournalEvents: [UUID: DiskActivityEvent] = [:]
    private var pendingJournalSince: Date?
    private var journalFlushTask: Task<Void, Never>?
    private var journalFlushInProgress = false
    private var journalFlushInFlightRoots: Set<String> = []
    private var journalFlushImmediatelyRequested = false
    private var journalStorageBlocked = false
    private var journalFlushFailureCount = 0
    /// Rows wait until the live-monitor coalescing window (1 s) has closed so a
    /// created+modified burst lands in the journal as one final row.
    private static let journalFlushDelay: TimeInterval = 1.2
    /// A never-quiet file still gets persisted at least this often.
    private static let journalFlushMaxWait: TimeInterval = 5
    /// Session coalescing is one second. Retaining committed versions slightly
    /// longer lets a max-wait flush turn a later merged version into a delta.
    private static let committedJournalEventRetention: TimeInterval = 2
    /// One or two failed flushes are normal transients; past that the store is
    /// really unavailable and the user deserves to hear about it.
    private static let journalFlushFailureReportThreshold = 3

    /// No default: a live `AppDependencies` needs an attributor, and only a
    /// host that links the Rust core has one.
    init(dependencies: AppDependencies) {
        self.dependencies = dependencies

        let activityStoragePreferences = dependencies.activityStoragePreferences.loadPreferences()
        activityDetailedRetentionDays = activityStoragePreferences.detailedRetentionDays
        activityAggregateRetentionDays = activityStoragePreferences.aggregateRetentionDays
        activityStorageLimitBytes = activityStoragePreferences.storageLimitBytes
        activityEncryptNewData = activityStoragePreferences.encryptNewData
        lastPersistedActivityStoragePreferences = activityStoragePreferences
        longTermWatchTargets = dependencies.longTermWatchTargets.loadTargets()
        launchAtLoginStatus = dependencies.launchAtLoginService.currentStatus()

        warmActivityStorageKey()
        observeActivityStoragePreferences()
        refreshFullDiskAccessStatus()
        startEnabledLongTermWatches()
        // Preload histories so the menu bar shows today's numbers before the
        // dashboard is ever opened.
        refreshActivityDashboardHistories(rootPaths: longTermWatchTargets.map(\.rootPath))
        refreshActivityStorageUsage()
        applyActivityStoragePolicy()
    }

    deinit {
        MainActor.assumeIsolated {
            cleanup()
        }
    }

    func cleanup() {
        persistActivityStoragePreferences(currentActivityStoragePreferences)
        flushJournalNow()
        fullDiskAccessRefreshTask?.cancel()
        fullDiskAccessRefreshTask = nil
        stopShortTermWatch()
        cancelActivityHistoryRefresh(clearHistory: true)
        cancelActivityDashboardHistoryRefresh(clearHistories: true)
        cancelActivityStorageUsageRefresh()
        cancelActivityStoragePolicy()
        cancelLongTermWatchBaseline()
        stopAllLongTermWatches()
    }

    // MARK: - Folder selection

    func addLongTermWatchFromOpenPanel() {
        guard let url = dependencies.systemActions.presentFolderPanel(
            "Monitor",
            "Choose a folder to monitor for long-term changes."
        ) else {
            return
        }
        enableLongTermWatch(rootPath: url)
    }

    /// Returns true when a watch started, so callers can open the Live Monitor window.
    @discardableResult
    func startShortTermWatchFromOpenPanel(
        options: DiskActivityAggregationOptions = .shortTermDefault
    ) -> Bool {
        guard let url = dependencies.systemActions.presentFolderPanel(
            "Watch",
            "Choose a folder to watch for live changes."
        ) else {
            return false
        }
        startShortTermWatch(rootPath: url, options: options)
        return true
    }

    func revealURLInFinder(_ url: URL) {
        dependencies.systemActions.reveal(url)
    }

    func addPreset(_ preset: MonitoringPreset) {
        guard !isLongTermWatchTarget(preset.rootPath) else {
            return
        }
        enableLongTermWatch(rootPath: preset.rootPath, options: preset.options)
    }

    // MARK: - Export

    func exportLiveSessionReport() {
        guard let session = liveWatchSession else {
            return
        }
        let name = session.rootPath.lastPathComponent.isEmpty ? "root" : session.rootPath.lastPathComponent
        guard let destination = dependencies.systemActions.presentSavePanel(
            "Pathlight Session \(name) \(Self.fileNameDate(session.startedAt)).md"
        ) else {
            return
        }
        write(ActivityExportService.sessionReport(session), to: destination)
    }

    func exportHistoryCSV(rootPath: URL) {
        guard let activityEventStore = dependencies.activityEventStore else {
            return
        }
        let name = rootPath.lastPathComponent.isEmpty ? "root" : rootPath.lastPathComponent
        guard let destination = dependencies.systemActions.presentSavePanel(
            "Pathlight History \(name) \(Self.fileNameDate(Date())).csv"
        ) else {
            return
        }
        Task { @MainActor [weak self] in
            do {
                let events = try await activityEventStore.loadEvents(rootPath: rootPath, limit: 200_000)
                self?.write(ActivityExportService.csv(events), to: destination)
            } catch {
                self?.lastErrorMessage = "Pathlight could not read the activity history."
            }
        }
    }

    private func write(_ text: String, to destination: URL) {
        do {
            try Data(text.utf8).write(to: destination, options: .atomic)
            dependencies.systemActions.reveal(destination)
        } catch {
            lastErrorMessage = "Pathlight could not write \(destination.lastPathComponent)."
        }
    }

    private static func fileNameDate(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HHmm"
        return formatter.string(from: date)
    }

    // MARK: - Full Disk Access

    func refreshFullDiskAccessStatus() {
        fullDiskAccessRefreshTask?.cancel()
        let probe = dependencies.systemActions.fullDiskAccessStatus
        fullDiskAccessRefreshTask = Task { @MainActor [weak self] in
            let status = await probe()
            guard !Task.isCancelled else { return }
            self?.fullDiskAccessStatus = status
        }
    }

    func prepareAndOpenFullDiskAccessSettings() {
        if !dependencies.systemActions.prepareAndOpenFullDiskAccessSettings() {
            lastErrorMessage = "Pathlight could not open Full Disk Access settings."
        }
    }

    // MARK: - Activity storage

    func restoreDefaultActivityStoragePreferences() {
        activityDetailedRetentionDays = ActivityStoragePreferences.defaults.detailedRetentionDays
        activityAggregateRetentionDays = ActivityStoragePreferences.defaults.aggregateRetentionDays
        activityStorageLimitBytes = ActivityStoragePreferences.defaults.storageLimitBytes
        activityEncryptNewData = ActivityStoragePreferences.defaults.encryptNewData
    }

    func refreshActivityStorageUsage() {
        cancelActivityStorageUsageRefresh()

        let taskID = UUID()
        activityStorageUsageTaskID = taskID
        let service = dependencies.activityStorageUsageService
        activityStorageUsageTask = Task { @MainActor [weak self] in
            guard let self else { return }
            let usage = await service.loadUsage()
            guard !Task.isCancelled, self.activityStorageUsageTaskID == taskID else {
                return
            }
            self.activityStorageUsage = usage
            self.activityStorageUsageTask = nil
            self.activityStorageUsageTaskID = nil
        }
    }

    func compactActivityStorage() {
        applyActivityStoragePolicy()
    }

    func resetActivityStorage() {
        let service = dependencies.activityStorageUsageService
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await service.resetStorage()
                self.activityStorageUsage = .empty
            } catch {
                self.lastErrorMessage = "Pathlight could not reset activity storage."
            }
        }
    }

    /// Uninstall is a one-way door: stop recording first so nothing writes
    /// after its files are gone, then remove what the user chose. A successful
    /// uninstall never returns — the app is quit from under us.
    func uninstall(scope: UninstallScope) async {
        cleanup()
        if scope == .appOnly {
            // The history is being kept, so it has to be all of it. `cleanup()`
            // only schedules the last flush, and the `exit(0)` inside the
            // uninstall never lets that task run.
            // ponytail: a flush already in flight cannot be awaited from here.
            await flushJournal()
        }
        let outcome = dependencies.systemActions.uninstall(scope)
        let failures: [String]
        switch outcome {
        case let .installIntact(reported):
            // Nothing was removed, so the user still has a Pathlight that is
            // meant to be recording. Leaving it stopped would be a silent
            // second failure.
            failures = reported
            startEnabledLongTermWatches()
        case let .removed(reported):
            failures = reported
        }
        guard !failures.isEmpty else { return }
        lastErrorMessage = "Pathlight could not remove \(failures.joined(separator: ", ")). Remove it by hand, or try again."
    }

    func startShortTermWatch(
        rootPath: URL,
        options: DiskActivityAggregationOptions = .shortTermDefault
    ) {
        stopShortTermWatch()

        let taskID = UUID()
        liveWatchTaskID = taskID
        let coordinator = LiveWatchSessionCoordinator(
            monitor: dependencies.activityMonitor,
            attribution: dependencies.activityAttribution,
            processHints: dependencies.processHints
        )
        let sizeProviders = dependencies.activitySizeProviders(
            ActivitySizeProviders.scope(kind: "live", rootPath: rootPath)
        )
        let baselineService = dependencies.activityBaselineService

        liveWatchTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.liveWatchTaskID == taskID {
                    self.liveWatchTask = nil
                    self.liveWatchTaskID = nil
                }
            }

            let stream = coordinator.sessions(
                rootPath: rootPath,
                options: options,
                sizeProviders: sizeProviders
            )
            for await session in stream {
                guard !Task.isCancelled, self.liveWatchTaskID == taskID else {
                    break
                }
                if session.receivedStreamEventCount > 0, self.liveWatchBaselineTask == nil {
                    self.liveWatchBaselineTask = Task.detached {
                        _ = await baselineService.captureBaseline(rootPath: rootPath)
                    }
                }
                self.liveWatchSession = session
                self.enqueueForJournal(session.latestChanges, rootPath: session.rootPath)
            }
        }
    }

    func stopShortTermWatch() {
        flushJournalNow()
        liveWatchTask?.cancel()
        liveWatchTask = nil
        liveWatchTaskID = nil
        liveWatchBaselineTask?.cancel()
        liveWatchBaselineTask = nil
        liveWatchSession = nil
    }

    // MARK: - Journal persistence

    /// Buffers changed events by their session identity and flushes once the
    /// coalescing window has closed, so merged bursts are written once, as the
    /// final merged row, and never re-written after a later merge.
    private func enqueueForJournal(_ changes: [WatchSessionModel.ChangedEvent], rootPath: URL) {
        guard dependencies.activityEventStore != nil,
              !journalStorageBlocked,
              !changes.isEmpty else {
            return
        }
        let now = Date()
        for change in changes {
            if pendingJournalEvents[change.id] == nil {
                pendingJournalOrder.append(change.id)
            }
            pendingJournalEvents[change.id] = change.event
        }
        pendingJournalRoots.insert(rootPath.standardizedFileURL.path)
        let since = pendingJournalSince ?? now
        pendingJournalSince = since
        let remainingMaxWait = max(0, Self.journalFlushMaxWait - now.timeIntervalSince(since))
        let delay = min(Self.journalFlushDelay, remainingMaxWait)

        scheduleJournalFlush(after: delay)
    }

    private func scheduleJournalFlush(after delay: TimeInterval) {
        guard !journalFlushInProgress else {
            if delay == 0 { journalFlushImmediatelyRequested = true }
            return
        }
        journalFlushTask?.cancel()
        journalFlushTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(Int(delay * 1_000)))
            guard let self, !Task.isCancelled else { return }
            self.journalFlushTask = nil
            await self.flushJournal()
        }
    }

    private struct PendingJournalBatch {
        let entries: [(id: UUID, event: DiskActivityEvent)]
        let roots: Set<String>
        let checkpoints: [String: LongTermWatchCheckpoint]
        let since: Date?
    }

    private func takePendingJournalBatch() -> PendingJournalBatch {
        let entries = pendingJournalOrder.compactMap { id in
            pendingJournalEvents[id].map { (id, $0) }
        }
        let roots = pendingJournalRoots
        let checkpoints = Dictionary(uniqueKeysWithValues: roots.compactMap { root in
            pendingJournalCheckpoints[root].map { (root, $0) }
        })
        let since = pendingJournalSince
        pendingJournalEvents.removeAll()
        pendingJournalOrder.removeAll()
        pendingJournalRoots.removeAll()
        pendingJournalSince = nil
        return PendingJournalBatch(
            entries: entries,
            roots: roots,
            checkpoints: checkpoints,
            since: since
        )
    }

    private func flushJournal() async {
        guard let store = dependencies.activityEventStore, !journalFlushInProgress else { return }
        let batch = takePendingJournalBatch()
        guard !batch.entries.isEmpty else {
            advanceUnblockedCheckpoints()
            return
        }
        journalFlushInProgress = true
        journalFlushInFlightRoots.formUnion(batch.roots)
        do {
            let journalEvents = batch.entries.compactMap { entry in
                Self.incrementalJournalEvent(
                    entry.event,
                    after: committedJournalEvents[entry.id]
                )
            }
            try await store.append(journalEvents)
            journalFlushFailureCount = 0
            monitoringStatusMessage = nil
            journalFlushInProgress = false
            journalFlushInFlightRoots.subtract(batch.roots)
            for entry in batch.entries {
                committedJournalEvents[entry.id] = entry.event
            }
            let retentionCutoff = Date().addingTimeInterval(-Self.committedJournalEventRetention)
            committedJournalEvents = committedJournalEvents.filter {
                $0.value.timestamp >= retentionCutoff || pendingJournalEvents[$0.key] != nil
            }
            for (root, checkpoint) in batch.checkpoints {
                persistCheckpoint(checkpoint, rootPath: root)
            }
            advanceUnblockedCheckpoints()
            applyActivityStoragePolicy()
            for root in batch.roots {
                scheduleEventDrivenHistoryRefresh(rootPath: URL(filePath: root, directoryHint: .isDirectory))
            }
        } catch ActivityEventStoreError.commitStateUnknown {
            journalFlushInProgress = false
            journalFlushInFlightRoots.subtract(batch.roots)
            journalStorageBlocked = true
            pendingJournalEvents.removeAll()
            pendingJournalOrder.removeAll()
            pendingJournalRoots.removeAll()
            pendingJournalSince = nil
            monitoringStatusMessage = "Activity history storage became inconsistent. Monitoring continues, but recording is paused until Pathlight restarts."
        } catch {
            journalFlushInProgress = false
            journalFlushInFlightRoots.subtract(batch.roots)
            restorePendingJournalBatch(batch)
            journalFlushFailureCount += 1
            if journalFlushFailureCount >= Self.journalFlushFailureReportThreshold {
                monitoringStatusMessage = "Pathlight could not write activity history. Monitoring continues and recording keeps retrying."
            }
        }
        if !pendingJournalEvents.isEmpty {
            let delay: TimeInterval = journalFlushImmediatelyRequested
                ? 0
                : Self.journalRetryDelay(failureCount: journalFlushFailureCount)
            journalFlushImmediatelyRequested = false
            scheduleJournalFlush(after: delay)
        } else {
            journalFlushImmediatelyRequested = false
        }
    }

    private func restorePendingJournalBatch(_ batch: PendingJournalBatch) {
        let restoredIDs = Set(batch.entries.map(\.id))
        pendingJournalOrder = batch.entries.map(\.id)
            + pendingJournalOrder.filter { !restoredIDs.contains($0) }
        for entry in batch.entries where pendingJournalEvents[entry.id] == nil {
            pendingJournalEvents[entry.id] = entry.event
        }
        pendingJournalRoots.formUnion(batch.roots)
        if let since = batch.since {
            pendingJournalSince = min(pendingJournalSince ?? since, since)
        }
    }

    /// `WatchSessionModel` publishes the full merged event on every update. If
    /// a five-second max-wait flush committed an earlier version, persist only
    /// the newly observed increment instead of counting the cumulative value
    /// again. IDs never intentionally cross paths or kinds; a mismatch is kept
    /// as independent evidence rather than guessed into a delta.
    nonisolated static func incrementalJournalEvent(
        _ current: DiskActivityEvent,
        after committed: DiskActivityEvent?
    ) -> DiskActivityEvent? {
        guard let committed else { return current }
        guard current != committed else { return nil }
        guard current.rootPath.standardizedFileURL == committed.rootPath.standardizedFileURL,
              current.path.standardizedFileURL == committed.path.standardizedFileURL,
              current.kind == committed.kind,
              current.timestamp >= committed.timestamp else {
            return current
        }

        let byteDelta: Int64?
        if let currentDelta = current.byteDelta, let committedDelta = committed.byteDelta {
            let difference = currentDelta.subtractingReportingOverflow(committedDelta)
            byteDelta = difference.overflow ? nil : difference.partialValue
        } else {
            byteDelta = nil
        }
        let confidence: DiskActivityEventConfidence
        if byteDelta == nil {
            confidence = .unknown
        } else if current.confidence == .confirmed, committed.confidence == .confirmed {
            confidence = .confirmed
        } else {
            confidence = .estimated
        }
        return DiskActivityEvent(
            kind: .modified,
            path: current.path,
            rootPath: current.rootPath,
            timestamp: current.timestamp,
            byteDelta: byteDelta,
            confidence: confidence,
            previousPath: nil,
            affectedItemCount: current.affectedItemCount,
            processName: current.processName
        )
    }

    /// Stop paths request the same serialized writer used by scheduled flushes.
    /// If the model is deallocated first, its checkpoint remains behind and the
    /// native journal replays the uncommitted interval on the next launch.
    /// The journal key is otherwise first touched by a background write minutes
    /// after launch, where a keychain prompt can sit unnoticed and hold up
    /// recording. Ask for it now, while the user is still looking at the app.
    private func warmActivityStorageKey() {
        let warmUp = dependencies.activityStorageKeyWarmUp
        Task.detached(priority: .utility) { [weak self] in
            for _ in 1...Self.activityStorageKeyWarmUpAttempts {
                do {
                    try warmUp()
                    return
                } catch ActivityStorageLineCodecError.keyUnavailable {
                    // Still waiting on the prompt; keep waiting without asking again.
                    continue
                } catch {
                    break
                }
            }
            await self?.reportActivityStorageKeyUnavailable()
        }
    }
    /// Each attempt waits out one key-load timeout, so this is how long a slow
    /// answer to the keychain prompt stays quiet.
    nonisolated private static let activityStorageKeyWarmUpAttempts = 3
    private func reportActivityStorageKeyUnavailable() {
        monitoringStatusMessage = "Pathlight needs keychain access to record activity history. Grant it and recording continues."
    }
    /// Doubles per consecutive failure so a stalled store is retried without
    /// spinning, and stays at one second while flushes are succeeding.
    nonisolated static func journalRetryDelay(failureCount: Int) -> TimeInterval {
        guard failureCount > 0 else { return 1 }
        return min(30, pow(2, Double(min(failureCount, 5))))
    }
    private func flushJournalNow() {
        journalFlushTask?.cancel()
        journalFlushTask = nil
        guard !pendingJournalEvents.isEmpty else {
            advanceUnblockedCheckpoints()
            return
        }
        scheduleJournalFlush(after: 0)
    }

    private func stageCheckpoint(_ checkpoint: LongTermWatchCheckpoint, rootPath: URL) {
        let root = rootPath.standardizedFileURL.path
        guard dependencies.activityEventStore == nil || !journalStorageBlocked else { return }
        let persisted = longTermWatchTargets.first(where: { $0.id == root })?.checkpoint
        if let existing = pendingJournalCheckpoints[root] ?? persisted {
            guard checkpoint.eventID >= existing.eventID || checkpoint.hasHistoryGap else { return }
            pendingJournalCheckpoints[root] = LongTermWatchCheckpoint(
                eventID: max(existing.eventID, checkpoint.eventID),
                recordedAt: max(existing.recordedAt, checkpoint.recordedAt),
                hasHistoryGap: existing.hasHistoryGap || checkpoint.hasHistoryGap
            )
        } else {
            pendingJournalCheckpoints[root] = checkpoint
        }
        advanceUnblockedCheckpoints()
    }

    private func advanceUnblockedCheckpoints() {
        guard dependencies.activityEventStore == nil || !journalStorageBlocked else { return }
        let ready = pendingJournalCheckpoints.filter {
            !pendingJournalRoots.contains($0.key) && !journalFlushInFlightRoots.contains($0.key)
        }
        for (root, checkpoint) in ready
        {
            persistCheckpoint(checkpoint, rootPath: root)
        }
    }

    private func persistCheckpoint(_ checkpoint: LongTermWatchCheckpoint, rootPath: String) {
        longTermWatchTargets = dependencies.longTermWatchTargets.updateCheckpoint(
            checkpoint,
            forRootPath: URL(filePath: rootPath, directoryHint: .isDirectory),
            currentTargets: longTermWatchTargets
        )
        if pendingJournalCheckpoints[rootPath] == checkpoint {
            pendingJournalCheckpoints[rootPath] = nil
        }
    }

    // Reloading history reads the whole journal, so event-driven refreshes are
    // throttled per root: the first fires immediately, later ones coalesce into
    // one trailing refresh so sustained churn costs at most one reload per second.
    private func scheduleEventDrivenHistoryRefresh(rootPath: URL) {
        let throttleInterval: TimeInterval = 1
        let key = rootPath.standardizedFileURL.path
        let now = Date()
        if let lastRefreshAt = lastEventDrivenHistoryRefreshAt[key],
           now.timeIntervalSince(lastRefreshAt) < throttleInterval {
            guard pendingEventDrivenHistoryRefreshTasks[key] == nil else {
                return
            }
            let delay = throttleInterval - now.timeIntervalSince(lastRefreshAt)
            pendingEventDrivenHistoryRefreshTasks[key] = Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(delay))
                guard let self, !Task.isCancelled else { return }
                self.pendingEventDrivenHistoryRefreshTasks[key] = nil
                self.lastEventDrivenHistoryRefreshAt[key] = Date()
                self.refreshActivityHistory(rootPath: rootPath)
            }
            return
        }
        lastEventDrivenHistoryRefreshAt[key] = now
        refreshActivityHistory(rootPath: rootPath)
    }

    func refreshActivityHistory(
        rootPath: URL,
        bucketInterval: TimeInterval = 3_600,
        eventLimit: Int = 500
    ) {
        cancelActivityHistoryRefresh(clearHistory: false)

        guard let activityEventStore = dependencies.activityEventStore else {
            activityHistory = nil
            activityDashboardHistories.removeValue(forKey: rootPath.standardizedFileURL.path)
            return
        }

        let taskID = UUID()
        activityHistoryTaskID = taskID
        let service = ActivityHistoryService(store: activityEventStore)

        activityHistoryTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.activityHistoryTaskID == taskID {
                    self.activityHistoryTask = nil
                    self.activityHistoryTaskID = nil
                }
            }

            do {
                let history = try await service.loadHistory(
                    rootPath: rootPath,
                    eventLimit: eventLimit,
                    bucketInterval: bucketInterval
                )
                guard !Task.isCancelled, self.activityHistoryTaskID == taskID else {
                    return
                }
                self.activityHistory = history
                self.activityDashboardHistories[history.rootPath.standardizedFileURL.path] = history
                self.evaluateGrowthAlert(history: history)
            } catch {
                guard !Task.isCancelled, self.activityHistoryTaskID == taskID else {
                    return
                }
                self.activityHistory = nil
                self.activityDashboardHistories.removeValue(forKey: rootPath.standardizedFileURL.path)
            }
        }
    }

    func refreshActivityDashboardHistories(
        rootPaths: [URL],
        bucketInterval: TimeInterval = 3_600,
        eventLimit: Int = 500
    ) {
        cancelActivityDashboardHistoryRefresh(clearHistories: false)

        let standardizedRoots = rootPaths
            .map(\.standardizedFileURL)
            .reduce(into: [String: URL]()) { rootsByID, root in
                rootsByID[root.path] = root
            }
            .values
            .sorted { $0.path < $1.path }

        guard !standardizedRoots.isEmpty else { return }

        guard let activityEventStore = dependencies.activityEventStore else {
            for root in standardizedRoots {
                activityDashboardHistories.removeValue(forKey: root.path)
            }
            return
        }

        let taskID = UUID()
        activityDashboardHistoryTaskID = taskID
        let service = ActivityHistoryService(store: activityEventStore)

        activityDashboardHistoryTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.activityDashboardHistoryTaskID == taskID {
                    self.activityDashboardHistoryTask = nil
                    self.activityDashboardHistoryTaskID = nil
                }
            }

            for root in standardizedRoots {
                guard !Task.isCancelled, self.activityDashboardHistoryTaskID == taskID else {
                    return
                }

                do {
                    let history = try await service.loadHistory(
                        rootPath: root,
                        eventLimit: eventLimit,
                        bucketInterval: bucketInterval
                    )
                    guard !Task.isCancelled, self.activityDashboardHistoryTaskID == taskID else {
                        return
                    }
                    self.activityDashboardHistories[history.rootPath.standardizedFileURL.path] = history
                    self.evaluateGrowthAlert(history: history)
                } catch {
                    guard !Task.isCancelled, self.activityDashboardHistoryTaskID == taskID else {
                        return
                    }
                    self.activityDashboardHistories.removeValue(forKey: root.path)
                }
            }
        }
    }

    private func cancelActivityHistoryRefresh(clearHistory: Bool) {
        activityHistoryTask?.cancel()
        activityHistoryTask = nil
        activityHistoryTaskID = nil
        if clearHistory {
            activityHistory = nil
            activityDashboardHistories.removeAll()
        }
    }

    private func cancelActivityDashboardHistoryRefresh(clearHistories: Bool) {
        activityDashboardHistoryTask?.cancel()
        activityDashboardHistoryTask = nil
        activityDashboardHistoryTaskID = nil
        if clearHistories {
            activityDashboardHistories.removeAll()
        }
    }

    private func cancelActivityStorageUsageRefresh() {
        activityStorageUsageTask?.cancel()
        activityStorageUsageTask = nil
        activityStorageUsageTaskID = nil
    }

    func enableLongTermWatch(
        rootPath: URL,
        options: LongTermWatchTargetOptions = .default
    ) {
        let target = LongTermWatchTarget(rootPath: rootPath, isEnabled: true, options: options)
        longTermWatchTargets = dependencies.longTermWatchTargets.upsert(target, currentTargets: longTermWatchTargets)
        startLongTermWatch(for: target)
    }

    func setLongTermWatchEnabled(_ isEnabled: Bool, rootPath: URL) {
        longTermWatchTargets = dependencies.longTermWatchTargets.setEnabled(
            isEnabled,
            forRootPath: rootPath,
            currentTargets: longTermWatchTargets
        )
        if let target = longTermWatchTargets.first(where: { $0.rootPath == rootPath.standardizedFileURL }) {
            if isEnabled {
                startLongTermWatch(for: target)
            } else {
                stopLongTermWatch(targetID: target.id)
            }
        }
    }

    func removeLongTermWatchTarget(rootPath: URL) {
        let targetID = rootPath.standardizedFileURL.path
        stopLongTermWatch(targetID: targetID)
        longTermWatchRuntimeStatuses.removeValue(forKey: targetID)
        longTermWatchTargets = dependencies.longTermWatchTargets.remove(
            rootPath: rootPath,
            currentTargets: longTermWatchTargets
        )
    }

    func setGrowthAlertThreshold(_ thresholdBytes: Int64?, rootPath: URL) {
        let targetID = rootPath.standardizedFileURL.path
        guard let target = longTermWatchTargets.first(where: { $0.id == targetID }) else {
            return
        }

        var options = target.options
        options.growthAlertThresholdBytes = thresholdBytes
        longTermWatchTargets = dependencies.longTermWatchTargets.updateOptions(
            options,
            forRootPath: target.rootPath,
            currentTargets: longTermWatchTargets
        )
    }

    /// Adds the advisor's pattern to the tracked target's exclusions, if the
    /// live-monitored folder is also a long-term watch.
    func excludeNoise(_ suggestion: ActivityNoiseSuggestion, rootPath: URL) {
        let targetID = rootPath.standardizedFileURL.path
        guard let target = longTermWatchTargets.first(where: { $0.id == targetID }) else {
            return
        }
        setExclusionPatterns(target.options.exclusionPatterns + [suggestion.pattern], rootPath: rootPath)
    }

    func isLongTermWatchTarget(_ rootPath: URL) -> Bool {
        longTermWatchTargets.contains { $0.id == rootPath.standardizedFileURL.path }
    }

    func setExclusionPatterns(_ patterns: [String], rootPath: URL) {
        let targetID = rootPath.standardizedFileURL.path
        guard let target = longTermWatchTargets.first(where: { $0.id == targetID }) else {
            return
        }

        var options = target.options
        let normalizedPatterns = ActivityExclusionPatterns.normalized(patterns)
        guard normalizedPatterns != options.exclusionPatterns else {
            return
        }

        options.exclusionPatterns = normalizedPatterns
        longTermWatchTargets = dependencies.longTermWatchTargets.updateOptions(
            options,
            forRootPath: target.rootPath,
            currentTargets: longTermWatchTargets
        )
        // Restart the watch so the running stream picks up the new filter now
        // instead of on the next reconnect.
        if let updatedTarget = longTermWatchTargets.first(where: { $0.id == targetID }),
           updatedTarget.isEnabled {
            startLongTermWatch(for: updatedTarget)
        }
    }

    private func evaluateGrowthAlert(history: ActivityHistorySnapshot) {
        guard let poster = dependencies.activityGrowthAlertPoster else {
            return
        }
        let targetID = history.rootPath.standardizedFileURL.path
        evaluateAnomalies(history: history, targetID: targetID, poster: poster)
        guard let target = longTermWatchTargets.first(where: { $0.id == targetID }),
              let growth = ActivityGrowthAlertEvaluator.alertworthyGrowth(
                  target: target,
                  history: history,
                  lastAlertedAt: growthAlertLastPostedAt[targetID]
              ),
              let threshold = target.options.growthAlertThresholdBytes else {
            return
        }

        growthAlertLastPostedAt[targetID] = Date()
        Task {
            await poster.postGrowthAlert(
                rootPath: target.rootPath,
                growthBytes: growth,
                thresholdBytes: threshold
            )
        }
    }

    private func evaluateAnomalies(
        history: ActivityHistorySnapshot,
        targetID: String,
        poster: any ActivityGrowthAlertPosting
    ) {
        guard longTermWatchTargets.contains(where: { $0.id == targetID && $0.isEnabled }) else {
            return
        }
        let now = Date()
        for anomaly in ActivityAnomalyDetector.anomalies(in: history, now: now) {
            let key = anomaly.identifier
            if let last = anomalyAlertLastPostedAt[key], now.timeIntervalSince(last) < Self.anomalyAlertInterval {
                continue
            }
            anomalyAlertLastPostedAt[key] = now
            Task {
                await poster.postActivityAlert(
                    rootPath: anomaly.rootPath,
                    identifier: key,
                    title: anomaly.title,
                    body: anomaly.body
                )
            }
        }
    }

    func clearLongTermWatchHistoryGap(rootPath: URL) {
        let targetID = rootPath.standardizedFileURL.path
        guard let target = longTermWatchTargets.first(where: { $0.id == targetID }),
              let checkpoint = target.checkpoint,
              checkpoint.hasHistoryGap else {
            return
        }

        longTermWatchTargets = dependencies.longTermWatchTargets.updateCheckpoint(
            LongTermWatchCheckpoint(
                eventID: checkpoint.eventID,
                recordedAt: checkpoint.recordedAt,
                hasHistoryGap: false
            ),
            forRootPath: target.rootPath,
            currentTargets: longTermWatchTargets
        )
        if let status = longTermWatchRuntimeStatuses[targetID], status.state == .historyGap {
            updateLongTermWatchRuntimeStatus(
                targetID: targetID,
                state: .watching,
                retryCount: status.retryCount
            )
        }
    }

    private func cancelLongTermWatchBaseline() {
        for task in longTermWatchBaselineTasks.values { task.cancel() }
        longTermWatchBaselineTasks.removeAll()
        longTermWatchBaselineIDs.removeAll()
    }

    private func startEnabledLongTermWatches() {
        for target in longTermWatchTargets where target.isEnabled {
            startLongTermWatch(for: target)
        }
    }

    private func startLongTermWatch(for target: LongTermWatchTarget) {
        guard target.isEnabled else {
            stopLongTermWatch(targetID: target.id)
            return
        }

        stopLongTermWatch(targetID: target.id)

        let taskID = UUID()
        longTermWatchTaskIDs[target.id] = taskID
        updateLongTermWatchRuntimeStatus(
            targetID: target.id,
            state: Self.rootExists(target.rootPath) ? .starting : .rootMissing,
            retryCount: 0
        )
        let coordinator = LiveWatchSessionCoordinator(
            monitor: dependencies.activityMonitor,
            attribution: dependencies.activityAttribution,
            processHints: dependencies.processHints
        )
        let sizeProviders = dependencies.activitySizeProviders(
            ActivitySizeProviders.scope(kind: "long-term", rootPath: target.rootPath)
        )
        longTermWatchTasks[target.id] = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.longTermWatchTaskIDs[target.id] == taskID {
                    self.longTermWatchTasks[target.id] = nil
                    self.longTermWatchTaskIDs[target.id] = nil
                }
            }

            var retryCount = 0
            while !Task.isCancelled, self.longTermWatchTaskIDs[target.id] == taskID {
                let currentTarget = self.longTermWatchTargets.first(where: { $0.id == target.id }) ?? target
                let rootKey = currentTarget.rootPath.standardizedFileURL.path
                // Within this process, resume after the latest accepted cursor,
                // including a batch still waiting for durable journal commit.
                // After a crash only the persisted checkpoint survives, causing
                // the native source to replay anything that was not committed.
                let resumeCheckpoint = self.pendingJournalCheckpoints[rootKey]
                    ?? currentTarget.checkpoint
                let stream = coordinator.sessions(
                    rootPath: currentTarget.rootPath,
                    sinceEventID: resumeCheckpoint?.eventID,
                    options: currentTarget.options.diskActivityOptions,
                    // Background watches don't need sub-second delivery; a wide
                    // latency window lets the kernel coalesce and saves wakeups.
                    monitorLatency: 30,
                    exclusionFilter: self.dependencies.activityExclusion(
                        currentTarget.options.exclusionPatterns,
                        currentTarget.rootPath
                    ),
                    sizeProviders: sizeProviders
                )
                var capturedInitialBaseline = false
                var lastGapCount: UInt64 = 0
                var isInitialSessionYield = true
                for await session in stream {
                    guard !Task.isCancelled, self.longTermWatchTaskIDs[target.id] == taskID else {
                        break
                    }

                    // The coordinator yields once before the monitor stream produces
                    // anything, so only later yields prove the watch is alive; resetting
                    // on the first would defeat the reconnect backoff for dead watches.
                    if isInitialSessionYield {
                        isInitialSessionYield = false
                    } else {
                        retryCount = 0
                    }
                    let existingCheckpoint = self.pendingJournalCheckpoints[rootKey]
                        ?? self.longTermWatchTargets.first(where: { $0.id == target.id })?.checkpoint
                    var checkpointToStage: LongTermWatchCheckpoint?
                    if let eventID = session.lastObservedEventID,
                       eventID != existingCheckpoint?.eventID || session.historyState == .gapDetected {
                        checkpointToStage = LongTermWatchCheckpoint(
                            eventID: eventID,
                            recordedAt: Date(),
                            hasHistoryGap: existingCheckpoint?.hasHistoryGap == true || session.historyState == .gapDetected
                        )
                    }

                    // A root change (rename, delete, unmount) arrives as a
                    // rescan request, so that is the moment to confirm the
                    // folder is still there rather than keep saying "Watching".
                    let runtimeState: LongTermWatchRuntimeState
                    switch session.historyState {
                    case .live:
                        runtimeState = existingCheckpoint?.hasHistoryGap == true ? .historyGap : .watching
                    case .catchingUp:
                        runtimeState = .catchingUp
                    case .gapDetected:
                        runtimeState = Self.rootExists(currentTarget.rootPath) ? .historyGap : .rootMissing
                    }
                    self.updateLongTermWatchRuntimeStatus(
                        targetID: target.id,
                        state: runtimeState,
                        retryCount: retryCount
                    )

                    // Register the source before beginning metadata enumeration. A
                    // separate task keeps consuming events while the scan is slow.
                    let newGap = session.historyGapCount > lastGapCount
                    lastGapCount = session.historyGapCount
                    if session.receivedStreamEventCount > 0,
                       session.historyState != .catchingUp,
                       !capturedInitialBaseline || newGap {
                        capturedInitialBaseline = true
                        self.captureLongTermBaseline(for: target, watchTaskID: taskID)
                    }
                    if !session.latestChanges.isEmpty {
                        self.updateLongTermWatchRuntimeStatus(
                            targetID: target.id,
                            state: runtimeState,
                            lastActivityAt: session.latestChanges.map(\.event.timestamp).max(),
                            retryCount: retryCount
                        )
                        self.enqueueForJournal(session.latestChanges, rootPath: session.rootPath)
                    }
                    if let checkpointToStage {
                        self.stageCheckpoint(checkpointToStage, rootPath: target.rootPath)
                    }
                }

                guard !Task.isCancelled, self.longTermWatchTaskIDs[target.id] == taskID else {
                    break
                }

                // A source ending invalidates any scan started under that stream,
                // even though the outer watch task survives the reconnect loop.
                self.longTermWatchBaselineTasks[target.id]?.cancel()
                self.longTermWatchBaselineTasks[target.id] = nil
                self.longTermWatchBaselineIDs[target.id] = nil
                retryCount += 1
                self.updateLongTermWatchRuntimeStatus(
                    targetID: target.id,
                    state: Self.rootExists(currentTarget.rootPath) ? .reconnecting : .rootMissing,
                    retryCount: retryCount
                )
                let delayNanoseconds = UInt64(min(1 << min(retryCount - 1, 5), 30)) * 1_000_000_000
                try? await Task.sleep(nanoseconds: delayNanoseconds)
            }
        }
    }

    private func captureLongTermBaseline(for target: LongTermWatchTarget, watchTaskID: UUID) {
        // A distinct gap supersedes an older in-flight scan. Cancellation IDs
        // prevent a removed/restarted watch from being resurrected on completion.
        longTermWatchBaselineTasks[target.id]?.cancel()
        let scanID = UUID()
        longTermWatchBaselineIDs[target.id] = scanID
        let service = dependencies.activityBaselineService
        longTermWatchBaselineTasks[target.id] = Task { @MainActor [weak self] in
            let baseline = await service.captureBaseline(rootPath: target.rootPath)
            guard let self, !Task.isCancelled,
                  self.longTermWatchTaskIDs[target.id] == watchTaskID,
                  self.longTermWatchBaselineIDs[target.id] == scanID,
                  self.longTermWatchTargets.contains(where: { $0.id == target.id && $0.isEnabled }) else { return }
            self.longTermWatchBaselineTasks[target.id] = nil
            self.longTermWatchBaselineIDs[target.id] = nil
            self.longTermWatchTargets = self.dependencies.longTermWatchTargets.updateBaseline(
                baseline, forRootPath: target.rootPath, currentTargets: self.longTermWatchTargets
            )
            // Notification latency means a quiet callback interval does not prove
            // a quiet filesystem. Until the monitor exposes a flush/barrier, keep
            // this scan unverified and do not append a guessed correction row.
            // The persisted history-gap checkpoint remains set after this scan.
        }
    }

    private func stopLongTermWatch(targetID: LongTermWatchTarget.ID) {
        longTermWatchBaselineTasks[targetID]?.cancel()
        longTermWatchBaselineTasks[targetID] = nil
        longTermWatchBaselineIDs[targetID] = nil
        flushJournalNow()
        longTermWatchTasks[targetID]?.cancel()
        longTermWatchTasks[targetID] = nil
        longTermWatchTaskIDs[targetID] = nil
        updateLongTermWatchRuntimeStatus(targetID: targetID, state: .paused, retryCount: 0)
    }

    private func stopAllLongTermWatches() {
        cancelLongTermWatchBaseline()
        for task in longTermWatchTasks.values {
            task.cancel()
        }
        longTermWatchTasks.removeAll()
        longTermWatchTaskIDs.removeAll()
        for target in longTermWatchTargets {
            updateLongTermWatchRuntimeStatus(targetID: target.id, state: .paused, retryCount: 0)
        }
    }

    func setLaunchAtLoginEnabled(_ enabled: Bool) {
        do {
            try dependencies.launchAtLoginService.setEnabled(enabled)
        } catch {
            lastErrorMessage = "Pathlight could not update its Login Item setting."
        }
        launchAtLoginStatus = dependencies.launchAtLoginService.currentStatus()
    }

    func refreshLaunchAtLoginStatus() {
        launchAtLoginStatus = dependencies.launchAtLoginService.currentStatus()
    }

    /// Monitoring only runs while the app does, so nudge toward launch-at-login
    /// once targets exist; the nudge disappears forever once dismissed or enabled.
    var shouldShowLaunchAtLoginNudge: Bool {
        !launchAtLoginNudgeDismissed
            && launchAtLoginStatus == .disabled
            && longTermWatchTargets.contains(where: \.isEnabled)
    }

    func dismissLaunchAtLoginNudge() {
        launchAtLoginNudgeDismissed = true
        UserDefaults.standard.set(true, forKey: Self.launchAtLoginNudgeDismissedKey)
    }

    func openLoginItemsSettings() {
        dependencies.launchAtLoginService.openLoginItemsSettings()
    }

    /// One `stat` on a path the user chose, only when the monitor already told
    /// us something changed about the root. Cheap enough not to count as polling.
    private nonisolated static func rootExists(_ rootPath: URL) -> Bool {
        FileManager.default.fileExists(atPath: rootPath.standardizedFileURL.path)
    }

    private func updateLongTermWatchRuntimeStatus(
        targetID: LongTermWatchTarget.ID,
        state: LongTermWatchRuntimeState,
        lastActivityAt: Date? = nil,
        retryCount: Int
    ) {
        let existingStatus = longTermWatchRuntimeStatuses[targetID]
        longTermWatchRuntimeStatuses[targetID] = LongTermWatchRuntimeStatus(
            state: state,
            lastActivityAt: lastActivityAt ?? existingStatus?.lastActivityAt,
            retryCount: retryCount
        )
    }

    // MARK: - Preferences persistence

    private func observeActivityStoragePreferences() {
        Publishers.CombineLatest4(
            $activityDetailedRetentionDays,
            $activityAggregateRetentionDays,
            $activityStorageLimitBytes,
            $activityEncryptNewData
        )
            .map { detailedRetentionDays, aggregateRetentionDays, storageLimitBytes, encryptNewData in
                ActivityStoragePreferences(
                    detailedRetentionDays: detailedRetentionDays,
                    aggregateRetentionDays: aggregateRetentionDays,
                    storageLimitBytes: storageLimitBytes,
                    encryptNewData: encryptNewData
                )
            }
            .dropFirst()
            .removeDuplicates()
            .debounce(for: Self.preferencePersistenceDebounce, scheduler: RunLoop.main)
            .sink { [weak self] preferences in
                self?.persistActivityStoragePreferences(preferences)
            }
            .store(in: &cancellables)
    }

    private var currentActivityStoragePreferences: ActivityStoragePreferences {
        ActivityStoragePreferences(
            detailedRetentionDays: activityDetailedRetentionDays,
            aggregateRetentionDays: activityAggregateRetentionDays,
            storageLimitBytes: activityStorageLimitBytes,
            encryptNewData: activityEncryptNewData
        )
    }

    private func persistActivityStoragePreferences(_ preferences: ActivityStoragePreferences) {
        guard lastPersistedActivityStoragePreferences != preferences else { return }
        dependencies.activityStoragePreferences.savePreferences(preferences)
        lastPersistedActivityStoragePreferences = preferences
        applyActivityStoragePolicy()
    }

    private func applyActivityStoragePolicy() {
        guard let activityEventStore = dependencies.activityEventStore else {
            return
        }

        cancelActivityStoragePolicy()
        let taskID = UUID()
        activityStoragePolicyTaskID = taskID
        let preferences = currentActivityStoragePreferences
        let usageService = dependencies.activityStorageUsageService
        activityStoragePolicyTask = Task { @MainActor [weak self] in
            let eventJournalLimitBytes = await usageService.availableEventJournalBytes(
                storageLimitBytes: preferences.storageLimitBytes
            )
            guard !Task.isCancelled, self?.activityStoragePolicyTaskID == taskID else {
                return
            }
            do {
                try await activityEventStore.enforceStoragePolicy(
                    preferences,
                    eventJournalLimitBytes: eventJournalLimitBytes,
                    now: Date()
                )
                guard !Task.isCancelled, self?.activityStoragePolicyTaskID == taskID else {
                    return
                }
                self?.activityStoragePolicyTask = nil
                self?.activityStoragePolicyTaskID = nil
                self?.refreshActivityStorageUsage()
            } catch {
                // A cleanup failure should not interrupt disk activity monitoring.
                if self?.activityStoragePolicyTaskID == taskID {
                    self?.activityStoragePolicyTask = nil
                    self?.activityStoragePolicyTaskID = nil
                }
            }
        }
    }

    private func cancelActivityStoragePolicy() {
        activityStoragePolicyTask?.cancel()
        activityStoragePolicyTask = nil
        activityStoragePolicyTaskID = nil
    }
}
