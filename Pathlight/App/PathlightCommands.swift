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

            // Every row's switch at once, on a key as well as in the toolbar
            // and the menu bar — `gui/` spells it Ctrl-Shift-.
            Button("Stop All Monitoring", systemImage: "stop.circle.fill") {
                appModel.stopAllMonitoring()
            }
            .keyboardShortcut(".", modifiers: [.command, .shift])
        }
    }
}
