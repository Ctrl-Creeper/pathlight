import Foundation

nonisolated struct ActivityNoiseSuggestion: Equatable, Sendable {
    let directory: URL
    /// gitignore-style pattern relative to the watch root, ready for the exclusion list.
    let pattern: String
    let eventCount: Int
    let shareOfTotal: Double
}

/// Spots one directory that dominates a session's events so the user can
/// exclude it instead of drowning in build or sync churn.
enum ActivityNoiseAdvisor {
    nonisolated static func suggestion(
        for events: [DiskActivityEvent],
        rootPath: URL,
        minimumEvents: Int = 200,
        minimumShare: Double = 0.4
    ) -> ActivityNoiseSuggestion? {
        guard events.count >= minimumEvents else {
            return nil
        }
        let rootPrefix = rootPath.standardizedFileURL.path + "/"
        var counts: [String: Int] = [:]
        for event in events {
            let parent = event.path.standardizedFileURL.deletingLastPathComponent().path
            guard parent.hasPrefix(rootPrefix) else {
                continue
            }
            counts[parent, default: 0] += 1
        }
        guard let (directory, count) = counts.max(by: { $0.value < $1.value }),
              count >= minimumEvents else {
            return nil
        }
        let share = Double(count) / Double(events.count)
        guard share >= minimumShare else {
            return nil
        }
        return ActivityNoiseSuggestion(
            directory: URL(filePath: directory, directoryHint: .isDirectory),
            pattern: String(directory.dropFirst(rootPrefix.count)) + "/",
            eventCount: count,
            shareOfTotal: share
        )
    }
}
