//
//  UninstallService.swift
//  Pathlight
//

import AppKit
import Foundation
import ServiceManagement
import UserNotifications

/// How much of Pathlight to remove. Reinstalling later and being done with the
/// app are different intents, so what Pathlight recorded is a separate decision
/// from the app itself.
nonisolated enum UninstallScope: Sendable {
    /// Move the app to the Trash. Activity history, settings, and the
    /// encryption key stay, so a reinstall picks up where this left off.
    case appOnly
    /// The app plus everything it ever created on this Mac.
    case everything
}

/// What an uninstall attempt actually did. `[String]` cannot express this, and
/// the caller needs it: an install that is still intact should go back to
/// monitoring, while a half-removed one must stay stopped so nothing rewrites
/// the journal that was just deleted.
nonisolated enum UninstallOutcome: Sendable, Equatable {
    /// Nothing was removed. Pathlight is still installed and still usable.
    case installIntact(failures: [String])
    /// The app is in the Trash. `failures` names what was left behind.
    case removed(failures: [String])
}

/// Removes what Pathlight put on this Mac.
///
/// Deliberately narrow: every path is derived from the bundle identifier or the
/// journal location, never from a watch target, so a folder the user asked
/// Pathlight to monitor can never reach the delete list.
nonisolated struct UninstallService {
    var home: URL = FileManager.default.homeDirectoryForCurrentUser
    var bundleID: String = Bundle.main.bundleIdentifier ?? "com.ctrlcreeper.Pathlight"
    var appBundleURL: URL? = Bundle.main.bundleURL
    var dataDirectory: URL = JSONLActivityEventStore.defaultJournalURL().deletingLastPathComponent()

    /// Everything Pathlight wrote outside the folders the user chose to watch.
    func removableDataItems() -> [URL] {
        let library = home.appending(path: "Library", directoryHint: .isDirectory)
        return [
            dataDirectory,
            library.appending(path: "Preferences/\(bundleID).plist"),
            library.appending(path: "Caches/\(bundleID)", directoryHint: .isDirectory),
            library.appending(path: "HTTPStorages/\(bundleID)", directoryHint: .isDirectory),
            library.appending(path: "HTTPStorages/\(bundleID).binarycookies"),
            library.appending(
                path: "Saved Application State/\(bundleID).savedState",
                directoryHint: .isDirectory
            ),
            library.appending(path: "WebKit/\(bundleID)", directoryHint: .isDirectory)
        ]
    }

    /// Performs the uninstall and reports what could not be removed, in plain
    /// language. `.removed(failures: [])` means nothing of `scope` is left.
    ///
    /// The app bundle goes first: it is the step that can fail on permissions,
    /// and failing it before anything is deleted leaves the install intact.
    func perform(_ scope: UninstallScope) -> UninstallOutcome {
        var failures: [String] = []

        // An app in the Trash must not still be a login item, whatever the scope.
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            failures.append("the login item")
        }

        if let appBundleURL, !moveToTrash(appBundleURL) {
            failures.append("the Pathlight app itself")
            return .installIntact(failures: failures)
        }

        guard scope == .everything else {
            return .removed(failures: failures)
        }

        let center = UNUserNotificationCenter.current()
        center.removeAllDeliveredNotifications()
        center.removeAllPendingNotificationRequests()

        // Drop the domain before deleting its file, otherwise the preferences
        // daemon writes its cached copy back out after the file is gone.
        UserDefaults.standard.removePersistentDomain(forName: bundleID)
        UserDefaults.standard.synchronize()

        let fileManager = FileManager.default
        for item in removableDataItems() where fileManager.fileExists(atPath: item.path) {
            do {
                try fileManager.removeItem(at: item)
            } catch {
                failures.append(item.lastPathComponent)
            }
        }

        // The key goes last. An encrypted journal that could not be deleted is
        // still the user's history; dropping its key first would leave them
        // with a file nothing on earth can read.
        do {
            try KeychainActivityStorageKeyProvider.live.deleteKey()
        } catch {
            failures.append("the activity encryption key in Keychain")
        }

        return .removed(failures: failures)
    }

    /// ponytail: the user's own Trash only. An app installed by another admin
    /// account fails here and is reported, rather than asking for credentials.
    private func moveToTrash(_ url: URL) -> Bool {
        (try? FileManager.default.trashItem(at: url, resultingItemURL: nil)) != nil
    }
}
