//
//  ScanArchiveService.swift
//  Pathlight
//
//  Created by Codex on 6/22/26.
//

import CryptoKit
import Foundation

nonisolated protocol ScanArchiveServicing: Sendable {
    func export(
        snapshot: ScanSnapshot,
        to destinationURL: URL,
        options: ScanArchiveExportOptions
    ) async throws -> ScanArchiveExportResult

    func previewSnapshot(from sourceURL: URL) async throws -> ScanArchivePreview

    func importSnapshot(
        from sourceURL: URL,
        progressReporter: ScanArchiveProgressReporter?
    ) async throws -> ScanArchiveImportResult
}

extension ScanArchiveServicing {
    func importSnapshot(from sourceURL: URL) async throws -> ScanArchiveImportResult {
        try await importSnapshot(from: sourceURL, progressReporter: nil)
    }
}

nonisolated struct ScanArchiveExportOptions: Sendable {
    var pathMode: ScanArchivePathMode
    var appVersion: String?
    var progressReporter: ScanArchiveProgressReporter?

    nonisolated init(
        pathMode: ScanArchivePathMode = .absolute,
        appVersion: String? = nil,
        progressReporter: ScanArchiveProgressReporter? = nil
    ) {
        self.pathMode = pathMode
        self.appVersion = appVersion
        self.progressReporter = progressReporter
    }
}

nonisolated struct ScanArchiveExportResult: Sendable {
    let archiveURL: URL
    let nodeChecksum: String
}

nonisolated struct ScanArchiveImportResult: Sendable {
    let archiveURL: URL
    let snapshot: ScanSnapshot
    let manifest: ScanArchiveDocument
}

nonisolated enum ScanArchiveProgressPhase: String, Sendable {
    case preparing
    case writingNodes
    case writingTopology
    case writingMetadata
    case readingManifest
    case readingNodes
    case readingTopology
    case validatingTopology
    case readingMetadata
    case rebuildingSnapshot
    case openingSnapshot
}

nonisolated struct ScanArchiveProgress: Equatable, Sendable {
    let phase: ScanArchiveProgressPhase
    let completedUnitCount: Int
    let totalUnitCount: Int?
    let message: String

    nonisolated init(
        phase: ScanArchiveProgressPhase,
        completedUnitCount: Int = 0,
        totalUnitCount: Int? = nil,
        message: String
    ) {
        self.phase = phase
        self.completedUnitCount = completedUnitCount
        self.totalUnitCount = totalUnitCount
        self.message = message
    }

    var fractionCompleted: Double? {
        guard let totalUnitCount, totalUnitCount > 0 else { return nil }
        return min(1, max(0, Double(completedUnitCount) / Double(totalUnitCount)))
    }
}

nonisolated final class ScanArchiveProgressReporter: @unchecked Sendable {
    let updates: AsyncStream<ScanArchiveProgress>
    private let continuation: AsyncStream<ScanArchiveProgress>.Continuation

    nonisolated init() {
        let streamPair = AsyncStream.makeStream(
            of: ScanArchiveProgress.self,
            bufferingPolicy: .bufferingNewest(1)
        )
        self.updates = streamPair.stream
        self.continuation = streamPair.continuation
    }

    func report(_ progress: ScanArchiveProgress) {
        continuation.yield(progress)
    }

    func finish() {
        continuation.finish()
    }
}

nonisolated struct ScanArchivePreview: Identifiable, Sendable {
    let archiveURL: URL
    let archiveSize: Int64
    let exportedAt: Date
    let appVersion: String
    let target: ScanArchiveTargetV1
    let startedAt: Date
    let finishedAt: Date?
    let isComplete: Bool
    let nodeCount: Int
    let warningCount: Int
    let totalAllocatedSize: Int64
    let totalLogicalSize: Int64
    let fileCount: Int
    let directoryCount: Int
    let accessibleItemCount: Int
    let inaccessibleItemCount: Int

    var id: URL {
        archiveURL
    }

    init(
        archiveURL: URL,
        archiveSize: Int64,
        manifest: ScanArchiveDocument,
        stats: ScanArchiveStatsV1
    ) {
        self.archiveURL = archiveURL
        self.archiveSize = archiveSize
        self.exportedAt = manifest.exportedAt
        self.appVersion = manifest.createdBy.appVersion
        self.target = manifest.snapshot.target
        self.startedAt = manifest.snapshot.startedAt
        self.finishedAt = manifest.snapshot.finishedAt
        self.isComplete = manifest.snapshot.isComplete
        self.nodeCount = manifest.snapshot.nodeCount
        self.warningCount = manifest.snapshot.warningCount
        self.totalAllocatedSize = stats.totalAllocatedSize
        self.totalLogicalSize = stats.totalLogicalSize
        self.fileCount = stats.fileCount
        self.directoryCount = stats.directoryCount
        self.accessibleItemCount = stats.accessibleItemCount
        self.inaccessibleItemCount = stats.inaccessibleItemCount
    }
}

nonisolated enum ScanArchiveError: LocalizedError, Equatable {
    case incompleteSnapshot
    case invalidArchivePackage(String)
    case unsupportedFormat(String)
    case unsupportedVersion(Int)
    case manifest(String)
    case nodes(String)
    case topology(String)
    case integrity(String)
    case stats(String)

    var errorDescription: String? {
        switch self {
        case .incompleteSnapshot:
            return "Only complete scans can be exported."
        case .invalidArchivePackage(let detail):
            return "The Pathlight scan snapshot package is invalid: \(detail)"
        case .unsupportedFormat(let format):
            return "Unsupported Pathlight scan snapshot format: \(format)."
        case .unsupportedVersion(let version):
            return "Unsupported Pathlight scan snapshot version: \(version)."
        case .manifest(let detail):
            return "Pathlight could not read the scan snapshot manifest: \(detail)"
        case .nodes(let detail):
            return "Pathlight could not read the scan snapshot node payload: \(detail)"
        case .topology(let detail):
            return "Pathlight could not read the scan snapshot topology: \(detail)"
        case .integrity(let detail):
            return "Pathlight scan snapshot integrity check failed: \(detail)"
        case .stats(let detail):
            return "Pathlight could not read the scan snapshot stats: \(detail)"
        }
    }
}

nonisolated struct ScanArchiveService: ScanArchiveServicing {
    nonisolated static let fileExtension = "pathlightscan"
    nonisolated static let formatIdentifier = "dev.ctrlcreeper.pathlight.scan"
    nonisolated static let currentFormatVersion = 3

    private nonisolated static let manifestFileName = "manifest.json"
    private nonisolated static let nodesFileName = "nodes.jsonl"
    private nonisolated static let topologyFileName = "topology.json"
    private nonisolated static let warningsFileName = "warnings.json"
    private nonisolated static let statsFileName = "stats.json"

    init(fileManager: FileManager = .default) {
        _ = fileManager
    }

    private var fileManager: FileManager {
        .default
    }

    func export(
        snapshot: ScanSnapshot,
        to destinationURL: URL,
        options: ScanArchiveExportOptions = ScanArchiveExportOptions()
    ) async throws -> ScanArchiveExportResult {
        try Task.checkCancellation()
        guard snapshot.isComplete else {
            throw ScanArchiveError.incompleteSnapshot
        }
        try validateArchiveExtension(destinationURL)

        let archiveURL = try createTemporaryArchiveDirectory(for: destinationURL)
        var didInstallArchive = false
        defer {
            if !didInstallArchive {
                try? fileManager.removeItem(at: archiveURL)
            }
        }

        let archiveSections = ScanArchiveSections(
            nodes: Self.nodesFileName,
            topology: Self.topologyFileName,
            warnings: Self.warningsFileName,
            stats: Self.statsFileName
        )
        let nodesURL = archiveURL.appending(path: archiveSections.nodes, directoryHint: .notDirectory)
        let topologyURL = archiveURL.appending(path: archiveSections.topology, directoryHint: .notDirectory)
        let warningsURL = archiveURL.appending(path: archiveSections.warnings, directoryHint: .notDirectory)
        let statsURL = archiveURL.appending(path: archiveSections.stats, directoryHint: .notDirectory)
        let manifestURL = archiveURL.appending(path: Self.manifestFileName, directoryHint: .notDirectory)

        options.progressReporter?.report(ScanArchiveProgress(
            phase: .preparing,
            message: "Preparing archive"
        ))
        let nodeChecksum = try await writeNodes(
            snapshot.treeStore,
            to: nodesURL,
            progressReporter: options.progressReporter
        )
        try Task.checkCancellation()

        options.progressReporter?.report(ScanArchiveProgress(
            phase: .writingTopology,
            message: "Writing topology"
        ))
        try writeJSON(ScanArchiveTopology(snapshot.treeStore), to: topologyURL)

        options.progressReporter?.report(ScanArchiveProgress(
            phase: .writingMetadata,
            message: "Writing metadata"
        ))
        try writeJSON(snapshot.scanWarnings.map(ScanArchiveWarningV1.init), to: warningsURL)
        try writeJSON(ScanArchiveStatsV1(snapshot.aggregateStats), to: statsURL)

        let manifest = try ScanArchiveDocument(
            exportedAt: Date(),
            appVersion: options.appVersion ?? Self.currentAppVersion(),
            snapshot: snapshot,
            pathMode: options.pathMode,
            sections: archiveSections,
            nodeChecksum: nodeChecksum
        )
        try writeJSON(manifest, to: manifestURL)

        try Task.checkCancellation()
        try installArchive(from: archiveURL, to: destinationURL)
        didInstallArchive = true

        return ScanArchiveExportResult(archiveURL: destinationURL, nodeChecksum: nodeChecksum)
    }

    func previewSnapshot(from sourceURL: URL) async throws -> ScanArchivePreview {
        try Task.checkCancellation()
        let manifest = try readValidatedManifest(from: sourceURL)
        let statsURL = try sectionURL(
            named: manifest.sections.stats,
            in: sourceURL,
            sectionDescription: "stats"
        )
        let stats: ScanArchiveStatsV1 = try readJSON(ScanArchiveStatsV1.self, from: statsURL) { detail in
            ScanArchiveError.stats(detail)
        }
        let archiveSize = try archiveLogicalSize(at: sourceURL)
        return ScanArchivePreview(
            archiveURL: sourceURL,
            archiveSize: archiveSize,
            manifest: manifest,
            stats: stats
        )
    }

    func importSnapshot(
        from sourceURL: URL,
        progressReporter: ScanArchiveProgressReporter? = nil
    ) async throws -> ScanArchiveImportResult {
        try Task.checkCancellation()
        progressReporter?.report(ScanArchiveProgress(
            phase: .readingManifest,
            message: "Reading manifest"
        ))
        let manifest = try readValidatedManifest(from: sourceURL)

        let nodesURL = try sectionURL(
            named: manifest.sections.nodes,
            in: sourceURL,
            sectionDescription: "nodes"
        )
        let topologyURL = try sectionURL(
            named: manifest.sections.topology,
            in: sourceURL,
            sectionDescription: "topology"
        )
        let warningsURL = try sectionURL(
            named: manifest.sections.warnings,
            in: sourceURL,
            sectionDescription: "warnings"
        )
        let statsURL = try sectionURL(
            named: manifest.sections.stats,
            in: sourceURL,
            sectionDescription: "stats"
        )

        let nodePayload = try await readNodes(
            from: nodesURL,
            expectedChecksum: manifest.integrity.nodes,
            expectedNodeCount: manifest.snapshot.nodeCount,
            progressReporter: progressReporter
        )

        progressReporter?.report(ScanArchiveProgress(
            phase: .readingTopology,
            message: "Reading topology"
        ))
        let archivedTopology: ScanArchiveTopology = try readJSON(ScanArchiveTopology.self, from: topologyURL) { detail in
            ScanArchiveError.topology(detail)
        }
        let topology = try archivedTopology.resolvedTopology(orderedNodeIDs: nodePayload.orderedNodeIDs)

        progressReporter?.report(ScanArchiveProgress(
            phase: .readingMetadata,
            message: "Reading metadata"
        ))
        let warnings: [ScanArchiveWarningV1] = try readJSON([ScanArchiveWarningV1].self, from: warningsURL) { detail in
            ScanArchiveError.manifest("warnings section failed: \(detail)")
        }
        let archivedStats: ScanArchiveStatsV1 = try readJSON(ScanArchiveStatsV1.self, from: statsURL) { detail in
            ScanArchiveError.stats(detail)
        }

        try Task.checkCancellation()
        progressReporter?.report(ScanArchiveProgress(
            phase: .rebuildingSnapshot,
            message: "Rebuilding snapshot"
        ))
        try validateCounts(manifest: manifest, nodesByID: nodePayload.nodesByID, warnings: warnings)
        let rebuiltParentIDs = try await validateTopology(
            topology,
            nodesByID: nodePayload.nodesByID,
            expectedRootID: manifest.snapshot.rootID,
            expectedTargetPath: manifest.snapshot.target.path,
            progressReporter: progressReporter
        )
        let treeStore = FileTreeStore(
            rootID: topology.rootID,
            nodesByID: nodePayload.nodesByID,
            childIDsByID: topology.childIDsByID,
            parentIDByID: rebuiltParentIDs
        )
        var importedWarnings = try warnings.map { try $0.modelWarning() }
        let computedStats = treeStore.aggregateStats
        if !archivedStats.matches(computedStats) {
            importedWarnings.append(Self.repairedStatsWarning(rootID: topology.rootID))
        }

        let snapshot = ScanSnapshot(
            id: manifest.snapshot.id,
            target: manifest.snapshot.target.modelTarget(),
            treeStore: treeStore,
            startedAt: manifest.snapshot.startedAt,
            finishedAt: manifest.snapshot.finishedAt,
            scanWarnings: importedWarnings,
            aggregateStats: computedStats,
            isComplete: manifest.snapshot.isComplete,
            scanOptions: manifest.snapshot.scanOptions,
            source: .imported(ImportedSnapshotContext(
                sourceURL: sourceURL,
                pathMode: manifest.snapshot.pathMode,
                liveActionCapability: manifest.snapshot.pathMode == .absolute ? .pathValidation : .disabled
            ))
        )

        return ScanArchiveImportResult(archiveURL: sourceURL, snapshot: snapshot, manifest: manifest)
    }

    private func readValidatedManifest(from sourceURL: URL) throws -> ScanArchiveDocument {
        try validatePackage(at: sourceURL)

        let manifestURL = sourceURL.appending(path: Self.manifestFileName, directoryHint: .notDirectory)
        let manifestData = try readData(from: manifestURL) { detail in
            ScanArchiveError.manifest(detail)
        }
        let header: ScanArchiveHeader = try decodeJSON(ScanArchiveHeader.self, from: manifestData) { detail in
            ScanArchiveError.manifest(detail)
        }
        try validateManifestHeader(format: header.format, formatVersion: header.formatVersion)
        let manifest: ScanArchiveDocument = try decodeJSON(ScanArchiveDocument.self, from: manifestData) { detail in
            ScanArchiveError.manifest(detail)
        }
        try validateManifest(manifest)
        return manifest
    }

    private func validatePackage(at url: URL) throws {
        try validateArchiveExtension(url)
        var isDirectory = ObjCBool(false)
        guard fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory),
              isDirectory.boolValue else {
            throw ScanArchiveError.invalidArchivePackage("expected a .\(Self.fileExtension) package directory")
        }
    }

    private func validateArchiveExtension(_ url: URL) throws {
        guard url.pathExtension.lowercased() == Self.fileExtension else {
            throw ScanArchiveError.invalidArchivePackage("expected a .\(Self.fileExtension) package")
        }
    }

    private func archiveLogicalSize(at archiveURL: URL) throws -> Int64 {
        let relativePaths: [String]
        do {
            relativePaths = try fileManager.subpathsOfDirectory(atPath: archiveURL.path)
        } catch {
            throw ScanArchiveError.invalidArchivePackage(
                "could not calculate snapshot size: \(error.localizedDescription)"
            )
        }

        var totalSize: Int64 = 0
        for relativePath in relativePaths {
            try Task.checkCancellation()
            let itemURL = archiveURL.appending(path: relativePath)
            let values: URLResourceValues
            do {
                values = try itemURL.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            } catch {
                throw ScanArchiveError.invalidArchivePackage(
                    "could not calculate snapshot size: \(error.localizedDescription)"
                )
            }

            guard values.isRegularFile == true else {
                continue
            }

            let (newTotalSize, overflow) = totalSize.addingReportingOverflow(Int64(values.fileSize ?? 0))
            guard !overflow else {
                throw ScanArchiveError.invalidArchivePackage("snapshot size exceeds supported range")
            }
            totalSize = newTotalSize
        }
        return totalSize
    }

    private func validateManifest(_ manifest: ScanArchiveDocument) throws {
        try validateManifestHeader(format: manifest.format, formatVersion: manifest.formatVersion)
        guard manifest.snapshot.isComplete else {
            throw ScanArchiveError.manifest("snapshot is not complete")
        }
        guard manifest.snapshot.nodeCount > 0 else {
            throw ScanArchiveError.manifest("snapshot has no nodes")
        }
        guard !manifest.snapshot.rootID.isEmpty,
              manifest.snapshot.rootID == manifest.snapshot.target.path else {
            throw ScanArchiveError.manifest("snapshot root does not match target path")
        }
        if let scanOptions = manifest.snapshot.scanOptions {
            let fingerprint = try Self.scanOptionsFingerprint(scanOptions)
            guard fingerprint == manifest.snapshot.scanOptionsFingerprint else {
                throw ScanArchiveError.integrity("scan options fingerprint mismatch")
            }
        }
        guard manifest.integrity.algorithm == "sha256" else {
            throw ScanArchiveError.integrity("unsupported integrity algorithm \(manifest.integrity.algorithm)")
        }
    }

    private func validateManifestHeader(format: String, formatVersion: Int) throws {
        guard format == Self.formatIdentifier else {
            throw ScanArchiveError.unsupportedFormat(format)
        }
        guard formatVersion == Self.currentFormatVersion else {
            throw ScanArchiveError.unsupportedVersion(formatVersion)
        }
    }

    private func validateCounts(
        manifest: ScanArchiveDocument,
        nodesByID: [String: FileNodeRecord],
        warnings: [ScanArchiveWarningV1]
    ) throws {
        guard nodesByID.count == manifest.snapshot.nodeCount else {
            throw ScanArchiveError.nodes("manifest expected \(manifest.snapshot.nodeCount) nodes, found \(nodesByID.count)")
        }
        guard warnings.count == manifest.snapshot.warningCount else {
            throw ScanArchiveError.manifest("manifest expected \(manifest.snapshot.warningCount) warnings, found \(warnings.count)")
        }
    }

    private func sectionURL(named sectionName: String, in archiveURL: URL, sectionDescription: String) throws -> URL {
        guard !sectionName.isEmpty,
              !sectionName.contains("/"),
              !sectionName.contains("\\") else {
            throw ScanArchiveError.manifest("invalid \(sectionDescription) section path")
        }
        return archiveURL.appending(path: sectionName, directoryHint: .notDirectory)
    }

    private func createTemporaryArchiveDirectory(for destinationURL: URL) throws -> URL {
        let parentURL = destinationURL.deletingLastPathComponent()
        let tempName = ".\(destinationURL.lastPathComponent).\(UUID().uuidString).tmp"
        let tempURL = parentURL.appending(path: tempName, directoryHint: .isDirectory)
        try fileManager.createDirectory(at: tempURL, withIntermediateDirectories: false)
        return tempURL
    }

    private func installArchive(from temporaryURL: URL, to destinationURL: URL) throws {
        if fileManager.fileExists(atPath: destinationURL.path) {
            var resultingURL: NSURL?
            try fileManager.replaceItem(
                at: destinationURL,
                withItemAt: temporaryURL,
                backupItemName: nil,
                options: [],
                resultingItemURL: &resultingURL
            )
        } else {
            try fileManager.moveItem(at: temporaryURL, to: destinationURL)
        }
    }

    private func writeJSON<T: Encodable>(_ value: T, to url: URL) throws {
        let data = try Self.makeSectionJSONEncoder().encode(value)
        try data.write(to: url, options: [.atomic])
    }

    private func readJSON<T: Decodable>(
        _ type: T.Type,
        from url: URL,
        mapError: (String) -> ScanArchiveError
    ) throws -> T {
        let data = try readData(from: url, mapError: mapError)
        return try decodeJSON(type, from: data, mapError: mapError)
    }

    private func readData(
        from url: URL,
        mapError: (String) -> ScanArchiveError
    ) throws -> Data {
        do {
            return try Data(contentsOf: url)
        } catch let error as ScanArchiveError {
            throw error
        } catch {
            throw mapError(error.localizedDescription)
        }
    }

    private func decodeJSON<T: Decodable>(
        _ type: T.Type,
        from data: Data,
        mapError: (String) -> ScanArchiveError
    ) throws -> T {
        do {
            return try Self.makeJSONDecoder().decode(type, from: data)
        } catch let error as ScanArchiveError {
            throw error
        } catch {
            throw mapError(error.localizedDescription)
        }
    }

    private static func makeSectionJSONEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.withoutEscapingSlashes]
        return encoder
    }

    private static func makeFingerprintJSONEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return encoder
    }

    static func makeJSONLineEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.withoutEscapingSlashes]
        return encoder
    }

    static func makeJSONDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }

    nonisolated static func scanOptionsFingerprint(_ options: ScanOptions?) throws -> String? {
        guard let options else { return nil }
        let data = try makeFingerprintJSONEncoder().encode(options)
        return Data(SHA256.hash(data: data)).base64EncodedString()
    }

    private static func currentAppVersion() -> String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
    }

    private static func repairedStatsWarning(rootID: String) -> ScanWarning {
        ScanWarning(
            path: rootID,
            message: "Archive stats did not match node payload. Pathlight repaired totals during import.",
            category: .fileSystem
        )
    }
}
