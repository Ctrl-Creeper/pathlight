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

/// One spelling for a pattern however it was typed, blanks and duplicates
/// dropped. The core owns the rule, since it applies it again when it compiles
/// the list; the app target injects `normalizedExclusionPatterns`.
typealias ActivityExclusionNormalizing = @Sendable (_ patterns: [String]) -> [String]

/// What a new watch starts with.
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
}
