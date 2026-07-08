//
//  FileBrowserResults.swift
//  Pathlight
//

import Foundation

enum FileBrowserResults {
    nonisolated static func filteredAndSortedCurrentContents(
        _ nodes: [FileNodeRecord],
        searchText: String,
        sortOrder: [FileNodeTableComparator],
        fileTreeStore: FileTreeStore? = nil
    ) -> [FileNodeRecord] {
        (try? filteredAndSortedCurrentContents(
            nodes,
            searchText: searchText,
            sortOrder: sortOrder,
            fileTreeStore: fileTreeStore,
            cancellationCheck: {}
        )) ?? []
    }

    nonisolated static func filteredAndSortedCurrentContents(
        _ nodes: [FileNodeRecord],
        searchText: String,
        sortOrder: [FileNodeTableComparator],
        fileTreeStore: FileTreeStore? = nil,
        cancellationCheck: @Sendable () throws -> Void
    ) throws -> [FileNodeRecord] {
        let trimmedSearchText = searchText.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !trimmedSearchText.isEmpty else {
            try cancellationCheck()
            return try sorted(
                nodes,
                sortOrder: sortOrder,
                fileTreeStore: fileTreeStore,
                cancellationCheck: cancellationCheck
            )
        }

        let normalizedSearchText = SearchNormalizer.normalize(trimmedSearchText)
        let includesPath = SearchNormalizer.queryIncludesPath(trimmedSearchText)
        var filteredNodes: [FileNodeRecord] = []
        filteredNodes.reserveCapacity(min(nodes.count, 256))

        for (offset, node) in nodes.enumerated() {
            if offset.isMultiple(of: 256) {
                try cancellationCheck()
            }

            if SearchNormalizer.nodeMatches(
                node,
                normalizedQuery: normalizedSearchText,
                includesPath: includesPath
            ) {
                filteredNodes.append(node)
            }
        }

        try cancellationCheck()
        return try sorted(
            filteredNodes,
            sortOrder: sortOrder,
            fileTreeStore: fileTreeStore,
            cancellationCheck: cancellationCheck
        )
    }

    nonisolated static func sorted(
        _ nodes: [FileNodeRecord],
        sortOrder: [FileNodeTableComparator],
        fileTreeStore: FileTreeStore? = nil
    ) -> [FileNodeRecord] {
        (try? sorted(
            nodes,
            sortOrder: sortOrder,
            fileTreeStore: fileTreeStore,
            cancellationCheck: {}
        )) ?? nodes
    }

    nonisolated static func sorted(
        _ nodes: [FileNodeRecord],
        sortOrder: [FileNodeTableComparator],
        fileTreeStore: FileTreeStore? = nil,
        cancellationCheck: @Sendable () throws -> Void
    ) throws -> [FileNodeRecord] {
        try cancellationCheck()
        guard !sortOrder.isEmpty else { return nodes }

        let preparedNodes = try preparedSortNodes(
            nodes,
            fileTreeStore: fileTreeStore,
            cancellationCheck: cancellationCheck
        )
        try cancellationCheck()

        let sortedNodes = preparedNodes.sorted { lhs, rhs in
            for comparator in sortOrder {
                switch lhs.compare(rhs, using: comparator) {
                case .orderedAscending:
                    return true
                case .orderedDescending:
                    return false
                case .orderedSame:
                    continue
                @unknown default:
                    continue
                }
            }
            return false
        }
        try cancellationCheck()
        return sortedNodes.map(\.node)
    }

    private nonisolated static func preparedSortNodes(
        _ nodes: [FileNodeRecord],
        fileTreeStore: FileTreeStore?,
        cancellationCheck: @Sendable () throws -> Void
    ) throws -> [PreparedSortNode] {
        var preparedNodes: [PreparedSortNode] = []
        preparedNodes.reserveCapacity(nodes.count)

        for (offset, node) in nodes.enumerated() {
            if offset.isMultiple(of: 256) {
                try cancellationCheck()
            }
            preparedNodes.append(PreparedSortNode(node: node, fileTreeStore: fileTreeStore))
        }

        return preparedNodes
    }

    private struct PreparedSortNode {
        let node: FileNodeRecord
        let name: String
        let id: String
        let allocatedSize: Int64
        let itemKind: String
        let descendantFileCount: Int
        let lastModified: Date?

        nonisolated init(node: FileNodeRecord, fileTreeStore: FileTreeStore?) {
            self.node = node
            self.name = node.name
            self.id = node.id
            self.allocatedSize = node.allocatedSize
            self.itemKind = node.itemKind
            self.descendantFileCount = FileBrowserPackageContents.areHidden(for: node, fileTreeStore: fileTreeStore)
                ? 0
                : node.descendantFileCount
            self.lastModified = node.lastModified
        }

        nonisolated func compare(_ rhs: PreparedSortNode, using comparator: FileNodeTableComparator) -> ComparisonResult {
            let result: ComparisonResult = switch comparator.field {
            case .name:
                name.localizedStandardCompare(rhs.name)
            case .allocatedSize:
                FileNodeSortComparison.compare(allocatedSize, rhs.allocatedSize)
            case .itemKind:
                itemKind.localizedStandardCompare(rhs.itemKind)
            case .descendantFileCount:
                FileNodeSortComparison.compare(descendantFileCount, rhs.descendantFileCount)
            case .lastModified:
                FileNodeSortComparison.compareOptional(lastModified, rhs.lastModified)
            }

            let orderedResult = FileNodeSortComparison.applying(comparator.order, to: result)
            switch orderedResult {
            case .orderedSame:
                return FileNodeSortComparison.fallback(
                    lhsName: name,
                    lhsID: id,
                    rhsName: rhs.name,
                    rhsID: rhs.id
                )
            default:
                return orderedResult
            }
        }
    }
}
