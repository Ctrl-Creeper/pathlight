import Foundation

nonisolated struct ActivityBaselineSnapshot: Codable, Equatable, Sendable {
    let rootPath: URL
    let capturedAt: Date
    let allocatedSize: Int64
    let measuredItemCount: Int
    let unreadableItemCount: Int

    init(
        rootPath: URL,
        capturedAt: Date,
        allocatedSize: Int64,
        measuredItemCount: Int,
        unreadableItemCount: Int
    ) {
        self.rootPath = rootPath.standardizedFileURL
        self.capturedAt = capturedAt
        self.allocatedSize = allocatedSize
        self.measuredItemCount = measuredItemCount
        self.unreadableItemCount = unreadableItemCount
    }
}

struct ActivityBaselineService: Sendable {
    typealias ContentsProvider = @Sendable (URL) throws -> [URL]

    private let sizeProvider: StorageAttributionService.SizeProvider
    private let contentsProvider: ContentsProvider

    init(
        sizeProvider: @escaping StorageAttributionService.SizeProvider = FileAllocatedSizeProvider.allocatedSize(for:),
        contentsProvider: @escaping ContentsProvider = Self.liveContents(at:)
    ) {
        self.sizeProvider = sizeProvider
        self.contentsProvider = contentsProvider
    }

    func captureBaseline(
        rootPath: URL,
        capturedAt: Date = Date()
    ) async -> ActivityBaselineSnapshot {
        let standardizedRoot = rootPath.standardizedFileURL
        var pending = [standardizedRoot]
        var allocatedSize: Int64 = 0
        var measuredItemCount = 0
        var unreadableItemCount = 0
        var visitedPaths = Set<String>()

        while let url = pending.popLast() {
            let standardizedURL = url.standardizedFileURL
            guard visitedPaths.insert(standardizedURL.path).inserted else {
                continue
            }

            if let size = sizeProvider(standardizedURL) {
                allocatedSize += size
                measuredItemCount += 1
            } else {
                unreadableItemCount += 1
            }

            do {
                pending.append(contentsOf: try contentsProvider(standardizedURL))
            } catch {
                unreadableItemCount += 1
            }
        }

        return ActivityBaselineSnapshot(
            rootPath: standardizedRoot,
            capturedAt: capturedAt,
            allocatedSize: allocatedSize,
            measuredItemCount: measuredItemCount,
            unreadableItemCount: unreadableItemCount
        )
    }

    private nonisolated static func liveContents(at url: URL) throws -> [URL] {
        try FileManager.default.contentsOfDirectory(
            at: url,
            includingPropertiesForKeys: [
                .isDirectoryKey,
                .isRegularFileKey,
                .isSymbolicLinkKey,
                .totalFileAllocatedSizeKey,
                .fileAllocatedSizeKey
            ],
            options: []
        )
    }
}
