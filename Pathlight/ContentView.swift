//
//  ContentView.swift
//  Pathlight
//

import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var appModel: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        ActivityDashboardView(
            targets: appModel.longTermWatchTargets,
            histories: Array(appModel.activityDashboardHistories.values),
            runtimeStatuses: appModel.longTermWatchRuntimeStatuses,
            historyQuery: appModel.activityHistoryQuery,
            showsLaunchAtLoginNudge: appModel.shouldShowLaunchAtLoginNudge,
            monitoringStatusMessage: appModel.monitoringStatusMessage,
            isMonitoringPaused: appModel.isMonitoringPaused,
            isLiveMonitorActive: appModel.liveWatchSession != nil,
            actions: dashboardActions
        )
        .alert(
            "Something Went Wrong",
            isPresented: Binding(
                get: { appModel.lastErrorMessage != nil },
                set: { isPresented in
                    if !isPresented {
                        appModel.lastErrorMessage = nil
                    }
                }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(appModel.lastErrorMessage ?? "Unknown error")
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active {
                appModel.refreshFullDiskAccessStatus()
                appModel.refreshLaunchAtLoginStatus()
            }
        }
    }

    private var dashboardActions: ActivityDashboardActions {
        ActivityDashboardActions(
            refreshActivityHistory: { appModel.refreshActivityHistory(rootPath: $0) },
            refreshActivityDashboardHistories: { appModel.refreshActivityDashboardHistories(rootPaths: $0) },
            narrowActivityHistory: { appModel.narrowActivityHistory($0, rootPath: $1) },
            setLongTermWatchEnabled: { appModel.setLongTermWatchEnabled($0, rootPath: $1) },
            removeLongTermWatchTarget: { appModel.removeLongTermWatchTarget(rootPath: $0) },
            revealInFinder: { appModel.revealURLInFinder($0) },
            clearHistoryGap: { appModel.clearLongTermWatchHistoryGap(rootPath: $0) },
            setGrowthAlertThreshold: { appModel.setGrowthAlertThreshold($0, rootPath: $1) },
            setRecordingFilters: { patterns, minimumBytes, maximumBytes, rootPath in
                appModel.setRecordingFilters(
                    patterns: patterns,
                    minimumFileBytes: minimumBytes,
                    maximumFileBytes: maximumBytes,
                    rootPath: rootPath
                )
            },
            enableLaunchAtLogin: { appModel.setLaunchAtLoginEnabled(true) },
            dismissLaunchAtLoginNudge: { appModel.dismissLaunchAtLoginNudge() },
            addFolder: { appModel.addLongTermWatchFromOpenPanel(configuration: $0) },
            addPreset: { appModel.addPreset($0, configuration: $1) },
            exportHistory: { appModel.exportHistoryCSV(rootPath: $0) },
            startLiveMonitor: { configuration in
                if appModel.startShortTermWatchFromOpenPanel(configuration: configuration) {
                    openWindow(id: "live-monitor")
                }
            },
            stopLiveMonitor: { appModel.stopShortTermWatch() },
            setMonitoringPaused: { appModel.setMonitoringPaused($0) }
        )
    }
}
