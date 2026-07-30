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
    var activityStoragePreferences: any ActivityStoragePreferencesPersisting
    var activityStorageUsageService: ActivityStorageUsageService
    var launchAtLoginService: any LaunchAtLoginControlling
    var activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)?

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
        activityBaselineService: ActivityBaselineService = ActivityBaselineService(),
        activityStoragePreferences: any ActivityStoragePreferencesPersisting = UserDefaultsActivityStoragePreferencesStore(),
        activityStorageUsageService: ActivityStorageUsageService = ActivityStorageUsageService(),
        launchAtLoginService: any LaunchAtLoginControlling = SystemLaunchAtLoginService(),
        activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)? = nil
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
        self.activityStoragePreferences = activityStoragePreferences
        self.activityStorageUsageService = activityStorageUsageService
        self.launchAtLoginService = launchAtLoginService
        self.activityGrowthAlertPoster = activityGrowthAlertPoster
    }

    static var live: AppDependencies {
        let systemActions = AppSystemActions.live
        let activityStoragePreferences = UserDefaultsActivityStoragePreferencesStore()
        let activityStorageLineCodec = ActivityStorageLineCodec(
            preferencesStore: activityStoragePreferences
        )
        let activitySizeIndex = ActivitySizeIndex.live(lineCodec: activityStorageLineCodec)
        let activitySizeProvider: StorageAttributionService.SizeProvider = { url in
            activitySizeIndex.recordKnownSize(
                FileAllocatedSizeProvider.allocatedSize(for: url),
                for: url
            )
        }
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
            usageStats: UserDefaultsAppUsageStatsStore(),
            activitySizeProvider: activitySizeProvider,
            activityPriorSizeProvider: { url in
                activitySizeIndex.takeKnownSize(for: url)
            },
            activityEventStore: JSONLActivityEventStore.live(lineCodec: activityStorageLineCodec),
            activityBaselineService: ActivityBaselineService(sizeProvider: activitySizeProvider),
            activityStoragePreferences: activityStoragePreferences,
            activityStorageUsageService: ActivityStorageUsageService(lineCodec: activityStorageLineCodec),
            launchAtLoginService: SystemLaunchAtLoginService(),
            activityGrowthAlertPoster: UserNotificationGrowthAlertPoster()
        )
    }
}
