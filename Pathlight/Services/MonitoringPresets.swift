import Foundation

/// One-click long-term watch targets with exclusions tuned for their churn.
nonisolated struct MonitoringPreset: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let rootPath: URL
    let extraExclusionPatterns: [String]

    var options: LongTermWatchTargetOptions {
        var options = LongTermWatchTargetOptions.default
        options.exclusionPatterns = ScanExclusionMatcher.normalizedPatterns(
            ActivityExclusionFilter.defaultPatterns + extraExclusionPatterns
        )
        return options
    }

    /// Presets whose folder exists on this Mac.
    nonisolated static func available(
        home: URL = FileManager.default.homeDirectoryForCurrentUser,
        isDirectory: (URL) -> Bool = Self.liveIsDirectory
    ) -> [MonitoringPreset] {
        all(home: home).filter { isDirectory($0.rootPath) }
    }

    nonisolated static func all(home: URL) -> [MonitoringPreset] {
        let library = home.appending(path: "Library", directoryHint: .isDirectory)
        return [
            MonitoringPreset(id: "downloads", title: "Downloads", rootPath: home.appending(path: "Downloads", directoryHint: .isDirectory), extraExclusionPatterns: []),
            MonitoringPreset(id: "desktop", title: "Desktop", rootPath: home.appending(path: "Desktop", directoryHint: .isDirectory), extraExclusionPatterns: []),
            MonitoringPreset(id: "documents", title: "Documents", rootPath: home.appending(path: "Documents", directoryHint: .isDirectory), extraExclusionPatterns: []),
            MonitoringPreset(id: "library", title: "Library", rootPath: library, extraExclusionPatterns: ["Logs/", "Saved Application State/", "**/*.sqlite-wal", "**/*.sqlite-shm"]),
            MonitoringPreset(id: "xcode-derived-data", title: "Xcode DerivedData", rootPath: library.appending(path: "Developer/Xcode/DerivedData", directoryHint: .isDirectory), extraExclusionPatterns: ["**/Index.noindex/"]),
            MonitoringPreset(id: "simulators", title: "iOS Simulators", rootPath: library.appending(path: "Developer/CoreSimulator/Devices", directoryHint: .isDirectory), extraExclusionPatterns: []),
            MonitoringPreset(id: "docker", title: "Docker Data", rootPath: library.appending(path: "Containers/com.docker.docker/Data", directoryHint: .isDirectory), extraExclusionPatterns: []),
            MonitoringPreset(id: "homebrew", title: "Homebrew", rootPath: URL(filePath: "/opt/homebrew", directoryHint: .isDirectory), extraExclusionPatterns: ["**/.git/"]),
            MonitoringPreset(id: "applications", title: "Applications", rootPath: URL(filePath: "/Applications", directoryHint: .isDirectory), extraExclusionPatterns: [])
        ]
    }

    private nonisolated static func liveIsDirectory(_ url: URL) -> Bool {
        (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
    }
}

/// Where did a deleted file go? Only the user's Trash is checked.
// ponytail: name match in ~/.Trash only; volume .Trashes and Time Machine lookups later.
enum TrashLocator {
    nonisolated static func contains(itemNamed name: String) -> Bool {
        let trash = FileManager.default.homeDirectoryForCurrentUser.appending(path: ".Trash", directoryHint: .isDirectory)
        return FileManager.default.fileExists(atPath: trash.appending(path: name).path)
    }
}
