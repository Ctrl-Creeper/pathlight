import Foundation
import PathlightRustCore

/// Noise filtering backed by the Rust core, so this app and the Windows and
/// Linux window agree about which paths are not worth recording — and a
/// journal both of them read holds one answer rather than two.
nonisolated final class RustActivityExclusion: ActivityExcluding, @unchecked Sendable {
    private let matcher: ExclusionMatcher

    /// nil when the patterns normalize to nothing, or when one of them is not
    /// a pattern the core can compile: filtering nothing is the honest answer
    /// to a rule nobody can apply, and the watch still records.
    init?(patterns: [String], rootPath: URL) {
        guard let matcher = try? ExclusionMatcher(
            patterns: patterns,
            root: rootPath.standardizedFileURL.path
        ), !matcher.isEmpty() else {
            return nil
        }
        self.matcher = matcher
    }

    func excludes(_ url: URL) -> Bool {
        matcher.excludes(path: url.standardizedFileURL.path)
    }
}
