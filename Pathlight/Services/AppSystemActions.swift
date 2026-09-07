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
    var prepareAndOpenFullDiskAccessSettings: () -> Bool
    var fullDiskAccessStatus: @Sendable () async -> FullDiskAccessStatus

    static let live = AppSystemActions(
        reveal: { url in
            SystemIntegration.reveal(url)
        },
        presentFolderPanel: { prompt, message in
            SystemIntegration.presentFolderPanel(prompt: prompt, message: message)
        },
        prepareAndOpenFullDiskAccessSettings: {
            SystemIntegration.prepareAndOpenFullDiskAccessSettings()
        },
        fullDiskAccessStatus: {
            // The probes touch protected paths on disk; keep them off the main actor.
            await Task.detached(priority: .utility) {
                SystemIntegration.fullDiskAccessStatus()
            }.value
        }
    )

    static let inert = AppSystemActions(
        reveal: { _ in },
        presentFolderPanel: { _, _ in nil },
        prepareAndOpenFullDiskAccessSettings: { true },
        fullDiskAccessStatus: { .unknown }
    )
}
