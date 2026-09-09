import Foundation
@testable import PathlightCore

func makeEncryptedActivityStorageLineCodec(encryptNewData: Bool = true) -> ActivityStorageLineCodec {
    ActivityStorageLineCodec(
        preferencesStore: FixedActivityStoragePreferencesStore(encryptNewData: encryptNewData),
        cryptor: AESGCMActivityStorageCryptor(keyProvider: FixedActivityStorageKeyProvider())
    )
}

func posixPermissions(at url: URL) throws -> Int {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return attributes[.posixPermissions] as? Int ?? 0
}

final class FixedActivityStoragePreferencesStore: ActivityStoragePreferencesPersisting {
    private let encryptNewData: Bool

    init(encryptNewData: Bool) {
        self.encryptNewData = encryptNewData
    }

    func loadPreferences() -> ActivityStoragePreferences {
        var preferences = ActivityStoragePreferences.defaults
        preferences.encryptNewData = encryptNewData
        return preferences
    }

    func savePreferences(_ preferences: ActivityStoragePreferences) {}
}

private struct FixedActivityStorageKeyProvider: ActivityStorageKeyProviding {
    func loadOrCreateKey() throws -> Data {
        Data(repeating: 7, count: 32)
    }
}

/// Byte attribution the app can be tested against.
///
/// The real one lives in the Rust core, which this package deliberately does
/// not link, so tests here stub it: one event per change, measured from the
/// watch's own providers. No aggregation, no size threshold, no rename netting
/// — `core/tests/attribution.rs` owns those rules, and these tests are about
/// what the app does with the events it gets.
final class StubActivityAttribution: ActivityAttributing, @unchecked Sendable {
    private let lock = NSLock()
    private let attribute: @Sendable (DiskActivityChange) -> [DiskActivityEvent]
    private var observed: [DiskActivityChange] = []

    init(attribute: @escaping @Sendable (DiskActivityChange) -> [DiskActivityEvent] = { _ in [] }) {
        self.attribute = attribute
    }

    /// Every change attribution was asked about. Emptiness is the assertion in
    /// the isolation tests: measuring a path is what records the next event.
    var observedChanges: [DiskActivityChange] {
        lock.lock()
        defer {
            lock.unlock()
        }
        return observed
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        lock.lock()
        observed.append(contentsOf: changes)
        lock.unlock()
        return changes.flatMap(attribute)
    }
}

/// One event per change, with the kinds attribution maps them to.
func stubActivityEvent(
    for change: DiskActivityChange,
    byteDelta: Int64?,
    confidence: DiskActivityEventConfidence = .confirmed
) -> DiskActivityEvent {
    let kind: DiskActivityEventKind = switch change.kind {
    case .created: .created
    case .modified: .modified
    case .deleted: .deleted
    case .renamed: .moved
    }
    return DiskActivityEvent(
        kind: kind,
        path: change.path,
        rootPath: change.rootPath,
        timestamp: change.timestamp,
        byteDelta: byteDelta,
        confidence: byteDelta == nil ? .unknown : confidence,
        previousPath: nil,
        affectedItemCount: 1
    )
}

let stubActivityAttribution: ActivityAttributionFactory = { _, sizes in
    StubActivityAttribution { change in
        switch change.kind {
        case .created, .modified:
            // `known` before `size`: measuring a path is also what records it,
            // so asking the other way round reports every change as zero.
            let known = sizes.known(change.path)
            guard let size = sizes.size(change.path) else {
                return []
            }
            // A first observation of an existing file is no evidence of growth,
            // which is the one arithmetic rule these tests still depend on.
            let byteDelta = known.map { size - $0 } ?? (change.kind == .created ? size : nil)
            return [stubActivityEvent(for: change, byteDelta: byteDelta)]
        case .deleted, .renamed:
            return [
                stubActivityEvent(
                    for: change,
                    byteDelta: sizes.prior(change.path).map { -$0 },
                    confidence: .estimated
                )
            ]
        }
    }
}
