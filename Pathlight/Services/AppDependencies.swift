//
//  AppDependencies.swift
//  Pathlight
//

import Foundation

/// The three byte-attribution lookups one watch needs, already scoped to it.
nonisolated struct ActivitySizeProviders: Sendable {
    var size: StorageAttributionService.SizeProvider
    var prior: StorageAttributionService.SizeProvider = { _ in nil }
    var known: StorageAttributionService.SizeProvider = { _ in nil }

    /// Stable scope for a watch: live and background watches over one root are
    /// separate observers, so they must not share a baseline either.
    nonisolated static func scope(kind: String, rootPath: URL) -> String {
        "\(kind):\(rootPath.standardizedFileURL.path)"
    }
}

@MainActor
struct AppDependencies {
    var systemActions: AppSystemActions
    var activityMonitor: any DiskActivityMonitoring
    /// Byte-attribution providers for one watch. Each watch asks for its own
    /// scope so overlapping watches cannot consume each other's measurements.
    var activitySizeProviders: @Sendable (_ scope: String) -> ActivitySizeProviders
    var activityEventStore: (any ActivityEventStoring)?
    var longTermWatchTargets: LongTermWatchTargetStore
    var activityBaselineService: ActivityBaselineService
    var activityStoragePreferences: any ActivityStoragePreferencesPersisting
    var activityStorageUsageService: ActivityStorageUsageService
    var launchAtLoginService: any LaunchAtLoginControlling
    var activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)?
    var processHints: (any ProcessHinting)?
    /// Loads the activity storage key up front; no-op when nothing is encrypted.
    var activityStorageKeyWarmUp: @Sendable () throws -> Void

    init(
        systemActions: AppSystemActions,
        activityMonitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor(),
        activitySizeProviders: @escaping @Sendable (String) -> ActivitySizeProviders = { _ in
            ActivitySizeProviders(size: FileAllocatedSizeProvider.allocatedSize(for:))
        },
        activityEventStore: (any ActivityEventStoring)? = JSONLActivityEventStore.live(),
        longTermWatchTargets: LongTermWatchTargetStore = LongTermWatchTargetStore(
            persistence: UserDefaultsLongTermWatchTargetPersistence()
        ),
        activityBaselineService: ActivityBaselineService = ActivityBaselineService(),
        activityStoragePreferences: any ActivityStoragePreferencesPersisting = UserDefaultsActivityStoragePreferencesStore(),
        activityStorageUsageService: ActivityStorageUsageService = ActivityStorageUsageService(),
        launchAtLoginService: any LaunchAtLoginControlling = SystemLaunchAtLoginService(),
        activityGrowthAlertPoster: (any ActivityGrowthAlertPosting)? = nil,
        processHints: (any ProcessHinting)? = nil,
        activityStorageKeyWarmUp: @escaping @Sendable () throws -> Void = {}
    ) {
        self.systemActions = systemActions
        self.activityMonitor = activityMonitor
        self.activitySizeProviders = activitySizeProviders
        self.activityEventStore = activityEventStore
        self.longTermWatchTargets = longTermWatchTargets
        self.activityBaselineService = activityBaselineService
        self.activityStoragePreferences = activityStoragePreferences
        self.activityStorageUsageService = activityStorageUsageService
        self.launchAtLoginService = launchAtLoginService
        self.activityGrowthAlertPoster = activityGrowthAlertPoster
        self.processHints = processHints
        self.activityStorageKeyWarmUp = activityStorageKeyWarmUp
    }

    /// `activityMonitor` defaults to the in-process FSEvents wrapper; the app
    /// passes the Rust-backed monitor instead.
    static func live(
        activityMonitor: any DiskActivityMonitoring = FSEventsDiskActivityMonitor(),
        activitySizeIndex: ActivitySizeIndex? = nil
    ) -> AppDependencies {
        let activityStoragePreferences = UserDefaultsActivityStoragePreferencesStore()
        let activityStorageLineCodec = ActivityStorageLineCodec(
            preferencesStore: activityStoragePreferences
        )
        let activitySizeIndex = activitySizeIndex ?? ActivitySizeIndex.live(lineCodec: activityStorageLineCodec)
        return AppDependencies(
            systemActions: .live,
            activityMonitor: activityMonitor,
            activitySizeProviders: { scope in
                ActivitySizeProviders(
                    size: { url in
                        activitySizeIndex.recordKnownSize(
                            FileAllocatedSizeProvider.allocatedSize(for: url),
                            for: url,
                            scope: scope
                        )
                    },
                    prior: { url in activitySizeIndex.takeKnownSize(for: url, scope: scope) },
                    known: { url in activitySizeIndex.knownSize(for: url, scope: scope) }
                )
            },
            activityEventStore: JSONLActivityEventStore.live(lineCodec: activityStorageLineCodec),
            // A scan may observe a new size before its delayed change event.
            // Keep those measurements out of the live attribution index so they
            // cannot consume an increment or overwrite a newer event measurement.
            activityBaselineService: ActivityBaselineService(),
            activityStoragePreferences: activityStoragePreferences,
            activityStorageUsageService: ActivityStorageUsageService(lineCodec: activityStorageLineCodec),
            launchAtLoginService: SystemLaunchAtLoginService(),
            activityGrowthAlertPoster: UserNotificationGrowthAlertPoster(),
            processHints: LsofProcessHintService(),
            activityStorageKeyWarmUp: { try activityStorageLineCodec.prepare() }
        )
    }
}
