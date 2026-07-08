import Foundation

struct StorageAttributionService {
    typealias SizeProvider = @Sendable (URL) -> Int64?

    private let options: DiskActivityAggregationOptions
    private let sizeProvider: SizeProvider

    init(
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping SizeProvider
    ) {
        self.options = options
        self.sizeProvider = sizeProvider
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        changes.compactMap { change in
            let size = sizeProvider(change.path)
            guard let size, abs(size) >= options.minimumRecordedByteDelta else {
                return nil
            }
            return DiskActivityEvent(
                kind: .created,
                path: change.path,
                rootPath: change.rootPath,
                timestamp: change.timestamp,
                byteDelta: size,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        }
    }
}
