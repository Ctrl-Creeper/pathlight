//
//  FileBrowserSearch.swift
//  Pathlight
//

import Foundation

protocol FileSearching: Sendable {
    func search(
        snapshotID: UUID,
        treeStore: FileTreeStore,
        normalizedQuery: String,
        includesPath: Bool,
        sortOrder: [FileNodeTableComparator]
    ) async throws -> [FileNodeRecord]

    func pruneIndexes(keeping snapshotID: UUID?) async
}

extension FileSearching {
    func pruneIndexes(keeping snapshotID: UUID?) async {}
}

actor CurrentContentsSearchService {
    func filteredAndSortedCurrentContents(
        _ nodes: [FileNodeRecord],
        searchText: String,
        sortOrder: [FileNodeTableComparator],
        fileTreeStore: FileTreeStore?,
        debounceDuration: Duration
    ) async throws -> [FileNodeRecord] {
        try await Task.sleep(for: debounceDuration)
        return try FileBrowserResults.filteredAndSortedCurrentContents(
            nodes,
            searchText: searchText,
            sortOrder: sortOrder,
            fileTreeStore: fileTreeStore,
            cancellationCheck: {
                try Task.checkCancellation()
            }
        )
    }
}

actor FileSearchService: FileSearching {
    private var indexes: [UUID: FileSearchIndex] = [:]

    func search(
        snapshotID: UUID,
        treeStore: FileTreeStore,
        normalizedQuery: String,
        includesPath: Bool,
        sortOrder: [FileNodeTableComparator]
    ) async throws -> [FileNodeRecord] {
        guard !normalizedQuery.isEmpty else { return [] }

        var index: FileSearchIndex
        if let cachedIndex = indexes[snapshotID] {
            index = cachedIndex
        } else {
            index = try await makeIndex(treeStore: treeStore)
            indexes = [snapshotID: index]
        }

        var matchedNodes: [FileNodeRecord] = []
        matchedNodes.reserveCapacity(min(index.entries.count, 256))

        for (offset, entry) in index.entries.enumerated() {
            if offset.isMultiple(of: 256) {
                try Task.checkCancellation()
            }

            if entry.normalizedNameKindHaystack.contains(normalizedQuery) {
                if let node = treeStore.nodesByID[entry.id] {
                    matchedNodes.append(node)
                }
                continue
            }

            guard includesPath else { continue }

            let normalizedPath: String
            if let cachedPath = index.normalizedPathsByID[entry.id] {
                normalizedPath = cachedPath
            } else {
                normalizedPath = SearchNormalizer.normalize(treeStore.nodesByID[entry.id]?.url.path ?? "")
                index.normalizedPathsByID[entry.id] = normalizedPath
            }

            if normalizedPath.contains(normalizedQuery) {
                if let node = treeStore.nodesByID[entry.id] {
                    matchedNodes.append(node)
                }
            }
        }

        if includesPath {
            indexes[snapshotID] = index
        }

        try Task.checkCancellation()
        let sortedNodes = try FileBrowserResults.sorted(
            matchedNodes,
            sortOrder: sortOrder,
            fileTreeStore: treeStore,
            cancellationCheck: {
                try Task.checkCancellation()
            }
        )
        try Task.checkCancellation()
        return sortedNodes
    }

    func pruneIndexes(keeping snapshotID: UUID?) {
        guard let snapshotID else {
            indexes.removeAll()
            return
        }

        indexes = indexes.filter { $0.key == snapshotID }
    }

    private func makeIndex(treeStore: FileTreeStore) async throws -> FileSearchIndex {
        var entries: [FileSearchEntry] = []
        entries.reserveCapacity(max(treeStore.nodeCount - 1, 0))

        var offset = 0
        try treeStore.forEachIndexedNodeID(excludingRoot: true) { id in
            if offset.isMultiple(of: 256) {
                try Task.checkCancellation()
            }
            offset += 1

            guard let node = treeStore.nodesByID[id] else { return }
            entries.append(FileSearchEntry(
                id: id,
                normalizedNameKindHaystack: SearchNormalizer.normalizedNameKindHaystack(for: node)
            ))
        }

        return FileSearchIndex(
            entries: entries,
            normalizedPathsByID: [:]
        )
    }
}

enum SearchNormalizer {
    nonisolated static func normalize(_ value: String) -> String {
        value
            .folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
            .lowercased()
    }

    nonisolated static func queryIncludesPath(_ query: String) -> Bool {
        query.contains("/") || query.contains("\\")
    }

    nonisolated static func nodeMatches(
        _ node: FileNodeRecord,
        normalizedQuery: String,
        includesPath: Bool
    ) -> Bool {
        if normalizedNameKindHaystack(for: node).contains(normalizedQuery) {
            return true
        }

        guard includesPath else { return false }
        return normalize(node.url.path).contains(normalizedQuery)
    }

    nonisolated static func normalizedNameKindHaystack(for node: FileNodeRecord) -> String {
        normalize([node.name, node.itemKind].joined(separator: "\n"))
    }
}

private struct FileSearchIndex {
    let entries: [FileSearchEntry]
    var normalizedPathsByID: [FileNodeRecord.ID: String]
}

private struct FileSearchEntry {
    let id: FileNodeRecord.ID
    let normalizedNameKindHaystack: String
}
