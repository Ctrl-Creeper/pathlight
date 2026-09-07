//
//  AppDependencies.swift
//  Pathlight
//

import Foundation

@MainActor
struct AppDependencies {
    var systemActions: AppSystemActions
    var activityMonitor: any DiskActivityMonitoring
    var activitySizeProvider: StorageAttributionService.SizeProvider
    var activityPriorSizeProvider: StorageAttributionService.SizeProvider
    var activityKnownSizeProvider: StorageAttributionService.SizeProvider
    var activityEventStore: (any ActivityEventStoring)?
    var longTermWatchTargets: LongTermWatchTargetStore
    var activityBaselineService: ActivityBaselineService
    var activityStoragePreferences: any ActivityStoragePreferencesPersisting
    var activityStorageUsageService: ActivityStorageUsageService
    var launchAtLoginService: any LaunchAtLoginControlling
    var activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)?
    var processHints: (any ProcessHinting)?

    init(
        systemActions: AppSystemActions,
        activityMonitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor(),
        activitySizeProvider: @escaping StorageAttributionService.SizeProvider = FileAllocatedSizeProvider.allocatedSize(for:),
        activityPriorSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil },
        activityKnownSizeProvider: @escaping StorageAttributionService.SizeProvider = { _ in nil },
        activityEventStore: (any ActivityEventStoring)? = JSONLActivityEventStore.live(),
        longTermWatchTargets: LongTermWatchTargetStore = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence()
        ),
        activityBaselineService: ActivityBaselineService = ActivityBaselineService(),
        activityStoragePreferences: any ActivityStoragePreferencesPersisting = UserDefaultsActivityStoragePreferencesStore(),
        activityStorageUsageService: ActivityStorageUsageService = ActivityStorageUsageService(),
        launchAtLoginService: any LaunchAtLoginControlling = SystemLaunchAtLoginService(),
        activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)? = nil,
        processHints: (any ProcessHinting)? = nil
    ) {
        self.systemActions = systemActions
        self.activityMonitor = activityMonitor
        self.activitySizeProvider = activitySizeProvider
        self.activityPriorSizeProvider = activityPriorSizeProvider
        self.activityKnownSizeProvider = activityKnownSizeProvider
        self.activityEventStore = activityEventStore
        self.longTermWatchTargets = longTermWatchTargets
        self.activityBaselineService = activityBaselineService
        self.activityStoragePreferences = activityStoragePreferences
        self.activityStorageUsageService = activityStorageUsageService
        self.launchAtLoginService = launchAtLoginService
        self.activityGrowthAlertPoster = activityGrowthAlertPoster
        self.processHints = processHints
    }

    /// `activityMonitor` defaults to the in-process FSEvents wrapper; the app
    /// passes the Rust-backed monitor instead.
    static func live(
        activityMonitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor()
    ) -> AppDependencies {
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
            systemActions: .live,
            activityMonitor: activityMonitor,
            activitySizeProvider: activitySizeProvider,
            activityPriorSizeProvider: { url in
                activitySizeIndex.takeKnownSize(for: url)
            },
            activityKnownSizeProvider: { url in
                activitySizeIndex.knownSize(for: url)
            },
            activityEventStore: JSONLActivityEventStore.live(lineCodec: activityStorageLineCodec),
            activityBaselineService: ActivityBaselineService(sizeProvider: activitySizeProvider),
            activityStoragePreferences: activityStoragePreferences,
            activityStorageUsageService: ActivityStorageUsageService(lineCodec: activityStorageLineCodec),
            launchAtLoginService: SystemLaunchAtLoginService(),
            activityGrowthAlertPoster: UserNotificationGrowthAlertPoster(),
            processHints: LsofProcessHintService()
        )
    }
}
