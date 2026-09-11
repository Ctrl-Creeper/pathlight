import SwiftUI

struct PathlightCommands: Commands {
    @ObservedObject var appModel: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Monitor Folder…", systemImage: "folder.badge.plus") {
                appModel.addLongTermWatchFromOpenPanel()
            }
            .keyboardShortcut("o")

            Divider()

            Button("Start Live Monitor…", systemImage: "waveform.path.ecg") {
                if appModel.startShortTermWatchFromOpenPanel() {
                    openWindow(id: "live-monitor")
                }
            }
            .keyboardShortcut("l", modifiers: [.command, .shift])

            Button("Stop Live Monitor", systemImage: "stop.circle") {
                appModel.stopShortTermWatch()
            }
            .keyboardShortcut(".")
            .disabled(appModel.liveWatchSession == nil)

            Divider()

            // One switch for every watch, on a key as well as in the toolbar
            // and the menu bar — the other two hosts spell it Ctrl-. and
            // `pathlight-monitor pause`.
            Button(
                appModel.isMonitoringPaused ? "Resume Monitoring" : "Pause Monitoring",
                systemImage: appModel.isMonitoringPaused ? "play.circle" : "pause.circle"
            ) {
                appModel.setMonitoringPaused(!appModel.isMonitoringPaused)
            }
            .keyboardShortcut("p", modifiers: [.command, .shift])
        }
    }
}
