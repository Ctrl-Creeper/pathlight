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
        Text(presentation.summaryText)

        Divider()

        ForEach(presentation.rows) { row in
            Button {
                openDashboard()
            } label: {
                Text("\(row.title)  •  \(row.statusText)  •  \(row.todayChangeText)")
            }
        }

        Divider()

        Button("Open Activity Dashboard") {
            openDashboard()
        }
    }

    private func openDashboard() {
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
        appModel.selectSidebarTarget(id: SidebarModel.activityDashboardID)
    }
}
