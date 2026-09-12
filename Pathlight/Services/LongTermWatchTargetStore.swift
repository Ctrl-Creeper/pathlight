import Foundation

nonisolated struct LongTermWatchTargetOptions: Codable, Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let recordsFileNames: Bool
    /// How long the native watcher coalesces changes before reporting them.
    let monitorLatency: TimeInterval
    /// Daily net growth that triggers a notification; nil means alerts are off.
    var growthAlertThresholdBytes: Int64?
    /// Gitignore-style patterns filtered out of this target's event stream.
    /// Empty means record everything.
    var exclusionPatterns: [String]
    /// Bounds on the size of the files this watch records; nil is no bound.
    var minimumFileBytes: Int64?
    var maximumFileBytes: Int64?

    init(
        minimumRecordedByteDelta: Int64,
        aggregationWindow: TimeInterval,
        recordsFileNames: Bool,
        monitorLatency: TimeInterval = 5,
        growthAlertThresholdBytes: Int64? = nil,
        exclusionPatterns: [String] = ActivityExclusionPatterns.defaults,
        minimumFileBytes: Int64? = nil,
        maximumFileBytes: Int64? = nil
    ) {
        self.minimumRecordedByteDelta = minimumRecordedByteDelta
        self.aggregationWindow = aggregationWindow
        self.recordsFileNames = recordsFileNames
        self.monitorLatency = MonitoringStartConfiguration.boundedMonitorLatency(monitorLatency)
        self.growthAlertThresholdBytes = growthAlertThresholdBytes
        self.exclusionPatterns = exclusionPatterns
        self.minimumFileBytes = minimumFileBytes
        self.maximumFileBytes = maximumFileBytes
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        minimumRecordedByteDelta = try container.decode(Int64.self, forKey: .minimumRecordedByteDelta)
        aggregationWindow = try container.decode(TimeInterval.self, forKey: .aggregationWindow)
        recordsFileNames = try container.decode(Bool.self, forKey: .recordsFileNames)
        // Before this was configurable, background watches always used 30 s.
        let persistedLatency = try container.decodeIfPresent(
            TimeInterval.self,
            forKey: .monitorLatency
        ) ?? 30
        monitorLatency = MonitoringStartConfiguration.boundedMonitorLatency(persistedLatency)
        growthAlertThresholdBytes = try container.decodeIfPresent(Int64.self, forKey: .growthAlertThresholdBytes)
        // Targets persisted before exclusions existed adopt the defaults.
        exclusionPatterns = try container.decodeIfPresent([String].self, forKey: .exclusionPatterns)
            ?? ActivityExclusionPatterns.defaults
        // Absent in targets persisted before size bounds existed, which is also
        // how "no bound" is stored: unbounded is the default a watch keeps.
        minimumFileBytes = try container.decodeIfPresent(Int64.self, forKey: .minimumFileBytes)
        maximumFileBytes = try container.decodeIfPresent(Int64.self, forKey: .maximumFileBytes)
    }

    static let `default` = LongTermWatchTargetOptions(
        minimumRecordedByteDelta: DiskActivityAggregationOptions.default.minimumRecordedByteDelta,
        aggregationWindow: DiskActivityAggregationOptions.default.aggregationWindow,
        recordsFileNames: DiskActivityAggregationOptions.default.longTermRecordsFileNames,
        monitorLatency: 5
    )

    var diskActivityOptions: DiskActivityAggregationOptions {
        DiskActivityAggregationOptions(
            minimumRecordedByteDelta: minimumRecordedByteDelta,
            aggregationWindow: aggregationWindow,
            longTermRecordsFileNames: recordsFileNames,
            minimumFileBytes: minimumFileBytes,
            maximumFileBytes: maximumFileBytes
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
