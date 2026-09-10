import AppKit
import SwiftUI

struct ActivityMenuBarView: View {
    @EnvironmentObject private var appModel: AppModel
    @Environment(\.openWindow) private var openWindow

    private var presentation: MenuBarActivityPresentation {
        MenuBarActivityPresentation(
            targets: appModel.longTermWatchTargets,
            histories: Array(appModel.activityDashboardHistories.values),
            runtimeStatuses: appModel.longTermWatchRuntimeStatuses
        )
    }

    var body: some View {
        Text(appModel.isMonitoringPaused
            ? "Paused — nothing is being recorded"
            : presentation.summaryText)

        Divider()

        ForEach(presentation.rows) { row in
            Button {
                openDashboard()
            } label: {
                Text("\(row.title)  •  \(row.statusText)  •  \(row.todayChangeText)")
            }
        }

        Divider()

        // Reachable with the window closed, which is when a pause is most
        // likely wanted: the watches run either way.
        Button(appModel.isMonitoringPaused ? "Resume Monitoring" : "Pause Monitoring") {
            appModel.setMonitoringPaused(!appModel.isMonitoringPaused)
        }

        Button("Open Activity Dashboard") {
            openDashboard()
        }
    }

    private func openDashboard() {
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
    }
}
