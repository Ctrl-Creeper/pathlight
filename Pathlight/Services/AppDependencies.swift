//
//  AppDependencies.swift
//  Pathlight
//

import Foundation

@MainActor
struct AppDependencies {
    var systemActions: AppSystemActions
    var activityMonitor: any DiskActivityMonitoring
    /// Byte attribution for one watch. Lives in the Rust core, so only a host
    /// that links it can supply one.
    var activityAttribution: ActivityAttributionFactory
    /// Noise filtering for one watch. Also lives in the Rust core, so this
    /// package has nothing to fall back on: filtering nothing records more
    /// than the user asked for, never less.
    var activityExclusion: ActivityExclusionFactory
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
    /// Short-window anomaly detection. The thresholds live in the Rust core,
    /// so a host that does not link it raises no anomalies rather than
    /// inventing its own idea of one.
    var activityAnomalies: ActivityAnomalyDetecting
    var processHints: (any ProcessHinting)?
    /// Loads the activity storage key up front; no-op when nothing is encrypted.
    var activityStorageKeyWarmUp: @Sendable () throws -> Void

    init(
        systemActions: AppSystemActions,
        activityMonitor: any DiskActivityMonitoring,
        activityAttribution: @escaping ActivityAttributionFactory,
        activityExclusion: @escaping ActivityExclusionFactory = { _, _ in nil },
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
        activityAnomalies: @escaping ActivityAnomalyDetecting = { _, _, _ in [] },
        processHints: (any ProcessHinting)? = nil,
        activityStorageKeyWarmUp: @escaping @Sendable () throws -> Void = {}
    ) {
        self.systemActions = systemActions
        self.activityMonitor = activityMonitor
        self.activityAttribution = activityAttribution
        self.activityExclusion = activityExclusion
        self.activitySizeProviders = activitySizeProviders
        self.activityEventStore = activityEventStore
        self.longTermWatchTargets = longTermWatchTargets
        self.activityBaselineService = activityBaselineService
        self.activityStoragePreferences = activityStoragePreferences
        self.activityStorageUsageService = activityStorageUsageService
        self.launchAtLoginService = launchAtLoginService
        self.activityGrowthAlertPoster = activityGrowthAlertPoster
        self.activityAnomalies = activityAnomalies
        self.processHints = processHints
        self.activityStorageKeyWarmUp = activityStorageKeyWarmUp
    }

    /// Every platform-specific piece is a parameter: this package holds no
    /// monitor, no attribution, no exclusion, no anomaly policy and no journal
    /// line format, because all five live in the Rust core that the app links
    /// and `swift test` does not.
    ///
    /// The cryptor has no default on purpose: a fallback would write plaintext
    /// rows for a user who asked for encryption.
    static func live(
        activityMonitor: any DiskActivityMonitoring,
        activityAttribution: @escaping ActivityAttributionFactory,
        activityExclusion: @escaping ActivityExclusionFactory,
        activityAnomalies: @escaping ActivityAnomalyDetecting,
        activityStorageCryptor: any ActivityStorageLineCrypting,
        activitySizeIndex: ActivitySizeIndex? = nil
    ) -> AppDependencies {
        let activityStoragePreferences = UserDefaultsActivityStoragePreferencesStore()
        let activityStorageLineCodec = ActivityStorageLineCodec(
            preferencesStore: activityStoragePreferences,
            cryptor: activityStorageCryptor
        )
        let activitySizeIndex = activitySizeIndex ?? ActivitySizeIndex.live(lineCodec: activityStorageLineCodec)
        return AppDependencies(
            systemActions: .live,
            activityMonitor: activityMonitor,
            activityAttribution: activityAttribution,
            activityExclusion: activityExclusion,
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
            activityAnomalies: activityAnomalies,
            processHints: LsofProcessHintService(),
            activityStorageKeyWarmUp: { try activityStorageLineCodec.prepare() }
        )
    }
}
