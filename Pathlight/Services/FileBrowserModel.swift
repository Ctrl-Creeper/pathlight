//
//  FileBrowserModel.swift
//  Pathlight
//

import Combine
import Foundation

enum FileBrowserFindTarget: Equatable, Sendable {
    case currentContents
    case entireScan
}

@MainActor
final class FileBrowserModel: ObservableObject {
    @Published private(set) var currentContentsSearchText = ""
    @Published private(set) var entireScanSearchText = ""
    @Published private(set) var searchScope: FileBrowserFindTarget = .currentContents
    @Published private(set) var sortOrder = [FileNodeTableComparator(field: .allocatedSize, order: .reverse)]
    @Published private(set) var isSearchingEntireScan = false
    @Published private(set) var isRefreshingCurrentContents = false
    @Published private var displayState = FileBrowserDisplayState()

    private let searchService: any FileSearching
    private let currentContentsService = CurrentContentsSearchService()
    private let searchDebounceDuration: Duration
    private let currentContentsAsyncThreshold: Int
    private var searchTask: Task<Void, Never>?
    private var searchIndexPruneTask: Task<Void, Never>?
    private var searchGeneration = 0
    private var nodes: [FileNodeRecord] = []
    private var contentID = ""
    private var contentRevision = 0
    private var snapshotID: UUID?
    private var fileTreeStore: FileTreeStore?
    private var hiddenNodeIDs: Set<FileNodeRecord.ID> = []
    private var needsRefreshAfterCleanup = false

    init(
        searchService: any FileSearching = FileSearchService(),
        searchDebounceDuration: Duration = .milliseconds(180),
        currentContentsAsyncThreshold: Int = 512
    ) {
        self.searchService = searchService
        self.searchDebounceDuration = searchDebounceDuration
        self.currentContentsAsyncThreshold = currentContentsAsyncThreshold
    }

    deinit {
        searchTask?.cancel()
        searchIndexPruneTask?.cancel()
    }

    var activeSearchText: String {
        switch searchScope {
        case .currentContents:
            currentContentsSearchText
        case .entireScan:
            entireScanSearchText
        }
    }

    var displayedNodes: [FileNodeRecord] {
        displayState.nodes
    }

    var isDisplayingCurrentResults: Bool {
        displayState.context == currentDisplayContext
    }

    func displayedNode(id: FileNodeRecord.ID) -> FileNodeRecord? {
        displayState.node(id: id)
    }

    func displayValues(
        for node: FileNodeRecord,
        hidesPackageContents: Bool = false
    ) -> FileBrowserNodeDisplayValues {
        displayState.displayValues(for: node, hidesPackageContents: hidesPackageContents)
    }

    func packageContentsAreHidden(for node: FileNodeRecord) -> Bool {
        FileBrowserPackageContents.areHidden(for: node, fileTreeStore: fileTreeStore)
    }

    var isShowingEntireScanResults: Bool {
        searchScope == .entireScan && !trimmedEntireScanSearchText.isEmpty
    }

    var isFilteringCurrentContents: Bool {
        searchScope == .currentContents && !trimmedCurrentContentsSearchText.isEmpty
    }

    func updateContent(
        nodes: [FileNodeRecord],
        contentID: String,
        snapshot: ScanSnapshot?,
        fileTreeStore: FileTreeStore?,
        hiddenNodeIDs: Set<FileNodeRecord.ID> = [],
        forceRefresh: Bool = false
    ) {
        let nextSnapshotID = snapshot?.id
        let previousSnapshotID = snapshotID
        guard forceRefresh ||
            needsRefreshAfterCleanup ||
            self.contentID != contentID ||
            snapshotID != nextSnapshotID ||
            self.hiddenNodeIDs != hiddenNodeIDs ||
            self.nodes != nodes else {
            return
        }

        needsRefreshAfterCleanup = false
        contentRevision += 1
        self.nodes = nodes
        self.contentID = contentID
        snapshotID = nextSnapshotID
        self.fileTreeStore = fileTreeStore
        self.hiddenNodeIDs = hiddenNodeIDs
        pruneSearchIndexesIfNeeded(previousSnapshotID: previousSnapshotID, nextSnapshotID: nextSnapshotID)
        refreshDisplayedNodes()
    }

    func setSearchScope(_ scope: FileBrowserFindTarget) {
        guard searchScope != scope else { return }
        searchScope = scope
        refreshDisplayedNodes()
    }

    func setActiveSearchText(_ text: String) {
        switch searchScope {
        case .currentContents:
            guard currentContentsSearchText != text else { return }
            currentContentsSearchText = text
        case .entireScan:
            guard entireScanSearchText != text else { return }
            entireScanSearchText = text
        }
        refreshDisplayedNodes()
    }

    func setSortOrder(_ order: [FileNodeTableComparator]) {
        guard sortOrder != order else { return }
        sortOrder = order
        refreshDisplayedNodes()
    }

    func cleanup() {
        let canceledDisplayRefresh = isSearchingEntireScan || isRefreshingCurrentContents
        cancelPendingSearch(clearLoading: true)
        needsRefreshAfterCleanup = needsRefreshAfterCleanup || canceledDisplayRefresh
        searchIndexPruneTask?.cancel()
        searchIndexPruneTask = nil
    }

    private func pruneSearchIndexesIfNeeded(previousSnapshotID: UUID?, nextSnapshotID: UUID?) {
        guard previousSnapshotID != nil,
              previousSnapshotID != nextSnapshotID else { return }

        searchIndexPruneTask?.cancel()
        searchIndexPruneTask = Task { [searchService] in
            guard !Task.isCancelled else { return }
            await searchService.pruneIndexes(keeping: nextSnapshotID)
        }
    }

    private func cancelPendingSearch(clearLoading: Bool) {
        searchGeneration += 1
        searchTask?.cancel()
        searchTask = nil
        if clearLoading {
            setIsSearchingEntireScan(false)
            setIsRefreshingCurrentContents(false)
        }
    }

    private var trimmedCurrentContentsSearchText: String {
        currentContentsSearchText.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var trimmedEntireScanSearchText: String {
        entireScanSearchText.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var currentDisplayContext: FileBrowserDisplayContext {
        FileBrowserDisplayContext(
            contentID: contentID,
            contentRevision: contentRevision,
            snapshotID: snapshotID,
            searchScope: searchScope,
            searchText: activeTrimmedSearchText,
            sortOrder: sortOrder,
            hiddenNodeIDs: hiddenNodeIDs
        )
    }

    private var activeTrimmedSearchText: String {
        switch searchScope {
        case .currentContents:
            trimmedCurrentContentsSearchText
        case .entireScan:
            trimmedEntireScanSearchText
        }
    }

    private func refreshDisplayedNodes() {
        cancelPendingSearch(clearLoading: false)

        if isShowingEntireScanResults {
            setIsRefreshingCurrentContents(false)
            scheduleEntireScanSearch()
        } else {
            setIsSearchingEntireScan(false)
            rebuildCurrentContentsResults()
        }
    }

    private func rebuildCurrentContentsResults() {
        let searchText = isFilteringCurrentContents ? trimmedCurrentContentsSearchText : ""
        let displayContext = currentDisplayContext
        let visibleNodes = Self.visibleNodes(
            nodes,
            hiddenNodeIDs: hiddenNodeIDs,
            fileTreeStore: fileTreeStore
        )

        guard shouldRefreshCurrentContentsAsynchronously(visibleNodes) else {
            setIsRefreshingCurrentContents(false)
            applyDisplayedNodes(
                FileBrowserResults.filteredAndSortedCurrentContents(
                    visibleNodes,
                    searchText: searchText,
                    sortOrder: sortOrder,
                    fileTreeStore: fileTreeStore
                ),
                context: displayContext
            )
            return
        }

        scheduleCurrentContentsRefresh(
            nodes: visibleNodes,
            searchText: searchText,
            displayContext: displayContext
        )
    }

    private func shouldRefreshCurrentContentsAsynchronously(_ nodes: [FileNodeRecord]) -> Bool {
        !nodes.isEmpty && nodes.count >= currentContentsAsyncThreshold
    }

    private func scheduleCurrentContentsRefresh(
        nodes: [FileNodeRecord],
        searchText: String,
        displayContext: FileBrowserDisplayContext
    ) {
        let sortOrder = sortOrder
        let fileTreeStore = fileTreeStore
        let request = FileBrowserDisplayRequest(
            generation: searchGeneration,
            displayContext: displayContext
        )
        let debounceDuration = searchText.isEmpty ? Duration.zero : searchDebounceDuration

        setIsRefreshingCurrentContents(true)
        searchTask = Task { [currentContentsService] in
            do {
                let refreshedNodes = try await currentContentsService.filteredAndSortedCurrentContents(
                    nodes,
                    searchText: searchText,
                    sortOrder: sortOrder,
                    fileTreeStore: fileTreeStore,
                    debounceDuration: debounceDuration
                )
                try Task.checkCancellation()

                await MainActor.run { [weak self] in
                    guard let self,
                          isCurrent(request) else {
                        return
                    }

                    applyDisplayedNodes(refreshedNodes, context: request.displayContext)
                    setIsRefreshingCurrentContents(false)
                }
            } catch is CancellationError {
                return
            } catch {
                await MainActor.run { [weak self] in
                    guard let self,
                          isCurrent(request) else {
                        return
                    }

                    setIsRefreshingCurrentContents(false)
                }
            }
        }
    }

    private func scheduleEntireScanSearch() {
        guard let snapshotID, let fileTreeStore else {
            setIsSearchingEntireScan(false)
            applyDisplayedNodes([], context: currentDisplayContext)
            return
        }

        let searchText = trimmedEntireScanSearchText
        guard !searchText.isEmpty else {
            setIsSearchingEntireScan(false)
            rebuildCurrentContentsResults()
            return
        }

        let normalizedSearchText = SearchNormalizer.normalize(searchText)
        let includesPath = SearchNormalizer.queryIncludesPath(searchText)
        let sortOrder = sortOrder
        let debounceDuration = searchDebounceDuration
        let hiddenNodeIDs = hiddenNodeIDs
        let request = FileBrowserDisplayRequest(
            generation: searchGeneration,
            displayContext: currentDisplayContext
        )

        setIsSearchingEntireScan(true)
        searchTask = Task { [searchService] in
            do {
                try await Task.sleep(for: debounceDuration)
                let matchedNodes = try await searchService.search(
                    snapshotID: snapshotID,
                    treeStore: fileTreeStore,
                    normalizedQuery: normalizedSearchText,
                    includesPath: includesPath,
                    sortOrder: sortOrder
                )
                let visibleMatchedNodes = Self.visibleNodes(
                    matchedNodes,
                    hiddenNodeIDs: hiddenNodeIDs,
                    fileTreeStore: fileTreeStore
                )
                try Task.checkCancellation()

                await MainActor.run { [weak self] in
                    guard let self,
                          isCurrent(request) else {
                        return
                    }

                    applyDisplayedNodes(visibleMatchedNodes, context: request.displayContext)
                    setIsSearchingEntireScan(false)
                }
            } catch is CancellationError {
                return
            } catch {
                await MainActor.run { [weak self] in
                    guard let self,
                          isCurrent(request) else {
                        return
                    }

                    setIsSearchingEntireScan(false)
                    applyDisplayedNodes([], context: request.displayContext)
                }
            }
        }
    }

    private func isCurrent(_ request: FileBrowserDisplayRequest) -> Bool {
        searchGeneration == request.generation &&
            currentDisplayContext == request.displayContext
    }

    private func applyDisplayedNodes(
        _ nodes: [FileNodeRecord],
        context: FileBrowserDisplayContext
    ) {
        displayState = FileBrowserDisplayState(nodes: nodes, context: context)
    }

    private func setIsSearchingEntireScan(_ isSearching: Bool) {
        guard isSearchingEntireScan != isSearching else { return }
        isSearchingEntireScan = isSearching
    }

    private func setIsRefreshingCurrentContents(_ isRefreshing: Bool) {
        guard isRefreshingCurrentContents != isRefreshing else { return }
        isRefreshingCurrentContents = isRefreshing
    }

    private nonisolated static func visibleNodes(
        _ nodes: [FileNodeRecord],
        hiddenNodeIDs: Set<FileNodeRecord.ID>,
        fileTreeStore: FileTreeStore?
    ) -> [FileNodeRecord] {
        guard !hiddenNodeIDs.isEmpty,
              let fileTreeStore else {
            return nodes
        }

        return nodes.filter { node in
            !fileTreeStore.isNodeOrDescendant(node.id, of: hiddenNodeIDs)
        }
    }
}
