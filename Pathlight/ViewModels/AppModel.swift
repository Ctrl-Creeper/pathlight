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
    private var activityDashboardHistoryTask: Task<Void, Never>?
    private var activityDashboardHistoryTaskID: UUID?
    private var activityStorageUsageTask: Task<Void, Never>?
    private var activityStorageUsageTaskID: UUID?
    private var activityStoragePolicyTask: Task<Void, Never>?
    private var activityStoragePolicyTaskID: UUID?
    private var longTermWatchBaselineTask: Task<Void, Never>?
    private var longTermWatchBaselineTaskID: UUID?
    private var longTermWatchTasks: [LongTermWatchTarget.ID: Task<Void, Never>] = [:]
    private var longTermWatchTaskIDs: [LongTermWatchTarget.ID: UUID] = [:]

    init(dependencies: AppDependencies = .live()) {
        self.dependencies = dependencies

        let activityStoragePreferences = dependencies.activityStoragePreferences.loadPreferences()
        activityDetailedRetentionDays = activityStoragePreferences.detailedRetentionDays
        activityAggregateRetentionDays = activityStoragePreferences.aggregateRetentionDays
        activityStorageLimitBytes = activityStoragePreferences.storageLimitBytes
        activityEncryptNewData = activityStoragePreferences.encryptNewData
        lastPersistedActivityStoragePreferences = activityStoragePreferences
        longTermWatchTargets = dependencies.longTermWatchTargets.loadTargets()
        launchAtLoginStatus = dependencies.launchAtLoginService.currentStatus()

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

    func startShortTermWatch(
        rootPath: URL,
        options: DiskActivityAggregationOptions = .shortTermDefault
    ) {
        stopShortTermWatch()

        let taskID = UUID()
        liveWatchTaskID = taskID
        let coordinator = LiveWatchSessionCoordinator(
            monitor: dependencies.activityMonitor,
            processHints: dependencies.processHints
        )
        let sizeProvider = dependencies.activitySizeProvider
        let priorSizeProvider = dependencies.activityPriorSizeProvider
        let baselineService = dependencies.activityBaselineService

        liveWatchBaselineTask = Task.detached {
            _ = await baselineService.captureBaseline(rootPath: rootPath)
        }

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
                sizeProvider: sizeProvider,
                priorSizeProvider: priorSizeProvider
            )
            var persistedEventCount = 0
            for await session in stream {
                guard !Task.isCancelled, self.liveWatchTaskID == taskID else {
                    break
                }
                self.liveWatchSession = session
                if session.events.count > persistedEventCount {
                    let newEvents = Array(session.events[persistedEventCount...])
                    persistedEventCount = session.events.count
                    if let activityEventStore = self.dependencies.activityEventStore {
                        do {
                            try await activityEventStore.append(newEvents)
                            self.applyActivityStoragePolicy()
                            self.scheduleEventDrivenHistoryRefresh(rootPath: session.rootPath)
                        } catch {
                            // A failed journal write should not interrupt the live watch session.
                        }
                    }
                }
            }
        }
    }

    func stopShortTermWatch() {
        liveWatchTask?.cancel()
        liveWatchTask = nil
        liveWatchTaskID = nil
        liveWatchBaselineTask?.cancel()
        liveWatchBaselineTask = nil
        liveWatchSession = nil
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
        cancelLongTermWatchBaseline()

        let taskID = UUID()
        longTermWatchBaselineTaskID = taskID
        let baselineService = dependencies.activityBaselineService

        longTermWatchBaselineTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.longTermWatchBaselineTaskID == taskID {
                    self.longTermWatchBaselineTask = nil
                    self.longTermWatchBaselineTaskID = nil
                }
            }

            let baseline = await baselineService.captureBaseline(rootPath: rootPath)
            guard !Task.isCancelled, self.longTermWatchBaselineTaskID == taskID else {
                return
            }

            let target = LongTermWatchTarget(
                rootPath: rootPath,
                isEnabled: true,
                options: options,
                baseline: baseline
            )
            self.longTermWatchTargets = self.dependencies.longTermWatchTargets.upsert(
                target,
                currentTargets: self.longTermWatchTargets
            )
            self.startLongTermWatch(for: target)
        }
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
        let normalizedPatterns = ScanExclusionMatcher.normalizedPatterns(patterns)
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
        longTermWatchBaselineTask?.cancel()
        longTermWatchBaselineTask = nil
        longTermWatchBaselineTaskID = nil
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
            state: .starting,
            retryCount: 0
        )
        let coordinator = LiveWatchSessionCoordinator(
            monitor: dependencies.activityMonitor,
            processHints: dependencies.processHints
        )
        let sizeProvider = dependencies.activitySizeProvider
        let priorSizeProvider = dependencies.activityPriorSizeProvider
        let eventStore = dependencies.activityEventStore

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
                let stream = coordinator.sessions(
                    rootPath: currentTarget.rootPath,
                    sinceEventID: currentTarget.checkpoint?.eventID,
                    options: currentTarget.options.diskActivityOptions,
                    // Background watches don't need sub-second delivery; a wide
                    // latency window lets the kernel coalesce and saves wakeups.
                    monitorLatency: 30,
                    exclusionFilter: ActivityExclusionFilter(
                        patterns: currentTarget.options.exclusionPatterns,
                        rootPath: currentTarget.rootPath
                    ),
                    sizeProvider: sizeProvider,
                    priorSizeProvider: priorSizeProvider
                )
                var persistedEventCount = 0
                var didCaptureGapBaseline = false
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
                    let existingCheckpoint = self.longTermWatchTargets.first(where: { $0.id == target.id })?.checkpoint
                    if let eventID = session.lastObservedEventID,
                       eventID != existingCheckpoint?.eventID || session.historyState == .gapDetected {
                        let checkpoint = LongTermWatchCheckpoint(
                            eventID: eventID,
                            recordedAt: Date(),
                            hasHistoryGap: existingCheckpoint?.hasHistoryGap == true || session.historyState == .gapDetected
                        )
                        self.longTermWatchTargets = self.dependencies.longTermWatchTargets.updateCheckpoint(
                            checkpoint,
                            forRootPath: target.rootPath,
                            currentTargets: self.longTermWatchTargets
                        )
                    }

                    let runtimeState: LongTermWatchRuntimeState
                    switch session.historyState {
                    case .live:
                        runtimeState = existingCheckpoint?.hasHistoryGap == true ? .historyGap : .watching
                    case .catchingUp:
                        runtimeState = .catchingUp
                    case .gapDetected:
                        runtimeState = .historyGap
                    }
                    self.updateLongTermWatchRuntimeStatus(
                        targetID: target.id,
                        state: runtimeState,
                        retryCount: retryCount
                    )

                    if session.historyState == .gapDetected, !didCaptureGapBaseline {
                        didCaptureGapBaseline = true
                        let baseline = await self.dependencies.activityBaselineService.captureBaseline(rootPath: target.rootPath)
                        guard !Task.isCancelled, self.longTermWatchTaskIDs[target.id] == taskID else {
                            break
                        }
                        let previousBaseline = self.longTermWatchTargets.first(where: { $0.id == target.id })?.baseline
                        self.longTermWatchTargets = self.dependencies.longTermWatchTargets.updateBaseline(
                            baseline,
                            forRootPath: target.rootPath,
                            currentTargets: self.longTermWatchTargets
                        )
                        if let eventStore,
                           let reconciliation = ActivityBaselineReconciler.reconciliationEvent(
                               previous: previousBaseline,
                               current: baseline
                           ) {
                            try? await eventStore.append([reconciliation])
                            self.scheduleEventDrivenHistoryRefresh(rootPath: target.rootPath)
                        }
                    }
                    if session.events.count > persistedEventCount {
                        let newEvents = Array(session.events[persistedEventCount...])
                        persistedEventCount = session.events.count
                        self.updateLongTermWatchRuntimeStatus(
                            targetID: target.id,
                            state: runtimeState,
                            lastActivityAt: newEvents.map(\.timestamp).max(),
                            retryCount: retryCount
                        )
                        if let eventStore {
                            do {
                                try await eventStore.append(newEvents)
                                self.applyActivityStoragePolicy()
                                self.scheduleEventDrivenHistoryRefresh(rootPath: session.rootPath)
                            } catch {
                                // A failed journal write should not interrupt long-term monitoring.
                            }
                        }
                    }
                }

                guard !Task.isCancelled, self.longTermWatchTaskIDs[target.id] == taskID else {
                    break
                }

                retryCount += 1
                self.updateLongTermWatchRuntimeStatus(
                    targetID: target.id,
                    state: .reconnecting,
                    retryCount: retryCount
                )
                let delayNanoseconds = UInt64(min(1 << min(retryCount - 1, 5), 30)) * 1_000_000_000
                try? await Task.sleep(nanoseconds: delayNanoseconds)
            }
        }
    }

    private func stopLongTermWatch(targetID: LongTermWatchTarget.ID) {
        longTermWatchTasks[targetID]?.cancel()
        longTermWatchTasks[targetID] = nil
        longTermWatchTaskIDs[targetID] = nil
        updateLongTermWatchRuntimeStatus(targetID: targetID, state: .paused, retryCount: 0)
    }

    private func stopAllLongTermWatches() {
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
