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
    private let isSystemUnderPressure: @Sendable () -> Bool

    init(
        sizeProvider: @escaping StorageAttributionService.SizeProvider = FileAllocatedSizeProvider.allocatedSize(for:),
        contentsProvider: @escaping ContentsProvider = Self.liveContents(at:),
        isSystemUnderPressure: @escaping @Sendable () -> Bool = Self.liveSystemPressure
    ) {
        self.sizeProvider = sizeProvider
        self.contentsProvider = contentsProvider
        self.isSystemUnderPressure = isSystemUnderPressure
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
            // Callers discard the result on cancellation, so a partial walk is safe.
            guard !Task.isCancelled else {
                break
            }
            // ponytail: crude pacing — back off in half-second steps while the
            // machine is hot or on low power instead of racing through the walk.
            if visitedPaths.count % 512 == 511, isSystemUnderPressure() {
                try? await Task.sleep(for: .milliseconds(500))
            }
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

    nonisolated static func liveSystemPressure() -> Bool {
        let processInfo = ProcessInfo.processInfo
        switch processInfo.thermalState {
        case .serious, .critical:
            return true
        case .nominal, .fair:
            break
        @unknown default:
            break
        }
        return processInfo.isLowPowerModeEnabled
    }

    private nonisolated static func liveContents(at url: URL) throws -> [URL] {
        // Only real directories are enumerated: regular files would throw here and
        // be miscounted as unreadable, and following symlinks double counts or loops.
        let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard values.isDirectory == true, values.isSymbolicLink != true else {
            return []
        }
        return try FileManager.default.contentsOfDirectory(
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
