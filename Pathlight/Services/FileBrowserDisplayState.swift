//
//  FileBrowserDisplayState.swift
//  Pathlight
//

import Foundation

struct FileBrowserDisplayRequest: Sendable {
    let generation: Int
    let displayContext: FileBrowserDisplayContext
}

struct FileBrowserDisplayContext: Equatable, Sendable {
    let contentID: String
    let contentRevision: Int
    let snapshotID: UUID?
    let searchScope: FileBrowserFindTarget
    let searchText: String
    let sortOrder: [FileNodeTableComparator]
    let hiddenNodeIDs: Set<FileNodeRecord.ID>

    static let empty = FileBrowserDisplayContext(
        contentID: "",
        contentRevision: 0,
        snapshotID: nil,
        searchScope: .currentContents,
        searchText: "",
        sortOrder: [],
        hiddenNodeIDs: []
    )
}

struct FileBrowserDisplayState {
    var nodes: [FileNodeRecord]
    var context: FileBrowserDisplayContext
    var indexesByNodeID: [FileNodeRecord.ID: Int]
    var displayValueCache: FileBrowserDisplayValueCache

    init(
        nodes: [FileNodeRecord] = [],
        context: FileBrowserDisplayContext = .empty
    ) {
        var uniqueNodes: [FileNodeRecord] = []
        var indexesByNodeID: [FileNodeRecord.ID: Int] = [:]
        uniqueNodes.reserveCapacity(nodes.count)
        indexesByNodeID.reserveCapacity(nodes.count)

        for node in nodes where indexesByNodeID[node.id] == nil {
            indexesByNodeID[node.id] = uniqueNodes.count
            uniqueNodes.append(node)
        }

        self.nodes = uniqueNodes
        self.context = context
        self.indexesByNodeID = indexesByNodeID
        self.displayValueCache = FileBrowserDisplayValueCache()
    }

    func node(id: FileNodeRecord.ID) -> FileNodeRecord? {
        guard let index = indexesByNodeID[id],
              nodes.indices.contains(index) else {
            return nil
        }
        return nodes[index]
    }

    func displayValues(
        for node: FileNodeRecord,
        hidesPackageContents: Bool = false
    ) -> FileBrowserNodeDisplayValues {
        let cacheKey = FileBrowserDisplayValueCacheKey(
            nodeID: node.id,
            hidesPackageContents: hidesPackageContents
        )
        if let cachedValues = displayValueCache.valuesByKey[cacheKey] {
            return cachedValues
        }

        let values = FileBrowserNodeDisplayValues(
            node: node,
            hidesPackageContents: hidesPackageContents
        )
        displayValueCache.valuesByKey[cacheKey] = values
        return values
    }
}

final class FileBrowserDisplayValueCache {
    var valuesByKey: [FileBrowserDisplayValueCacheKey: FileBrowserNodeDisplayValues] = [:]
}

struct FileBrowserDisplayValueCacheKey: Hashable {
    let nodeID: FileNodeRecord.ID
    let hidesPackageContents: Bool
}

enum FileBrowserPackageContents {
    nonisolated static func areHidden(
        for node: FileNodeRecord,
        fileTreeStore: FileTreeStore?
    ) -> Bool {
        node.isPackage &&
            node.isDirectory &&
            !node.isAutoSummarized &&
            (node.descendantFileCount > 0 || node.allocatedSize > 0 || node.logicalSize > 0) &&
            fileTreeStore?.containsChildren(id: node.id) != true
    }
}

struct FileBrowserNodeDisplayValues: Equatable, Sendable {
    let allocatedSize: String
    let descendantCount: String
    let modifiedDate: String

    init(node: FileNodeRecord, hidesPackageContents: Bool = false) {
        allocatedSize = PathlightFormatters.size(node.allocatedSize)
        descendantCount = Self.descendantCountText(
            for: node,
            hidesPackageContents: hidesPackageContents
        )
        modifiedDate = PathlightFormatters.date(node.lastModified)
    }

    private static func descendantCountText(
        for node: FileNodeRecord,
        hidesPackageContents: Bool
    ) -> String {
        if hidesPackageContents && node.isPackage {
            return "—"
        }
        if node.isDirectory {
            return "\(node.descendantFileCount)"
        }
        if node.isSynthetic || node.isSymbolicLink {
            return "—"
        }
        return "1"
    }
}
