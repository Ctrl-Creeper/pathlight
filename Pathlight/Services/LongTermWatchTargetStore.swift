import Foundation

nonisolated struct LongTermWatchTargetOptions: Codable, Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let recordsFileNames: Bool

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

    init(
        rootPath: URL,
        isEnabled: Bool = true,
        options: LongTermWatchTargetOptions = .default,
        baseline: ActivityBaselineSnapshot? = nil
    ) {
        let standardizedRoot = rootPath.standardizedFileURL
        self.id = standardizedRoot.path
        self.rootPath = standardizedRoot
        self.isEnabled = isEnabled
        self.options = options
        self.baseline = baseline
    }
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

    func clear() {
        persistence.clearTargets()
    }
}
