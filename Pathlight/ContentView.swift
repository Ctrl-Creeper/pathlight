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
            showsLaunchAtLoginNudge: appModel.shouldShowLaunchAtLoginNudge,
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
            setLongTermWatchEnabled: { appModel.setLongTermWatchEnabled($0, rootPath: $1) },
            removeLongTermWatchTarget: { appModel.removeLongTermWatchTarget(rootPath: $0) },
            revealInFinder: { appModel.revealURLInFinder($0) },
            clearHistoryGap: { appModel.clearLongTermWatchHistoryGap(rootPath: $0) },
            setGrowthAlertThreshold: { appModel.setGrowthAlertThreshold($0, rootPath: $1) },
            setExclusionPatterns: { appModel.setExclusionPatterns($0, rootPath: $1) },
            enableLaunchAtLogin: { appModel.setLaunchAtLoginEnabled(true) },
            dismissLaunchAtLoginNudge: { appModel.dismissLaunchAtLoginNudge() },
            addFolder: { appModel.addLongTermWatchFromOpenPanel() },
            startLiveMonitor: { options in
                if appModel.startShortTermWatchFromOpenPanel(options: options) {
                    openWindow(id: "live-monitor")
                }
            },
            stopLiveMonitor: { appModel.stopShortTermWatch() }
        )
    }
}
