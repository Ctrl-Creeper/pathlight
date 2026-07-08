//
//  AppDependencies.swift
//  Pathlight
//

import Foundation

@MainActor
struct AppDependencies {
    var preferences: any AppPreferencesPersisting
    var recentTargets: RecentTargetStore
    var systemActions: AppSystemActions
    var scanService: any ScanEventStreaming
    var scanArchiveService: any ScanArchiveServicing
    var usageStats: any AppUsageStatsPersisting
    var activityMonitor: any DiskActivityMonitoring
    var activitySizeProvider: StorageAttributionService.SizeProvider
    var activityPriorSizeProvider: StorageAttributionService.SizeProvider
    var activityEventStore: (any ActivityEventStoring)?
    var longTermWatchTargets: LongTermWatchTargetStore
    var activityBaselineService: ActivityBaselineService

    init(
        preferences: any AppPreferencesPersisting,
        recentTargets: RecentTargetStore,
        systemActions: AppSystemActions,
        scanService: any ScanEventStreaming = ScanEngine(),
        scanArchiveService: any ScanArchiveServicing = ScanArchiveService(),
        usageStats: any AppUsageStatsPersisting = InMemoryAppUsageStatsStore(),
        activityMonitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor(),
        activitySizeProvider: @escaping StorageAttributionService.SizeProvider = FileAllocatedSizeProvider.allocatedSize(for:),
        activityPriorSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil },
        activityEventStore: (any ActivityEventStoring)? = JSONLActivityEventStore.live(),
        longTermWatchTargets: LongTermWatchTargetStore = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence()
        ),
        activityBaselineService: ActivityBaselineService = ActivityBaselineService()
    ) {
        self.preferences = preferences
        self.recentTargets = recentTargets
        self.systemActions = systemActions
        self.scanService = scanService
        self.scanArchiveService = scanArchiveService
        self.usageStats = usageStats
        self.activityMonitor = activityMonitor
        self.activitySizeProvider = activitySizeProvider
        self.activityPriorSizeProvider = activityPriorSizeProvider
        self.activityEventStore = activityEventStore
        self.longTermWatchTargets = longTermWatchTargets
        self.activityBaselineService = activityBaselineService
    }

    static var live: AppDependencies {
        let systemActions = AppSystemActions.live
        return AppDependencies(
            preferences: UserDefaultsAppPreferencesStore(),
            recentTargets: RecentTargetStore(
                persistence: UserDefaultsRecentTargetPersistence(),
                isAvailable: { target in
                    systemActions.isExistingDirectory(target.url)
                }
            ),
            systemActions: systemActions,
            scanService: ScanEngine(),
            usageStats: UserDefaultsAppUsageStatsStore()
        )
    }
}
