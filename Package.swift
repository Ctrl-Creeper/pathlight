// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "PathlightCore",
    platforms: [
        .macOS("14.0")
    ],
    products: [
        .library(
            name: "PathlightCore",
            targets: ["PathlightCore"]
        )
    ],
    targets: [
        .target(
            name: "PathlightCore",
            path: "Pathlight",
            exclude: [
                "App",
                "AppIcon.icon",
                "Assets.xcassets",
                "ContentView.swift",
                "Features",
                "Info.plist",
                "PathlightApp.swift",
                "Shared"
            ],
            sources: [
                "Services/ActivityAttribution.swift",
                "Services/ActivityBaselineService.swift",
                "Services/ActivityDashboardPresentation.swift",
                "Services/ActivityEventStore.swift",
                "Services/ActivityExclusionFilter.swift",
                "Services/ActivityGrowthAlertService.swift",
                "Services/ActivityHistoryService.swift",
                "Services/ActivityHistoryPresentation.swift",
                "Services/ActivityExportService.swift",
                "Services/ActivityNoiseAdvisor.swift",
                "Services/ActivitySizeIndex.swift",
                "Services/ActivityStoragePreferencesStore.swift",
                "Services/ActivityStorageFileProtection.swift",
                "Services/ActivityStorageLineCodec.swift",
                "Services/ActivityStorageUsageService.swift",
                "Services/ActivityTimelinePresentation.swift",
                "Services/AppDependencies.swift",
                "Services/AppSystemActions.swift",
                "Services/CommandLineToolInstaller.swift",
                "Services/DiskActivityMonitor.swift",
                "Services/DiskActivityModels.swift",
                "Services/FileSizeFormatter.swift",
                "Services/LaunchAtLoginService.swift",
                "Services/LiveWatchSessionCoordinator.swift",
                "Services/LongTermWatchTargetStore.swift",
                "Services/MonitoringPresets.swift",
                "Services/ProcessHintService.swift",
                "Services/ScanExclusionMatcher.swift",
                "Services/SystemIntegration.swift",
                "Services/UninstallService.swift",
                "Services/WatchSessionModel.swift",
                "ViewModels/AppModel.swift"
            ]
        ),
        .testTarget(
            name: "PathlightCoreTests",
            dependencies: ["PathlightCore"],
            path: "PathlightCoreTests"
        )
    ]
)
