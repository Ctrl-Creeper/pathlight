//
//  PathlightApp.swift
//  Pathlight
//
//  Created by Colin Kim on 4/1/26.
//

import AppKit
import Combine
import Sparkle
import SwiftUI

// This view model class publishes when new updates can be checked by the user
@MainActor
final class CheckForUpdatesViewModel: ObservableObject {
    @Published var canCheckForUpdates = false

    init(updater: SPUUpdater) {
        updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .assign(to: &$canCheckForUpdates)
    }
}

// This is the view for the Check for Updates menu item
// Note this intermediate view is necessary for the disabled state on the menu item to work properly before Monterey.
// See https://stackoverflow.com/questions/68553092/menu-not-updating-swiftui-bug for more info
struct CheckForUpdatesView: View {
    @StateObject private var checkForUpdatesViewModel: CheckForUpdatesViewModel
    private let updater: SPUUpdater
    
    init(updater: SPUUpdater) {
        self.updater = updater
        
        // Create our view model for our CheckForUpdatesView
        _checkForUpdatesViewModel = StateObject(wrappedValue: CheckForUpdatesViewModel(updater: updater))
    }
    
    var body: some View {
        Button("Check for Updates…", systemImage: "arrow.triangle.2.circlepath") {
            updater.checkForUpdates()
        }
            .disabled(!checkForUpdatesViewModel.canCheckForUpdates)
    }
}

@main
struct PathlightApp: App {
    @NSApplicationDelegateAdaptor(PathlightAppDelegate.self) private var appDelegate
    @StateObject private var appModel = AppModel()
    private let updaterController: SPUStandardUpdaterController
    private let issueReportURL = URL(string: "https://github.com/Ctrl-Creeper/pathlight/issues/new/choose")

    init() {
        NSWindow.allowsAutomaticWindowTabbing = false
        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
    }

    var body: some Scene {
        Window("Pathlight", id: "main") {
            ContentView()
                .environmentObject(appModel)
                .frame(minWidth: 820, maxWidth: .infinity, minHeight: 540, maxHeight: .infinity)
        }
        .defaultSize(width: 1120, height: 720)
        .windowResizability(.contentMinSize)
        .commands {
            PathlightCommands(appModel: appModel)

            CommandGroup(after: .appInfo) {
                CheckForUpdatesView(updater: updaterController.updater)
            }

            CommandGroup(after: .help) {
                Button("Report Issue…", systemImage: "flag") {
                    if let issueReportURL {
                        NSWorkspace.shared.open(issueReportURL)
                    }
                }
            }
        }

        // Menu bar presence only while folders are actually monitored.
        MenuBarExtra(
            "Pathlight Activity",
            systemImage: "waveform.path.ecg",
            isInserted: Binding(
                get: { !appModel.longTermWatchTargets.isEmpty },
                set: { _ in }
            )
        ) {
            ActivityMenuBarView()
                .environmentObject(appModel)
        }

        Window("Live Monitor", id: "live-monitor") {
            LiveMonitorWindowView()
                .environmentObject(appModel)
        }
        .defaultSize(width: 460, height: 560)
        .windowResizability(.contentMinSize)

        Settings {
            SettingsView()
                .environmentObject(appModel)
        }
    }
}
