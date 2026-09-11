import AppKit
import UserNotifications

/// Keeps monitoring alive: offers "keep running in the menu bar" instead of a
/// silent quit, and drops the Dock icon while only the menu bar item remains.
final class PathlightAppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private enum DefaultsKey {
        static let suppressQuitConfirmation = "suppressMonitoringQuitConfirmation"
    }

    private var windowWillCloseObserver: (any NSObjectProtocol)?
    private var windowDidBecomeKeyObserver: (any NSObjectProtocol)?

    func applicationDidFinishLaunching(_ notification: Notification) {
        UNUserNotificationCenter.current().delegate = self
        let center = NotificationCenter.default
        windowWillCloseObserver = center.addObserver(
            forName: NSWindow.willCloseNotification,
            object: nil,
            queue: .main
        ) { notification in
            let closingWindow = notification.object as? NSWindow
            // The window is still visible while the notification fires, so
            // re-evaluate after the close completes.
            DispatchQueue.main.async {
                Self.dropToAccessoryIfBackgroundOnly(excluding: closingWindow)
            }
        }
        windowDidBecomeKeyObserver = center.addObserver(
            forName: NSWindow.didBecomeKeyNotification,
            object: nil,
            queue: .main
        ) { notification in
            guard let window = notification.object as? NSWindow else { return }
            DispatchQueue.main.async {
                guard Self.isRegularWindow(window),
                      NSApp.activationPolicy() != .regular else {
                    return
                }
                NSApp.setActivationPolicy(.regular)
                NSApp.activate()
            }
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard hasEnabledWatchTargets,
              !UserDefaults.standard.bool(forKey: DefaultsKey.suppressQuitConfirmation) else {
            return .terminateNow
        }

        let alert = NSAlert()
        alert.messageText = String(localized: "Quit Pathlight and stop monitoring?")
        alert.informativeText = String(
            localized: "Folder monitoring only runs while Pathlight is open. You can keep it running in the menu bar instead."
        )
        alert.addButton(withTitle: String(localized: "Keep Running in Menu Bar"))
        alert.addButton(withTitle: String(localized: "Quit"))
        alert.showsSuppressionButton = true
        alert.suppressionButton?.title = String(localized: "Don't ask again")

        let response = alert.runModal()
        if response == .alertFirstButtonReturn {
            for window in NSApp.windows where Self.isRegularWindow(window) {
                window.close()
            }
            NSApp.setActivationPolicy(.accessory)
            return .terminateCancel
        }

        if alert.suppressionButton?.state == .on {
            UserDefaults.standard.set(true, forKey: DefaultsKey.suppressQuitConfirmation)
        }
        return .terminateNow
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        // Without a delegate, an alert about a folder filling up is swallowed
        // whenever Pathlight itself happens to be the frontmost app.
        completionHandler([.banner, .list])
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        // Answer the system first; the closure is not Sendable, so it cannot
        // cross into the main-actor hop that raises the window.
        completionHandler()
        Task { @MainActor in
            Self.showDashboard()
        }
    }

    /// Brings the dashboard back after a notification tap. SwiftUI keeps the
    /// singleton `main` window alive across a close, so ordering it front is
    /// enough — `openWindow` is not reachable from an AppKit delegate.
    @MainActor
    private static func showDashboard() {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        let regular = NSApp.windows.filter(isRegularWindow)
        let window = regular.first { $0.identifier?.rawValue.contains("main") == true } ?? regular.first
        window?.makeKeyAndOrderFront(nil)
    }

    private var hasEnabledWatchTargets: Bool {
        UserDefaultsLongTermWatchTargetPersistence()
            .loadTargets()
            .contains(where: \.isEnabled)
    }

    private static func dropToAccessoryIfBackgroundOnly(excluding closingWindow: NSWindow?) {
        let hasVisibleRegularWindow = NSApp.windows.contains { window in
            window !== closingWindow && window.isVisible && isRegularWindow(window)
        }
        guard !hasVisibleRegularWindow,
              NSApp.activationPolicy() == .regular,
              UserDefaultsLongTermWatchTargetPersistence()
                  .loadTargets()
                  .contains(where: \.isEnabled) else {
            return
        }
        NSApp.setActivationPolicy(.accessory)
    }

    /// Real user-facing windows; status bar items, popovers, and other
    /// borderless panels don't count toward "the app is on screen".
    private static func isRegularWindow(_ window: NSWindow) -> Bool {
        window.styleMask.contains(.titled) && !(window is NSPanel)
    }
}
