import Foundation

/// Filters raw monitoring paths against gitignore-style patterns before
/// attribution, so noisy churn (caches, `.DS_Store`, temp files) never reaches
/// the activity journal. An empty pattern list means "record everything".
nonisolated struct ActivityExclusionFilter: Sendable {
    /// Suggested starting point for long-term watches. Users can edit or clear
    /// the list per target; nothing here is enforced.
    static let defaultPatterns: [String] = [
        ".DS_Store",
        ".Trash/",
        ".Trashes/",
        ".fseventsd/",
        ".Spotlight-V100/",
        ".TemporaryItems/",
        ".DocumentRevisions-V100/",
        "**/Caches/",
        "*.tmp",
        "*.part",
        "*.crdownload",
        "*.download"
    ]

    private let matcher: ScanExclusionMatcher
    private let rootPath: String

    /// Returns nil when the pattern list normalizes to empty, so callers can
    /// skip filtering entirely.
    init?(patterns: [String], rootPath: URL) {
        let normalizedPatterns = ScanExclusionMatcher.normalizedPatterns(patterns)
        guard !normalizedPatterns.isEmpty else {
            return nil
        }

        let standardizedRoot = rootPath.standardizedFileURL
        self.rootPath = standardizedRoot.path
        // includeCloudStorage: true — a monitoring root is an explicit user
        // choice, so the scanner's implicit cloud-folder exclusion must not apply.
        self.matcher = ScanExclusionMatcher(
            patterns: normalizedPatterns,
            rootURL: standardizedRoot,
            includeCloudStorage: true
        )
    }

    func excludes(_ url: URL) -> Bool {
        let path = url.standardizedFileURL

        // Deleted paths can no longer be stat'ed, so the leaf is matched both
        // as a file and as a directory; over-matching is acceptable for noise
        // filtering and keeps the filter free of filesystem calls.
        if matcher.excludes(path, isDirectory: false) || matcher.excludes(path, isDirectory: true) {
            return true
        }

        // Events arrive for deep descendants, but patterns describe the folders
        // being ignored — check every ancestor directory up to the watch root.
        var ancestor = path.deletingLastPathComponent()
        while ancestor.path != rootPath, ancestor.path.hasPrefix(rootPath) {
            if matcher.excludes(ancestor, isDirectory: true) {
                return true
            }
            let parent = ancestor.deletingLastPathComponent()
            guard parent.path != ancestor.path else {
                break
            }
            ancestor = parent
        }

        return false
    }
}
