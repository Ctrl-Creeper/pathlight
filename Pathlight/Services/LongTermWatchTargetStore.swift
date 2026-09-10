import Foundation

nonisolated struct LongTermWatchTargetOptions: Codable, Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let recordsFileNames: Bool
    /// Daily net growth that triggers a notification; nil means alerts are off.
    var growthAlertThresholdBytes: Int64?
    /// Gitignore-style patterns filtered out of this target's event stream.
    /// Empty means record everything.
    var exclusionPatterns: [String]

    init(
        minimumRecordedByteDelta: Int64,
        aggregationWindow: TimeInterval,
        recordsFileNames: Bool,
        growthAlertThresholdBytes: Int64? = nil,
        exclusionPatterns: [String] = ActivityExclusionPatterns.defaults
    ) {
        self.minimumRecordedByteDelta = minimumRecordedByteDelta
        self.aggregationWindow = aggregationWindow
        self.recordsFileNames = recordsFileNames
        self.growthAlertThresholdBytes = growthAlertThresholdBytes
        self.exclusionPatterns = exclusionPatterns
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        minimumRecordedByteDelta = try container.decode(Int64.self, forKey: .minimumRecordedByteDelta)
        aggregationWindow = try container.decode(TimeInterval.self, forKey: .aggregationWindow)
        recordsFileNames = try container.decode(Bool.self, forKey: .recordsFileNames)
        growthAlertThresholdBytes = try container.decodeIfPresent(Int64.self, forKey: .growthAlertThresholdBytes)
        // Targets persisted before exclusions existed adopt the defaults.
        exclusionPatterns = try container.decodeIfPresent([String].self, forKey: .exclusionPatterns)
            ?? ActivityExclusionPatterns.defaults
    }

    static let `default` = LongTermWatchTargetOptions(
        minimumRecordedByteDelta: DiskActivityAggregationOptions.default.minimumRecordedByteDelta,
        aggregationWindow: DiskActivityAggregationOptions.default.aggregationWindow,
        recordsFileNames: DiskActivityAggregationOptions.default.longTermRecordsFileNames
    )

    var diskActivityOptions: DiskActivityAggregationOptions {
        DiskActivityAggregationOptions(
            minimumRecordedByteDelta: minimumRecordedByteDelta,
            aggregationWindow: aggregationWindow,
            longTermRecordsFileNames: recordsFileNames
        )
    }
}

nonisolated struct LongTermWatchTarget: Codable, Equatable, Identifiable, Sendable {
    let id: String
    let rootPath: URL
    var isEnabled: Bool
    var options: LongTermWatchTargetOptions
    var baseline: ActivityBaselineSnapshot?
    var checkpoint: LongTermWatchCheckpoint?

    init(
        rootPath: URL,
        isEnabled: Bool = true,
        options: LongTermWatchTargetOptions = .default,
        baseline: ActivityBaselineSnapshot? = nil,
        checkpoint: LongTermWatchCheckpoint? = nil
    ) {
        let standardizedRoot = rootPath.standardizedFileURL
        self.id = standardizedRoot.path
        self.rootPath = standardizedRoot
        self.isEnabled = isEnabled
        self.options = options
        self.baseline = baseline
        self.checkpoint = checkpoint
    }
}

nonisolated struct LongTermWatchCheckpoint: Codable, Equatable, Sendable {
    let eventID: UInt64
    let recordedAt: Date
    let hasHistoryGap: Bool
}

nonisolated enum LongTermWatchRuntimeState: Equatable, Sendable {
    case starting
    case watching
    case reconnecting
    case catchingUp
    case historyGap
    /// The watched folder or volume is no longer there, so nothing can be
    /// observed until it comes back.
    case rootMissing
    case paused
}

nonisolated struct LongTermWatchRuntimeStatus: Equatable, Sendable {
    let state: LongTermWatchRuntimeState
    let lastActivityAt: Date?
    let retryCount: Int

    static let paused = LongTermWatchRuntimeStatus(
        state: .paused,
        lastActivityAt: nil,
        retryCount: 0
    )
}

protocol LongTermWatchTargetPersisting: AnyObject {
    func loadTargets() -> [LongTermWatchTarget]
    func saveTargets(_ targets: [LongTermWatchTarget])
    func clearTargets()
}

final class UserDefaultsLongTermWatchTargetPersistence: LongTermWatchTargetPersisting {
    private enum Key {
        static let targets = "longTermWatchTargets"
    }

    private let defaults: UserDefaults
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func loadTargets() -> [LongTermWatchTarget] {
        guard let data = defaults.data(forKey: Key.targets) else {
            return []
        }

        do {
            return try decoder.decode([LongTermWatchTarget].self, from: data)
        } catch {
            return []
        }
    }

    func saveTargets(_ targets: [LongTermWatchTarget]) {
        do {
            let data = try encoder.encode(targets)
            defaults.set(data, forKey: Key.targets)
        } catch {
            defaults.removeObject(forKey: Key.targets)
        }
    }

    func clearTargets() {
        defaults.removeObject(forKey: Key.targets)
    }
}

struct LongTermWatchTargetStore {
    private let persistence: any LongTermWatchTargetPersisting

    init(persistence: any LongTermWatchTargetPersisting) {
        self.persistence = persistence
    }

    func loadTargets() -> [LongTermWatchTarget] {
        persistence.loadTargets()
    }

    func upsert(
        _ target: LongTermWatchTarget,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        var updatedTargets = currentTargets.filter { $0.id != target.id }
        updatedTargets.insert(target, at: 0)
        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func setEnabled(
        _ isEnabled: Bool,
        forRootPath rootPath: URL,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        let targetID = rootPath.standardizedFileURL.path
        let updatedTargets = currentTargets.map { target in
            guard target.id == targetID else {
                return target
            }

            var updatedTarget = target
            updatedTarget.isEnabled = isEnabled
            return updatedTarget
        }

        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func remove(
        rootPath: URL,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        let targetID = rootPath.standardizedFileURL.path
        let updatedTargets = currentTargets.filter { $0.id != targetID }
        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func updateCheckpoint(
        _ checkpoint: LongTermWatchCheckpoint,
        forRootPath rootPath: URL,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        let targetID = rootPath.standardizedFileURL.path
        let updatedTargets = currentTargets.map { target in
            guard target.id == targetID else {
                return target
            }

            var updatedTarget = target
            updatedTarget.checkpoint = checkpoint
            return updatedTarget
        }
        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func updateOptions(
        _ options: LongTermWatchTargetOptions,
        forRootPath rootPath: URL,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        let targetID = rootPath.standardizedFileURL.path
        let updatedTargets = currentTargets.map { target in
            guard target.id == targetID else {
                return target
            }

            var updatedTarget = target
            updatedTarget.options = options
            return updatedTarget
        }
        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func updateBaseline(
        _ baseline: ActivityBaselineSnapshot,
        forRootPath rootPath: URL,
        currentTargets: [LongTermWatchTarget]
    ) -> [LongTermWatchTarget] {
        let targetID = rootPath.standardizedFileURL.path
        let updatedTargets = currentTargets.map { target in
            guard target.id == targetID else {
                return target
            }

            var updatedTarget = target
            updatedTarget.baseline = baseline
            return updatedTarget
        }
        persistence.saveTargets(updatedTargets)
        return updatedTargets
    }

    func clear() {
        persistence.clearTargets()
    }
}
