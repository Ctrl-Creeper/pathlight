import Foundation
import Testing

@testable import PathlightCore

@Suite("Uninstall service")
struct UninstallServiceTests {
    private func makeService(home: URL) -> UninstallService {
        UninstallService(
            home: home,
            bundleID: "com.example.Pathlight",
            appBundleURL: nil,
            dataDirectory: home.appending(
                path: "Library/Application Support/Pathlight",
                directoryHint: .isDirectory
            )
        )
    }

    @Test("removes the journal, the preferences, and the caches it created")
    func coversEverythingPathlightWrote() {
        let home = URL(filePath: "/Users/tester")
        let paths = makeService(home: home).removableDataItems().map(\.path)

        #expect(paths.contains("/Users/tester/Library/Application Support/Pathlight"))
        #expect(paths.contains("/Users/tester/Library/Preferences/com.example.Pathlight.plist"))
        #expect(paths.contains("/Users/tester/Library/Caches/com.example.Pathlight"))
    }

    /// The one guarantee the uninstall dialog makes that a user cannot verify
    /// before agreeing to it: monitored folders are never deleted.
    @Test("never proposes deleting anything outside Pathlight's own storage")
    func staysOutOfUserFolders() {
        let home = URL(filePath: "/Users/tester")
        let service = makeService(home: home)

        for item in service.removableDataItems() {
            let path = item.standardizedFileURL.path
            #expect(
                path == service.dataDirectory.standardizedFileURL.path
                    || path.hasPrefix("/Users/tester/Library/"),
                "\(path) is outside Pathlight's own storage"
            )
            #expect(path.contains("Pathlight") || path.contains("com.example.Pathlight"))
        }
    }

    /// The app bundle goes first precisely so this case exists: when it cannot
    /// be trashed, the user still has a working Pathlight, and every recording
    /// they own has to still be there.
    @Test("a bundle it cannot trash leaves the install and the data untouched")
    func reportsAnIntactInstallWithoutDeletingAnything() throws {
        let home = URL(
            filePath: "/tmp/pathlight-uninstall-\(UUID().uuidString)",
            directoryHint: .isDirectory
        )
        let dataDirectory = home.appending(
            path: "Library/Application Support/Pathlight",
            directoryHint: .isDirectory
        )
        try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let journal = dataDirectory.appending(path: "activity-events.jsonl")
        try Data("row".utf8).write(to: journal)
        defer { try? FileManager.default.removeItem(at: home) }

        var service = makeService(home: home)
        // A path that is not there cannot be trashed, which is the same answer
        // the real failure gives: an app another admin account installed.
        service.appBundleURL = home.appending(path: "Pathlight.app", directoryHint: .isDirectory)

        let outcome = service.perform(.everything)

        #expect(outcome == .installIntact(failures: ["the Pathlight app itself"]))
        #expect(
            FileManager.default.fileExists(atPath: journal.path),
            "nothing may be deleted until the app itself is gone"
        )
    }
}
