import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct DiscardPileDragPayload: Codable, Hashable, Transferable {
    static let contentType = UTType(exportedAs: "dev.ctrlcreeper.pathlight.discard-pile-drag-payload")

    let snapshotID: UUID
    let nodeIDs: [FileNodeRecord.ID]

    static var transferRepresentation: some TransferRepresentation {
        CodableRepresentation(contentType: contentType)
    }
}

struct WorkspaceActions {
    let chooseFolder: () -> Void
    let startScan: (ScanTarget) -> Void
    let stopScan: () -> Void
    let rescan: () -> Void
    let handleDroppedURLs: ([URL]) -> Bool
    let selectNodeImmediately: (String?) -> Void
    let selectNode: (String?) -> Void
    let selectNodesImmediately: (Set<String>, String?) -> Void
    let selectNodes: (Set<String>, String?) -> Void
    let focusNode: (String?) -> Void
    let selectAndFocusNode: (String) -> Void
    let navigateBack: () -> Void
    let navigateForward: () -> Void
    let navigateToParent: () -> Void
    let expandSummarizedNode: (FileNodeRecord) -> Void
    let zoomIntoSelection: () -> Void
    let recordSunburstSegmentClick: () -> Void
    let selectedFileActions: SelectedFileActions
    let bulkFileActions: BulkFileActions
    let startShortTermWatch: (URL) -> Void
    let stopShortTermWatch: () -> Void
    let refreshActivityHistory: (URL) -> Void
    let enableLongTermWatch: (URL) -> Void
    let openFullDiskAccessSettings: () -> Void
    let setDiscardPileDragActive: (Bool) -> Void
    let setDiscardPileDragActiveAfterThreshold: (Bool) -> Void
}

struct SelectedFileActions {
    let quickLook: () -> Void
    let revealInFinder: () -> Void
    let open: () -> Void
    let copyPath: () -> Void
    let moveToTrash: () -> Void

    func perform(_ action: FileNodeAction) {
        switch action {
        case .quickLook:
            quickLook()
        case .revealInFinder:
            revealInFinder()
        case .open:
            open()
        case .copyPath:
            copyPath()
        case .moveToTrash:
            moveToTrash()
        }
    }
}

struct BulkFileActions {
    let revealInFinder: ([FileNodeRecord]) -> Void
    let copyPaths: ([FileNodeRecord]) -> Void
    let addToDiscardPile: ([FileNodeRecord]) -> Void
    let moveToTrash: ([FileNodeRecord]) -> Void
}

struct WorkspaceView: View {
    @ObservedObject var scanState: ScanCoordinator
    @ObservedObject var navigation: WorkspaceNavigationModel
    @Binding var isInspectorPresented: Bool
    @FocusState.Binding var focusedWorkspaceTarget: WorkspaceFocusTarget?

    let maxRenderedDepth: Int
    let showFreeSpaceInSunburst: Bool
    let discardPileHiddenNodeIDs: Set<FileNodeRecord.ID>
    let startupDiskTarget: ScanTarget?
    let liveWatchSession: WatchSessionModel?
    let activityHistory: ActivityHistorySnapshot?
    let longTermWatchTargets: [LongTermWatchTarget]
    let fullDiskAccessStatus: FullDiskAccessStatus
    let freeSpaceAvailableCapacity: (ScanSnapshot, FileNodeRecord) -> Int64?
    let actions: WorkspaceActions

    var body: some View {
        VStack(spacing: 0) {
            workspaceContent

            if let liveWatchSession {
                Divider()
                ActivityTimelinePanel(
                    session: liveWatchSession,
                    onStop: actions.stopShortTermWatch
                )
            }

            if let visibleActivityHistory {
                Divider()
                ActivityHistoryPanel(snapshot: visibleActivityHistory)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .hidingWindowToolbarBackgroundWhenAvailable()
        .toolbar {
            ToolbarItem(placement: .automatic) { Spacer() }
            ToolbarItemGroup(placement: .automatic) {
                Button {
                    actions.chooseFolder()
                } label: {
                    Label("Choose Folder", systemImage: "folder.badge.plus")
                }
                .disabled(scanState.isScanning)
                .help("Choose Folder")

                if let watchRoot {
                    if liveWatchSession == nil {
                        Button {
                            actions.startShortTermWatch(watchRoot)
                        } label: {
                            Label("Watch", systemImage: "waveform.path.ecg")
                        }
                        .disabled(scanState.isScanning)
                        .help("Watch File Activity")
                    } else {
                        Button {
                            actions.stopShortTermWatch()
                        } label: {
                            Label("Stop Watching", systemImage: "stop.circle")
                        }
                        .help("Stop Watching")
                    }

                    Button {
                        actions.refreshActivityHistory(watchRoot)
                    } label: {
                        Label("Refresh Activity History", systemImage: "clock.arrow.circlepath")
                    }
                    .disabled(scanState.isScanning)
                    .help("Refresh Activity History")

                    if !isWatchRootTracked {
                        Button {
                            actions.enableLongTermWatch(watchRoot)
                        } label: {
                            Label("Track", systemImage: "clock.badge.plus")
                        }
                        .disabled(scanState.isScanning)
                        .help("Track Long-Term Activity")
                    }
                }

                if scanState.canStopScan {
                    Button {
                        actions.stopScan()
                    } label: {
                        Label("Stop", systemImage: "stop.fill")
                    }
                    .help("Stop Scan")
                } else {
                    Button {
                        actions.rescan()
                    } label: {
                        Label("Rescan", systemImage: "arrow.clockwise")
                    }
                    .disabled(!scanState.canRescan)
                    .help("Rescan")
                }
            }
            ToolbarItem(placement: .automatic) { Spacer() }
            ToolbarItem(placement: .automatic) {
                Button {
                    isInspectorPresented.toggle()
                } label: {
                    Label(inspectorToggleTitle, systemImage: "sidebar.trailing")
                }
                .labelStyle(.iconOnly)
                .help(inspectorToggleTitle)
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            actions.handleDroppedURLs(urls)
        }
        .task(id: watchRoot?.standardizedFileURL.path) {
            if let watchRoot {
                actions.refreshActivityHistory(watchRoot)
            }
        }
    }

    @ViewBuilder
    private var workspaceContent: some View {
        if let snapshot = scanState.snapshot,
           let focusNode = navigation.currentFocusNode {
            ActiveWorkspaceView(
                scanState: scanState,
                navigation: navigation,
                snapshot: snapshot,
                focusNode: focusNode,
                focusedWorkspaceTarget: $focusedWorkspaceTarget,
                maxRenderedDepth: maxRenderedDepth,
                showFreeSpaceInSunburst: showFreeSpaceInSunburst,
                discardPileHiddenNodeIDs: discardPileHiddenNodeIDs,
                fullDiskAccessStatus: fullDiskAccessStatus,
                freeSpaceAvailableCapacity: freeSpaceAvailableCapacity,
                actions: actions
            )
        } else if scanState.isScanning {
            ScanningWorkspaceState(
                progress: scanState.progress,
                selectedTarget: scanState.selectedTarget,
                actions: actions
            )
        } else {
            EmptyWorkspaceState(
                startupDiskTarget: startupDiskTarget,
                actions: actions
            )
        }
    }

    private var watchRoot: URL? {
        scanState.snapshot?.target.url ?? scanState.selectedTarget?.url
    }

    private var visibleActivityHistory: ActivityHistorySnapshot? {
        guard let watchRoot,
              let activityHistory,
              activityHistory.rootPath == watchRoot.standardizedFileURL,
              activityHistory.eventCount > 0 else {
            return nil
        }

        return activityHistory
    }

    private var isWatchRootTracked: Bool {
        guard let watchRoot else {
            return false
        }

        let rootPath = watchRoot.standardizedFileURL.path
        return longTermWatchTargets.contains { target in
            target.rootPath.standardizedFileURL.path == rootPath
        }
    }
}

private extension WorkspaceView {
    var inspectorToggleTitle: String {
        isInspectorPresented ? "Hide Inspector" : "Show Inspector"
    }
}

private extension View {
    @ViewBuilder
    func hidingWindowToolbarBackgroundWhenAvailable() -> some View {
        if #available(macOS 15.0, *) {
            toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        } else {
            self
        }
    }
}
