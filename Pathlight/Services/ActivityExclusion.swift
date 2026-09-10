import Foundation

/// Filters raw monitoring paths against gitignore-style patterns before
/// attribution, so noisy churn (caches, `.DS_Store`, temp files) never reaches
/// the activity journal. An empty pattern list means "record everything".
///
/// Declared here and implemented in the Rust core, which every host shares —
/// two implementations of "is this path noise" drift, and the journal both
/// hosts read would then hold two answers about what was worth recording. This
/// package does not link the core, so `swift test` needs no xcframework; the
/// app target injects `RustActivityExclusion`, the way it already injects
/// attribution and its monitor.
nonisolated protocol ActivityExcluding: Sendable {
    /// Whether this path is noise. A watch asks about paths under its root
    /// only; anything outside is never excluded.
    func excludes(_ url: URL) -> Bool
}

/// Builds the filter for one watch, or nil when the patterns normalize to
/// nothing — a caller that gets nil can skip filtering entirely rather than
/// ask about every path.
typealias ActivityExclusionFactory = @Sendable (
    _ patterns: [String],
    _ rootPath: URL
) -> (any ActivityExcluding)?

/// The pattern list itself: what a new watch starts with, and how a typed
/// pattern is stored.
// `nonisolated`, like the rules it describes: a pattern list is text, and the
// watch that asks about it runs off the main actor.
nonisolated enum ActivityExclusionPatterns {
    /// Suggested starting point for long-term watches. Users can edit or clear
    /// the list per target; nothing here is enforced.
    ///
    /// Kept in step with the core's `exclusion::DEFAULT_PATTERNS` by hand: a
    /// default list is text this package shows before any watch exists, and
    /// the core it would have to ask is not linked here.
    static let defaults: [String] = [
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

    /// One spelling for a pattern however it was typed, blanks dropped and
    /// duplicates removed while the order the user wrote them in is kept.
    ///
    /// This is about what gets stored and shown, not about what matches: the
    /// core applies the same rules again when it compiles the list, so a
    /// pattern saved by an older build still means what it says.
    static func normalized(_ patterns: [String]) -> [String] {
        var normalizedPatterns: [String] = []
        for pattern in patterns {
            guard let normalizedPattern = normalized(pattern: pattern),
                  !normalizedPatterns.contains(normalizedPattern) else {
                continue
            }
            normalizedPatterns.append(normalizedPattern)
        }
        return normalizedPatterns
    }

    /// nil for a pattern that is nothing but separators and space.
    private static func normalized(pattern: String) -> String? {
        var text = pattern
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "\\", with: "/")

        while text.hasPrefix("./") {
            text.removeFirst(2)
        }
        while text.hasPrefix("/") {
            text.removeFirst()
        }
        while text.contains("//") {
            text = text.replacingOccurrences(of: "//", with: "/")
        }

        let directoryOnly = text.hasSuffix("/")
        while text.hasSuffix("/") {
            text.removeLast()
        }

        guard !text.isEmpty else { return nil }
        return directoryOnly ? "\(text)/" : text
    }
}
