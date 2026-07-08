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
                "Models/FileNodeActions.swift",
                "Models/FileNodeRecord.swift",
                "Models/FileTreeStore.swift",
                "Models/ScanProgress.swift",
                "Models/ScanSnapshot.swift",
                "Models/ScanTarget.swift",
                "Models/TrashSafetyPolicy.swift",
                "Services/AtomicDirectoryParallelSummary.swift",
                "Services/AtomicDirectorySummaryProbe.swift",
                "Services/AtomicDirectorySummaryWalker.swift",
                "Services/AtomicDirectorySummarizer.swift",
                "Services/AtomicDirectorySummaryModels.swift",
                "Services/ActivityEventStore.swift",
                "Services/ActivityHistoryService.swift",
                "Services/ActivityHistoryPresentation.swift",
                "Services/ActivityTimelinePresentation.swift",
                "Services/DiskActivityMonitor.swift",
                "Services/DiskActivityModels.swift",
                "Services/AppDependencies.swift",
                "Services/AppPreferencesStore.swift",
                "Services/AppSystemActions.swift",
                "Services/AppUsageStatsStore.swift",
                "Services/FileBrowserDisplayState.swift",
                "Services/FileBrowserModel.swift",
                "Services/FileBrowserResults.swift",
                "Services/FileBrowserSearch.swift",
                "Services/FileBrowserSorting.swift",
                "Services/FileSizeFormatter.swift",
                "Services/HardLinkDeduplicator.swift",
                "Services/LiveWatchSessionCoordinator.swift",
                "Services/QuickLookIntegration.swift",
                "Services/RecentTargetStore.swift",
                "Services/ScanArchiveModels.swift",
                "Services/ScanArchiveNodeIO.swift",
                "Services/ScanArchiveProgressReporting.swift",
                "Services/ScanArchiveService.swift",
                "Services/ScanArchiveTopologyValidator.swift",
                "Services/StorageAttributionService.swift",
                "Services/ScanDiagnostics.swift",
                "Services/ScanCoordinator.swift",
                "Services/ScanEngine.swift",
                "Services/ScanExclusionMatcher.swift",
                "Services/ScanMetadataLoader.swift",
                "Services/ScanSnapshotTransformService.swift",
                "Services/ScanWarningFactory.swift",
                "Services/SunburstChartModel.swift",
                "Services/SunburstColorResolver.swift",
                "Services/SunburstFreeSpaceVisualization.swift",
                "Services/SunburstGeometry.swift",
                "Services/SunburstVisualizationFilterModel.swift",
                "Services/SunburstViewportTransform.swift",
                "Services/SystemIntegration.swift",
                "Services/WatchSessionModel.swift",
                "ViewModels/AppQuickLookController.swift",
                "ViewModels/AppModel.swift",
                "ViewModels/SidebarScanCacheController.swift",
                "ViewModels/SidebarModel.swift",
                "ViewModels/WorkspaceNavigationModel.swift"
            ]
        ),
        .testTarget(
            name: "PathlightCoreTests",
            dependencies: ["PathlightCore"],
            path: "PathlightCoreTests"
        )
    ]
)
