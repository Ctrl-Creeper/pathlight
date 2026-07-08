//
//  ScanArchiveModels.swift
//  Pathlight
//

import Foundation

nonisolated struct ScanArchiveResolvedTopology: Sendable {
    let rootID: String
    let childIDsByID: [String: [String]]
}

nonisolated struct ScanArchiveHeader: Decodable, Sendable {
    let format: String
    let formatVersion: Int
}

nonisolated struct ScanArchiveDocument: Codable, Sendable {
    let format: String
    let formatVersion: Int
    let createdBy: ScanArchiveCreatedBy
    let exportedAt: Date
    let snapshot: ScanArchiveSnapshotSummary
    let sections: ScanArchiveSections
    let integrity: ScanArchiveIntegrity

    init(
        exportedAt: Date,
        appVersion: String,
        snapshot: ScanSnapshot,
        pathMode: ScanArchivePathMode,
        sections: ScanArchiveSections,
        nodeChecksum: String,
        formatVersion: Int = ScanArchiveService.currentFormatVersion,
        swiftSchema: String = "ScanArchiveV3"
    ) throws {
        self.format = ScanArchiveService.formatIdentifier
        self.formatVersion = formatVersion
        self.createdBy = ScanArchiveCreatedBy(appVersion: appVersion, swiftSchema: swiftSchema)
        self.exportedAt = exportedAt
        self.snapshot = try ScanArchiveSnapshotSummary(snapshot, pathMode: pathMode)
        self.sections = sections
        self.integrity = ScanArchiveIntegrity(nodes: nodeChecksum)
    }
}

nonisolated struct ScanArchiveCreatedBy: Codable, Sendable {
    let app: String
    let appVersion: String
    let swiftSchema: String

    init(appVersion: String, swiftSchema: String = "ScanArchiveV3") {
        self.app = "Pathlight"
        self.appVersion = appVersion
        self.swiftSchema = swiftSchema
    }
}

nonisolated struct ScanArchiveSnapshotSummary: Codable, Sendable {
    let id: UUID
    let startedAt: Date
    let finishedAt: Date?
    let isComplete: Bool
    let target: ScanArchiveTargetV1
    let rootID: String
    let nodeCount: Int
    let warningCount: Int
    let pathMode: ScanArchivePathMode
    let scanOptions: ScanOptions?
    let scanOptionsFingerprint: String?

    init(_ snapshot: ScanSnapshot, pathMode: ScanArchivePathMode) throws {
        self.id = snapshot.id
        self.startedAt = snapshot.startedAt
        self.finishedAt = snapshot.finishedAt
        self.isComplete = snapshot.isComplete
        self.target = ScanArchiveTargetV1(snapshot.target)
        self.rootID = snapshot.treeStore.rootID
        self.nodeCount = snapshot.treeStore.nodeCount
        self.warningCount = snapshot.scanWarnings.count
        self.pathMode = pathMode
        self.scanOptions = snapshot.scanOptions
        self.scanOptionsFingerprint = try ScanArchiveService.scanOptionsFingerprint(snapshot.scanOptions)
    }
}

nonisolated struct ScanArchiveTargetV1: Codable, Sendable {
    let path: String
    let displayName: String
    let kind: ScanTargetKind

    init(_ target: ScanTarget) {
        self.path = target.url.path
        self.displayName = target.displayName
        self.kind = target.kind
    }

    func modelTarget() -> ScanTarget {
        ScanTarget(
            id: path,
            url: URL(filePath: path, directoryHint: .isDirectory),
            displayName: displayName,
            kind: kind
        )
    }
}

nonisolated struct ScanArchiveSections: Codable, Sendable {
    let nodes: String
    let topology: String
    let warnings: String
    let stats: String
}

nonisolated struct ScanArchiveIntegrity: Codable, Sendable {
    let algorithm: String
    let nodes: String

    init(nodes: String) {
        self.algorithm = "sha256"
        self.nodes = nodes
    }
}

nonisolated struct ScanArchiveNode: Codable, Sendable {
    private enum CodingKeys: String, CodingKey {
        case id = "i"
        case path = "p"
        case name = "n"
        case isDirectory = "d"
        case isSymbolicLink = "s"
        case allocatedSize = "a"
        case unduplicatedAllocatedSize = "u"
        case logicalSize = "l"
        case descendantFileCount = "c"
        case lastModified = "m"
        case fileIdentity = "f"
        case linkCount = "k"
        case isPackage = "g"
        case isAccessible = "r"
        case isSelfAccessible = "q"
        case isSynthetic = "y"
        case isAutoSummarized = "z"
    }

    let id: String
    let path: String?
    let name: String
    let isDirectory: Bool
    let isSymbolicLink: Bool
    let allocatedSize: Int64
    let unduplicatedAllocatedSize: Int64
    let logicalSize: Int64
    let descendantFileCount: Int
    let lastModified: Date?
    let fileIdentity: ScanArchiveFileIdentity?
    let linkCount: UInt64
    let isPackage: Bool
    let isAccessible: Bool
    let isSelfAccessible: Bool
    let isSynthetic: Bool
    let isAutoSummarized: Bool

    init(_ node: FileNodeRecord) {
        self.id = node.id
        self.path = node.isSynthetic || node.url.path != node.id ? node.url.path : nil
        self.name = node.name
        self.isDirectory = node.isDirectory
        self.isSymbolicLink = node.isSymbolicLink
        self.allocatedSize = node.allocatedSize
        self.unduplicatedAllocatedSize = node.unduplicatedAllocatedSize
        self.logicalSize = node.logicalSize
        self.descendantFileCount = node.descendantFileCount
        self.lastModified = node.lastModified
        self.fileIdentity = node.fileIdentity.map(ScanArchiveFileIdentity.init)
        self.linkCount = node.linkCount
        self.isPackage = node.isPackage
        self.isAccessible = node.isAccessible
        self.isSelfAccessible = node.isSelfAccessible
        self.isSynthetic = node.isSynthetic
        self.isAutoSummarized = node.isAutoSummarized
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.id = try container.decode(String.self, forKey: .id)
        self.path = try container.decodeIfPresent(String.self, forKey: .path)
        self.name = try container.decode(String.self, forKey: .name)
        self.isDirectory = try container.decodeIfPresent(Bool.self, forKey: .isDirectory) ?? false
        self.isSymbolicLink = try container.decodeIfPresent(Bool.self, forKey: .isSymbolicLink) ?? false
        self.allocatedSize = try container.decode(Int64.self, forKey: .allocatedSize)
        self.unduplicatedAllocatedSize = try container.decodeIfPresent(
            Int64.self,
            forKey: .unduplicatedAllocatedSize
        ) ?? allocatedSize
        self.logicalSize = try container.decodeIfPresent(Int64.self, forKey: .logicalSize) ?? allocatedSize
        self.descendantFileCount = try container.decodeIfPresent(Int.self, forKey: .descendantFileCount) ??
            (isDirectory ? 0 : 1)
        if let lastModifiedSeconds = try container.decodeIfPresent(Double.self, forKey: .lastModified) {
            self.lastModified = Date(timeIntervalSince1970: lastModifiedSeconds)
        } else {
            self.lastModified = nil
        }
        self.fileIdentity = try container.decodeIfPresent(ScanArchiveFileIdentity.self, forKey: .fileIdentity)
        self.linkCount = try container.decodeIfPresent(UInt64.self, forKey: .linkCount) ?? 1
        self.isPackage = try container.decodeIfPresent(Bool.self, forKey: .isPackage) ?? false
        self.isAccessible = try container.decodeIfPresent(Bool.self, forKey: .isAccessible) ?? true
        self.isSelfAccessible = try container.decodeIfPresent(Bool.self, forKey: .isSelfAccessible) ?? isAccessible
        self.isSynthetic = try container.decodeIfPresent(Bool.self, forKey: .isSynthetic) ?? false
        self.isAutoSummarized = try container.decodeIfPresent(Bool.self, forKey: .isAutoSummarized) ?? false
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(name, forKey: .name)
        if let path {
            try container.encode(path, forKey: .path)
        }
        if isDirectory {
            try container.encode(true, forKey: .isDirectory)
        }
        if isSymbolicLink {
            try container.encode(true, forKey: .isSymbolicLink)
        }
        try container.encode(allocatedSize, forKey: .allocatedSize)
        if unduplicatedAllocatedSize != allocatedSize {
            try container.encode(unduplicatedAllocatedSize, forKey: .unduplicatedAllocatedSize)
        }
        if logicalSize != allocatedSize {
            try container.encode(logicalSize, forKey: .logicalSize)
        }
        let defaultDescendantFileCount = isDirectory ? 0 : 1
        if descendantFileCount != defaultDescendantFileCount {
            try container.encode(descendantFileCount, forKey: .descendantFileCount)
        }
        if let lastModified {
            try container.encode(lastModified.timeIntervalSince1970, forKey: .lastModified)
        }
        if let fileIdentity {
            try container.encode(fileIdentity, forKey: .fileIdentity)
        }
        if linkCount != 1 {
            try container.encode(linkCount, forKey: .linkCount)
        }
        if isPackage {
            try container.encode(true, forKey: .isPackage)
        }
        if !isAccessible {
            try container.encode(false, forKey: .isAccessible)
        }
        if isSelfAccessible != isAccessible {
            try container.encode(isSelfAccessible, forKey: .isSelfAccessible)
        }
        if isSynthetic {
            try container.encode(true, forKey: .isSynthetic)
        }
        if isAutoSummarized {
            try container.encode(true, forKey: .isAutoSummarized)
        }
    }

    func modelNode() throws -> FileNodeRecord {
        guard !id.isEmpty else {
            throw ScanArchiveError.nodes("node has empty ID")
        }
        let resolvedPath = path ?? id
        guard !resolvedPath.isEmpty else {
            throw ScanArchiveError.nodes("node \(id) has empty path")
        }
        guard allocatedSize >= 0, unduplicatedAllocatedSize >= 0, logicalSize >= 0 else {
            throw ScanArchiveError.nodes("node \(id) has negative size")
        }
        guard descendantFileCount >= 0 else {
            throw ScanArchiveError.nodes("node \(id) has negative descendant count")
        }

        let nodeURL = URL(filePath: resolvedPath, directoryHint: isDirectory ? .isDirectory : .notDirectory)
        guard isSynthetic || id == nodeURL.path else {
            throw ScanArchiveError.nodes("node \(id) path does not match ID")
        }

        return FileNodeRecord(
            id: id,
            url: nodeURL,
            name: name,
            isDirectory: isDirectory,
            isSymbolicLink: isSymbolicLink,
            allocatedSize: allocatedSize,
            unduplicatedAllocatedSize: unduplicatedAllocatedSize,
            logicalSize: logicalSize,
            descendantFileCount: descendantFileCount,
            lastModified: lastModified,
            fileIdentity: try fileIdentity?.modelIdentity(),
            linkCount: max(linkCount, 1),
            isPackage: isPackage,
            isAccessible: isAccessible,
            isSelfAccessible: isSelfAccessible,
            isSynthetic: isSynthetic,
            isAutoSummarized: isAutoSummarized
        )
    }
}

nonisolated struct ScanArchiveFileIdentity: Codable, Sendable {
    nonisolated enum Kind: Int, Codable, Sendable {
        case resourceIdentifier = 0
        case fileSystem = 1
    }

    private enum CodingKeys: String, CodingKey {
        case kind = "k"
        case resourceIdentifier = "r"
        case device = "d"
        case inode = "i"
    }

    let kind: Kind
    let resourceIdentifier: String?
    let device: UInt64?
    let inode: UInt64?

    init(_ identity: FileIdentity) {
        switch identity {
        case .resourceIdentifier(let data):
            self.kind = .resourceIdentifier
            self.resourceIdentifier = data.base64EncodedString()
            self.device = nil
            self.inode = nil
        case .fileSystem(let device, let inode):
            self.kind = .fileSystem
            self.resourceIdentifier = nil
            self.device = device
            self.inode = inode
        }
    }

    func modelIdentity() throws -> FileIdentity {
        switch kind {
        case .resourceIdentifier:
            guard let resourceIdentifier,
                  let data = Data(base64Encoded: resourceIdentifier) else {
                throw ScanArchiveError.nodes("file identity has invalid resource identifier")
            }
            return FileIdentity(resourceIdentifier: data)
        case .fileSystem:
            guard let device, let inode else {
                throw ScanArchiveError.nodes("file identity has incomplete file system identity")
            }
            return FileIdentity(device: device, inode: inode)
        }
    }
}

nonisolated struct ScanArchiveTopology: Codable, Sendable {
    private enum CodingKeys: String, CodingKey {
        case rootOrdinal = "r"
        case childOrdinalsByOrdinal = "c"
    }

    let rootOrdinal: Int
    let childOrdinalsByOrdinal: [String: [Int]]

    init(rootOrdinal: Int, childOrdinalsByOrdinal: [String: [Int]]) {
        self.rootOrdinal = rootOrdinal
        self.childOrdinalsByOrdinal = childOrdinalsByOrdinal
    }

    init(_ treeStore: FileTreeStore) throws {
        let orderedNodeIDs = treeStore.indexedNodeIDs()
        var ordinalByID: [String: Int] = [:]
        ordinalByID.reserveCapacity(orderedNodeIDs.count)

        for (ordinal, nodeID) in orderedNodeIDs.enumerated() {
            ordinalByID[nodeID] = ordinal
        }

        guard let rootOrdinal = ordinalByID[treeStore.rootID] else {
            throw ScanArchiveError.topology("root node is missing from node order")
        }

        var childOrdinalsByOrdinal: [String: [Int]] = [:]
        childOrdinalsByOrdinal.reserveCapacity(treeStore.childIDsByID.count)

        for parentID in orderedNodeIDs {
            guard let childIDs = treeStore.childIDsByID[parentID],
                  !childIDs.isEmpty else {
                continue
            }
            guard let parentOrdinal = ordinalByID[parentID] else {
                throw ScanArchiveError.topology("parent \(parentID) is missing from node order")
            }

            var childOrdinals: [Int] = []
            childOrdinals.reserveCapacity(childIDs.count)
            for childID in childIDs {
                guard let childOrdinal = ordinalByID[childID] else {
                    throw ScanArchiveError.topology("child \(childID) is missing from node order")
                }
                childOrdinals.append(childOrdinal)
            }
            childOrdinalsByOrdinal[String(parentOrdinal)] = childOrdinals
        }

        self.rootOrdinal = rootOrdinal
        self.childOrdinalsByOrdinal = childOrdinalsByOrdinal
    }

    func resolvedTopology(orderedNodeIDs: [String]) throws -> ScanArchiveResolvedTopology {
        guard orderedNodeIDs.indices.contains(rootOrdinal) else {
            throw ScanArchiveError.topology("root ordinal \(rootOrdinal) is out of range")
        }

        var childIDsByID: [String: [String]] = [:]
        childIDsByID.reserveCapacity(childOrdinalsByOrdinal.count)

        for (parentOrdinalKey, childOrdinals) in childOrdinalsByOrdinal {
            guard let parentOrdinal = Int(parentOrdinalKey),
                  String(parentOrdinal) == parentOrdinalKey else {
                throw ScanArchiveError.topology("parent ordinal \(parentOrdinalKey) is invalid")
            }
            guard orderedNodeIDs.indices.contains(parentOrdinal) else {
                throw ScanArchiveError.topology("parent ordinal \(parentOrdinal) is out of range")
            }

            var childIDs: [String] = []
            childIDs.reserveCapacity(childOrdinals.count)
            for childOrdinal in childOrdinals {
                guard orderedNodeIDs.indices.contains(childOrdinal) else {
                    throw ScanArchiveError.topology("child ordinal \(childOrdinal) is out of range")
                }
                childIDs.append(orderedNodeIDs[childOrdinal])
            }
            childIDsByID[orderedNodeIDs[parentOrdinal]] = childIDs
        }

        return ScanArchiveResolvedTopology(
            rootID: orderedNodeIDs[rootOrdinal],
            childIDsByID: childIDsByID
        )
    }
}

nonisolated struct ScanArchiveWarningV1: Codable, Sendable {
    let path: String
    let message: String
    let category: String

    init(_ warning: ScanWarning) {
        self.path = warning.path
        self.message = warning.message
        self.category = warning.category.rawValue
    }

    func modelWarning() throws -> ScanWarning {
        guard let category = ScanWarningCategory(rawValue: category) else {
            throw ScanArchiveError.manifest("unknown warning category \(category)")
        }
        return ScanWarning(path: path, message: message, category: category)
    }
}

nonisolated struct ScanArchiveStatsV1: Codable, Sendable {
    let totalAllocatedSize: Int64
    let totalLogicalSize: Int64
    let fileCount: Int
    let directoryCount: Int
    let accessibleItemCount: Int
    let inaccessibleItemCount: Int

    init(_ stats: ScanAggregateStats) {
        self.totalAllocatedSize = stats.totalAllocatedSize
        self.totalLogicalSize = stats.totalLogicalSize
        self.fileCount = stats.fileCount
        self.directoryCount = stats.directoryCount
        self.accessibleItemCount = stats.accessibleItemCount
        self.inaccessibleItemCount = stats.inaccessibleItemCount
    }

    func matches(_ stats: ScanAggregateStats) -> Bool {
        totalAllocatedSize == stats.totalAllocatedSize &&
            totalLogicalSize == stats.totalLogicalSize &&
            fileCount == stats.fileCount &&
            directoryCount == stats.directoryCount &&
            accessibleItemCount == stats.accessibleItemCount &&
            inaccessibleItemCount == stats.inaccessibleItemCount
    }
}
