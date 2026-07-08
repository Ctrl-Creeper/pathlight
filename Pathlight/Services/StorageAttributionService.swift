import Foundation

struct StorageAttributionService {
    typealias SizeProvider = @Sendable (URL) -> Int64?

    private let options: DiskActivityAggregationOptions
    private let sizeProvider: SizeProvider
    private let priorSizeProvider: SizeProvider

    init(
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping SizeProvider,
        priorSizeProvider: @escaping SizeProvider = { _ in nil }
    ) {
        self.options = options
        self.sizeProvider = sizeProvider
        self.priorSizeProvider = priorSizeProvider
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        changes.compactMap { change in
            guard let event = event(for: change) else {
                return nil
            }
            guard let byteDelta = event.byteDelta else {
                return event
            }
            return abs(byteDelta) >= options.minimumRecordedByteDelta ? event : nil
        }
    }

    private func event(for change: DiskActivityChange) -> DiskActivityEvent? {
        switch change.kind {
        case .created:
            return sizedEvent(kind: .created, change: change, confidence: .confirmed)
        case .modified:
            return sizedEvent(kind: .modified, change: change, confidence: .confirmed)
        case .deleted:
            if let priorSize = priorSizeProvider(change.path) {
                return DiskActivityEvent(
                    kind: .deleted,
                    path: change.path,
                    rootPath: change.rootPath,
                    timestamp: change.timestamp,
                    byteDelta: -priorSize,
                    confidence: .estimated,
                    previousPath: nil,
                    affectedItemCount: 1
                )
            }
            return DiskActivityEvent(
                kind: .deleted,
                path: change.path,
                rootPath: change.rootPath,
                timestamp: change.timestamp,
                byteDelta: nil,
                confidence: .unknown,
                previousPath: nil,
                affectedItemCount: 1
            )
        case .renamed(let previousPath):
            return sizedEvent(
                kind: .moved,
                change: change,
                confidence: .confirmed,
                previousPath: previousPath
            )
        }
    }

    private func sizedEvent(
        kind: DiskActivityEventKind,
        change: DiskActivityChange,
        confidence: DiskActivityEventConfidence,
        previousPath: URL? = nil
    ) -> DiskActivityEvent? {
        guard let size = sizeProvider(change.path) else {
            return nil
        }
        return DiskActivityEvent(
            kind: kind,
            path: change.path,
            rootPath: change.rootPath,
            timestamp: change.timestamp,
            byteDelta: size,
            confidence: confidence,
            previousPath: previousPath,
            affectedItemCount: 1
        )
    }
}
