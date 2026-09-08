//
//  AppSystemActions.swift
//  Pathlight
//

import AppKit
import Foundation

/// The few system-facing calls the monitor needs, injectable so tests never
/// touch NSOpenPanel, Finder, or the Full Disk Access probes.
@MainActor
struct AppSystemActions {
    var reveal: (URL) -> Void
    var presentFolderPanel: (_ prompt: String, _ message: String) -> URL?
    var presentSavePanel: (_ suggestedFileName: String) -> URL?
    var prepareAndOpenFullDiskAccessSettings: () -> Bool
    var fullDiskAccessStatus: @Sendable () async -> FullDiskAccessStatus
    /// Removes Pathlight and quits when nothing is left behind. Reports what
    /// could not be removed, so a partial uninstall is said out loud instead of
    /// looking like a silent success.
    var uninstall: (UninstallScope) -> UninstallOutcome

    static let live = AppSystemActions(
        reveal: { url in
            SystemIntegration.reveal(url)
        },
        presentFolderPanel: { prompt, message in
            SystemIntegration.presentFolderPanel(prompt: prompt, message: message)
        },
        presentSavePanel: { suggestedFileName in
            SystemIntegration.presentSavePanel(suggestedFileName: suggestedFileName)
        },
        prepareAndOpenFullDiskAccessSettings: {
            SystemIntegration.prepareAndOpenFullDiskAccessSettings()
        },
        fullDiskAccessStatus: {
            // The probes touch protected paths on disk; keep them off the main actor.
            await Task.detached(priority: .utility) {
                SystemIntegration.fullDiskAccessStatus()
            }.value
        },
        uninstall: { scope in
            let outcome = UninstallService().perform(scope)
            guard outcome == .removed(failures: []) else { return outcome }
            // Everything this process would write on its way out is already
            // gone; a normal terminate would only put some of it back.
            exit(0)
        }
    )

    static let inert = AppSystemActions(
        reveal: { _ in },
        presentFolderPanel: { _, _ in nil },
        presentSavePanel: { _ in nil },
        prepareAndOpenFullDiskAccessSettings: { true },
        fullDiskAccessStatus: { .unknown },
        uninstall: { _ in .removed(failures: []) }
    )
}
