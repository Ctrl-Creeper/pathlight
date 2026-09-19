import Charts
import SwiftUI

struct ActivityDashboardActions {
    let refreshActivityHistory: (URL) -> Void
    let refreshActivityDashboardHistories: ([URL]) -> Void
    let narrowActivityHistory: (ActivityHistoryQuery, URL) -> Void
    let setLongTermWatchEnabled: (Bool, URL) -> Void
    let removeLongTermWatchTarget: (URL) -> Void
    let revealInFinder: (URL) -> Void
    let clearHistoryGap: (URL) -> Void
    let setGrowthAlertThreshold: (Int64?, URL) -> Void
    let setRecordingFilters: ([String], Int64, TimeInterval, Int64?, Int64?, URL) -> Void
    let enableLaunchAtLogin: () -> Void
    let dismissLaunchAtLoginNudge: () -> Void
    let addFolder: (MonitoringStartConfiguration) -> Void
    let addPreset: (MonitoringPreset, MonitoringStartConfiguration) -> Void
    let exportHistory: (URL) -> Void
    let startLiveMonitor: (MonitoringStartConfiguration) -> Void
    let stopLiveMonitor: () -> Void
    let stopAllMonitoring: () -> Void
}

struct ActivityDashboardView: View {
    let targets: [LongTermWatchTarget]
    let histories: [ActivityHistorySnapshot]
    let runtimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus]
    var baselineProgress: [String: BaselineProgress] = [:]
    var historyQuery: ActivityHistoryQuery = .everything
    let showsLaunchAtLoginNudge: Bool
    var monitoringStatusMessage: String? = nil
    let isLiveMonitorActive: Bool
    let actions: ActivityDashboardActions

    @State private var selectedTargetID: String?
    @State private var presets = MonitoringPreset.available()
    @State private var monitoringSetup: MonitoringSetup?

    private var untrackedPresets: [MonitoringPreset] {
        let tracked = Set(targets.map(\.id))
        return presets.filter { !tracked.contains($0.rootPath.standardizedFileURL.path) }
    }

    private var selectedRootPath: URL? {
        guard let selectedTargetID else {
            return targets.first?.rootPath
        }

        return targets.first { $0.id == selectedTargetID }?.rootPath ?? targets.first?.rootPath
    }

    private var presentation: ActivityDashboardPresentation {
        ActivityDashboardPresentation(
            targets: targets,
            selectedRootPath: selectedRootPath,
            histories: histories,
            runtimeStatuses: runtimeStatuses
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let monitoringStatusMessage {
                ActivityMonitoringStatusBanner(message: monitoringStatusMessage)
                    .transition(.opacity)
            }
            dashboardContent
        }
        .animation(PathlightMotion.state, value: monitoringStatusMessage)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .toolbar {
            ToolbarItemGroup(placement: .automatic) {
                Menu {
                    Button("Choose Folder…") {
                        monitoringSetup = .longTerm
                    }
                    if !untrackedPresets.isEmpty {
                        Divider()
                        ForEach(untrackedPresets) { preset in
                            Button(preset.title) {
                                monitoringSetup = .preset(preset)
                            }
                        }
                    }
                } label: {
                    Label("Monitor Folder", systemImage: "folder.badge.plus")
                }
                .help("Monitor a Folder Long-Term")


                // ponytail: "stop all" is the row switches pressed at once, so the
                // rows and this button can never disagree the way a shared pause did.
                if targets.contains(where: \.isEnabled) || isLiveMonitorActive {
                    Button {
                        actions.stopAllMonitoring()
                    } label: {
                        Label("Stop All", systemImage: "stop.circle.fill")
                    }
                    .help("Switch every folder off and stop the live monitor")
                }

                if isLiveMonitorActive {
                    Button {
                        actions.stopLiveMonitor()
                    } label: {
                        Label("Stop Live Monitor", systemImage: "stop.circle")
                    }
                    .help("Stop Live Monitor")
                } else {
                    Button {
                        monitoringSetup = .live
                    } label: {
                        Label("Live Monitor", systemImage: "waveform.path.ecg")
                    }
                    .help("Watch a Folder Live")
                }

                if let selectedRootPath {
                    Button {
                        refreshAllTargetHistories()
                    } label: {
                        Label("Refresh", systemImage: "arrow.clockwise")
                    }
                    .help("Refresh Activity")

                    Button {
                        actions.revealInFinder(selectedRootPath)
                    } label: {
                        Label("Reveal", systemImage: "folder")
                    }
                    .help("Reveal in Finder")

                    Button {
                        actions.exportHistory(selectedRootPath)
                    } label: {
                        Label("Export CSV", systemImage: "square.and.arrow.up")
                    }
                    .help("Export this folder's activity history as CSV")
                }
            }
        }
        .onAppear {
            reconcileSelectedTarget()
            refreshAllTargetHistories()
        }
        .onChange(of: targets) { _, _ in
            reconcileSelectedTarget()
            refreshAllTargetHistories()
        }
        .onChange(of: selectedTargetID) { _, _ in
            refreshSelectedHistory()
        }
        .sheet(item: $monitoringSetup) { setup in
            MonitoringStartConfigurationView(setup: setup) { configuration in
                switch setup {
                case .longTerm:
                    actions.addFolder(configuration)
                case let .preset(preset):
                    actions.addPreset(preset, configuration)
                case .live:
                    actions.startLiveMonitor(configuration)
                }
            }
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Label(presentation.title, systemImage: "waveform.path.ecg")
                .font(.title2.weight(.semibold))

            Text(presentation.summaryText)
                .font(.subheadline.monospacedDigit())
                .foregroundStyle(.secondary)

            Spacer(minLength: 12)
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 16)
    }

    @ViewBuilder
    private var dashboardContent: some View {
        if targets.isEmpty {
            ActivityDashboardEmptyState(
                title: presentation.emptyStateTitle ?? "No Activity",
                onAddFolder: { monitoringSetup = .longTerm }
            )
        } else {
            HSplitView {
                targetList
                    .frame(minWidth: 260, idealWidth: 320, maxWidth: 420)

                selectedTargetDetail
                    .frame(minWidth: 460, maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private var targetList: some View {
        ScrollView {
            LazyVStack(spacing: 8) {
                if showsLaunchAtLoginNudge {
                    ActivityLaunchAtLoginNudge(
                        onEnable: actions.enableLaunchAtLogin,
                        onDismiss: actions.dismissLaunchAtLoginNudge
                    )
                    .transition(.opacity)
                }

                ForEach(presentation.targetRows) { row in
                    ActivityDashboardTargetRow(
                        row: row,
                        baselineProgress: baselineProgress[row.id],
                        isSelected: row.id == presentation.selectedTargetID,
                        onSelect: {
                            selectedTargetID = row.id
                        },
                        onSetEnabled: {
                            actions.setLongTermWatchEnabled($0, row.rootPath)
                        },
                        onReveal: {
                            actions.revealInFinder(row.rootPath)
                        },
                        onRemove: {
                            actions.removeLongTermWatchTarget(row.rootPath)
                        },
                        onClearHistoryGap: {
                            actions.clearHistoryGap(row.rootPath)
                        },
                        onSetGrowthAlertThreshold: {
                            actions.setGrowthAlertThreshold($0, row.rootPath)
                        },
                        onSetRecordingFilters: { patterns, minimumDelta, latency, minimumBytes, maximumBytes in
                            actions.setRecordingFilters(
                                patterns, minimumDelta, latency, minimumBytes, maximumBytes,
                                row.rootPath
                            )
                        }
                    )
                    .transition(.opacity)
                }
            }
            .padding(14)
            .animation(PathlightMotion.state, value: presentation.targetRows.count)
            .animation(PathlightMotion.state, value: showsLaunchAtLoginNudge)
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var selectedTargetDetail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
            ActivityDashboardTrendSection(buckets: presentation.trendBuckets)

            Divider()

            ActivityDashboardTopChangesSection(
                changes: presentation.topChanges,
                onLocate: { actions.revealInFinder($0) }
            )

            if !presentation.recentDeletions.isEmpty {
                Divider()

                ActivityDashboardDeletionsSection(
                    deletions: presentation.recentDeletions,
                    onReveal: { actions.revealInFinder(URL(filePath: $0)) }
                )
            }

            Divider()

            ActivityDashboardTimelineSection(
                rows: presentation.timelineRows,
                query: historyQuery,
                matchedCount: selectedHistory?.eventCount ?? presentation.timelineRows.count,
                onNarrow: { query in
                    guard let selectedRootPath else { return }
                    actions.narrowActivityHistory(query, selectedRootPath)
                },
                onLocate: { actions.revealInFinder(URL(filePath: $0)) },
                onReveal: { actions.revealInFinder(URL(filePath: $0)) }
            )
            }
        }
    }

    private func reconcileSelectedTarget() {
        let targetIDs = Set(targets.map(\.id))
        if let selectedTargetID, targetIDs.contains(selectedTargetID) {
            return
        }

        selectedTargetID = historyRootIDIfTracked ?? targets.first?.id
    }

    private var historyRootIDIfTracked: String? {
        guard let history = histories.first else { return nil }
        let historyRootID = history.rootPath.standardizedFileURL.path
        guard targets.contains(where: { $0.id == historyRootID }) else {
            return nil
        }
        return historyRootID
    }

    /// The read the timeline is showing: its totals cover every row that
    /// matched, which is what the page footer counts against.
    private var selectedHistory: ActivityHistorySnapshot? {
        guard let selectedTargetID else { return nil }
        return histories.first { $0.rootPath.standardizedFileURL.path == selectedTargetID }
    }

    private func refreshSelectedHistory() {
        guard let selectedRootPath else { return }
        actions.refreshActivityHistory(selectedRootPath)
    }

    private func refreshAllTargetHistories() {
        actions.refreshActivityDashboardHistories(targets.map(\.rootPath))
    }
}

private enum MonitoringSetup: Identifiable {
    case longTerm
    case preset(MonitoringPreset)
    case live

    private struct Presentation {
        let id: String
        let title: String
        let actionTitle: String
    }

    private var presentation: Presentation {
        switch self {
        case .longTerm:
            Presentation(
                id: "long-term",
                title: "Monitor a Folder",
                actionTitle: "Choose Folder and Start"
            )
        case let .preset(preset):
            Presentation(
                id: "preset-\(preset.id)",
                title: "Monitor \(preset.title)",
                actionTitle: "Start Monitoring"
            )
        case .live:
            Presentation(
                id: "live",
                title: "Start Live Monitor",
                actionTitle: "Choose Folder and Start"
            )
        }
    }

    var id: String { presentation.id }
    var title: String { presentation.title }
    var actionTitle: String { presentation.actionTitle }
}

private struct MonitoringStartConfigurationView: View {
    let setup: MonitoringSetup
    let onStart: (MonitoringStartConfiguration) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var minimumKilobytes: Int64 = 1
    @State private var reportIntervalSeconds = 5.0

    private var configuration: MonitoringStartConfiguration {
        MonitoringStartConfiguration(
            minimumKilobytes: minimumKilobytes,
            monitorLatency: reportIntervalSeconds
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text(setup.title)
                .font(.title2.weight(.semibold))

            Form {
                LabeledContent {
                    Stepper(
                        value: $minimumKilobytes,
                        in: 0...MonitoringStartConfiguration.maximumMinimumKilobytes
                    ) {
                        HStack(spacing: 6) {
                            TextField("Kilobytes", value: $minimumKilobytes, format: .number)
                                .labelsHidden()
                                .multilineTextAlignment(.trailing)
                                .frame(width: 90)
                            Text("KB")
                                .foregroundStyle(.secondary)
                        }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text("Smallest change")
                        HelpTip("A change that moves fewer bytes than this is not recorded. 0 records every measurable change.")
                    }
                }

                LabeledContent {
                    Stepper(
                        value: $reportIntervalSeconds,
                        in: MonitoringStartConfiguration.minimumMonitorLatency...MonitoringStartConfiguration.maximumMonitorLatency,
                        step: 0.25
                    ) {
                        HStack(spacing: 6) {
                            TextField(
                                "Seconds",
                                value: $reportIntervalSeconds,
                                format: .number.precision(.fractionLength(0...2))
                            )
                            .labelsHidden()
                            .multilineTextAlignment(.trailing)
                            .frame(width: 90)
                            Text("seconds")
                                .foregroundStyle(.secondary)
                        }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text("Report changes every")
                        HelpTip("A shorter wait shows a change sooner and wakes this Mac more often; a longer one is cheaper and groups a burst of edits into fewer rows. Nothing is lost either way — only when you hear about it. Both of these stay editable while the watch runs.")
                    }
                }

            }
            .formStyle(.grouped)

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    dismiss()
                }
                Button(setup.actionTitle) {
                    onStart(configuration)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 440)
    }
}

private struct ActivityDashboardTargetRow: View {
    let row: ActivityDashboardPresentation.TargetRow
    var baselineProgress: BaselineProgress? = nil
    let isSelected: Bool
    let onSelect: () -> Void
    let onSetEnabled: (Bool) -> Void
    let onReveal: () -> Void
    let onRemove: () -> Void
    let onClearHistoryGap: () -> Void
    let onSetGrowthAlertThreshold: (Int64?) -> Void
    let onSetRecordingFilters: ([String], Int64, TimeInterval, Int64?, Int64?) -> Void

    @State private var showsExclusionEditor = false

    private static let alertThresholdChoices: [(label: String, bytes: Int64)] = [
        ("1 GB", 1_000_000_000),
        ("5 GB", 5_000_000_000),
        ("20 GB", 20_000_000_000)
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button(action: onSelect) {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 10) {
                        Image(systemName: row.isEnabled ? "record.circle.fill" : "pause.circle")
                            .foregroundStyle(row.isEnabled ? Color.green : Color.secondary)

                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.title)
                                .font(.headline)
                                .lineLimit(1)
                                .truncationMode(.middle)

                            Text(row.subtitle)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }

                        Spacer(minLength: 8)
                    }

                    HStack(alignment: .top, spacing: 8) {
                        ActivityDashboardMetric(title: row.statusText, value: row.changeText)
                        ActivityDashboardMetric(title: row.eventText, value: row.thresholdText)
                    }
                    .fixedSize(horizontal: false, vertical: true)

                    if let baselineProgress {
                        BaselineProgressRow(progress: baselineProgress)
                    }

                    Text(row.lastActivityText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(PathlightCardButtonStyle())
            .accessibilityAddTraits(isSelected ? .isSelected : [])

            HStack(spacing: 8) {
                Button {
                    onSetEnabled(!row.isEnabled)
                } label: {
                    Label(row.isEnabled ? "Pause" : "Resume", systemImage: row.isEnabled ? "pause.fill" : "play.fill")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help(row.isEnabled ? "Pause" : "Resume")

                Button(action: onReveal) {
                    Label("Reveal", systemImage: "folder")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Reveal in Finder")

                Menu {
                    Button {
                        onSetGrowthAlertThreshold(nil)
                    } label: {
                        if row.growthAlertThresholdBytes == nil {
                            Label("Off", systemImage: "checkmark")
                        } else {
                            Text("Off")
                        }
                    }
                    ForEach(Self.alertThresholdChoices, id: \.bytes) { choice in
                        Button {
                            onSetGrowthAlertThreshold(choice.bytes)
                        } label: {
                            if row.growthAlertThresholdBytes == choice.bytes {
                                Label(choice.label, systemImage: "checkmark")
                            } else {
                                Text(choice.label)
                            }
                        }
                    }
                } label: {
                    Label(
                        "Growth Alerts",
                        systemImage: row.growthAlertThresholdBytes == nil ? "bell.slash" : "bell.fill"
                    )
                }
                .menuStyle(.button)
                .buttonStyle(.borderless)
                .menuIndicator(.hidden)
                .labelStyle(.iconOnly)
                .fixedSize()
                .help("Notify when this folder grows past a daily threshold")

                Button {
                    showsExclusionEditor = true
                } label: {
                    Label(
                        "What This Watch Records",
                        systemImage: row.exclusionPatterns.isEmpty
                            && row.minimumFileBytes == nil
                            && row.maximumFileBytes == nil
                            ? "line.3.horizontal.decrease.circle"
                            : "line.3.horizontal.decrease.circle.fill"
                    )
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Edit which files this watch records")
                .popover(isPresented: $showsExclusionEditor, arrowEdge: .bottom) {
                    ActivityRecordingFilterEditor(
                        patterns: row.exclusionPatterns,
                        minimumFileBytes: row.minimumFileBytes,
                        maximumFileBytes: row.maximumFileBytes,
                        minimumRecordedByteDelta: row.minimumRecordedByteDelta,
                        monitorLatency: row.monitorLatency,
                        onApply: onSetRecordingFilters
                    )
                }

                if let note = row.unobservableLinkNote {
                    Label("Linked Items", systemImage: "link.badge.plus")
                        .labelStyle(.iconOnly)
                        .foregroundStyle(.secondary)
                        .help(note)
                }

                if row.hasHistoryGap {
                    Button(action: onClearHistoryGap) {
                        Label("Clear History Gap", systemImage: "exclamationmark.triangle")
                    }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.yellow)
                    .help("History gap detected — click to acknowledge")
                }

                Spacer(minLength: 0)

                Button(role: .destructive, action: onRemove) {
                    Label("Remove", systemImage: "trash")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Remove")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(rowBackground, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(isSelected ? Color.accentColor.opacity(0.55) : Color.clear, lineWidth: 1)
        }
        .animation(PathlightMotion.state, value: isSelected)
    }

    private var rowBackground: Color {
        isSelected ? Color.accentColor.opacity(0.12) : Color(nsColor: .controlBackgroundColor)
    }
}

/// Recording health sits above the data it describes, so the numbers below are
/// never read as complete while they are not. Translucent and un-dismissable:
/// it is live status, not a message, and it clears itself once writes succeed.
private struct ActivityMonitoringStatusBanner: View {
    let message: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)

            Text(message)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
        .accessibilityElement(children: .combine)
    }
}

private struct ActivityLaunchAtLoginNudge: View {
    let onEnable: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Label("Monitoring stops when Pathlight quits", systemImage: "power")
                    .font(.subheadline.weight(.semibold))
                HelpTip("Start Pathlight at login so monitoring resumes automatically.")
            }

            HStack(spacing: 10) {
                Button("Start at Login", action: onEnable)
                    .controlSize(.small)

                Button("Not Now", action: onDismiss)
                    .controlSize(.small)
                    .buttonStyle(.borderless)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(Color.accentColor.opacity(0.25), lineWidth: 1)
        }
    }
}

private struct ActivityRecordingFilterEditor: View {
    let onApply: ([String], Int64, TimeInterval, Int64?, Int64?) -> Void

    @State private var text: String
    @State private var deltaText: String
    @State private var latencyText: String
    @State private var minimumText: String
    @State private var maximumText: String
    @Environment(\.dismiss) private var dismiss

    /// Bounds are typed and shown in megabytes: bytes are unreadable at the
    /// sizes anybody sets a bound at, and this is the unit the rest of the
    /// dashboard counts in.
    private static let bytesPerUnit: Double = 1_000_000

    /// The typed threshold, or nil when the box cannot be read — which keeps
    /// Apply off rather than silently recording more than the user asked for.
    /// In bytes, unlike the bounds above: this is the setting people take to
    /// zero, and "0.001 MB" is not how anybody says one kilobyte.
    private var deltaBytes: Int64? {
        let trimmed = deltaText.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return 0 }
        guard let value = Int64(trimmed), value >= 0 else { return nil }
        return value
    }

    /// The typed wait, or nil when the box is not a number inside the range the
    /// watcher accepts. Out of range is nil rather than clamped: a box that
    /// silently becomes a different number is how a person stops trusting it.
    private var latencySeconds: TimeInterval? {
        let trimmed = latencyText.trimmingCharacters(in: .whitespaces)
        guard let value = Double(trimmed),
              value >= MonitoringStartConfiguration.minimumMonitorLatency,
              value <= MonitoringStartConfiguration.maximumMonitorLatency else {
            return nil
        }
        return value
    }

    private static func latencyText(for seconds: TimeInterval) -> String {
        seconds == seconds.rounded() ? String(Int(seconds)) : String(format: "%.2f", seconds)
    }

    init(
        patterns: [String],
        minimumFileBytes: Int64?,
        maximumFileBytes: Int64?,
        minimumRecordedByteDelta: Int64,
        monitorLatency: TimeInterval,
        onApply: @escaping ([String], Int64, TimeInterval, Int64?, Int64?) -> Void
    ) {
        self.onApply = onApply
        _text = State(initialValue: patterns.joined(separator: "\n"))
        _deltaText = State(initialValue: String(max(minimumRecordedByteDelta, 0)))
        _latencyText = State(initialValue: Self.latencyText(for: monitorLatency))
        _minimumText = State(initialValue: Self.unitText(for: minimumFileBytes))
        _maximumText = State(initialValue: Self.unitText(for: maximumFileBytes))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("What This Watch Records")
                .font(.headline)


            LabeledContent {
                HStack(spacing: 6) {
                    TextField("0", text: $deltaText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                        .multilineTextAlignment(.trailing)
                    Text("bytes")
                        .foregroundStyle(.secondary)
                }
            } label: {
                HStack(spacing: 4) {
                    Text("Smallest change")
                    HelpTip("A change that moves fewer bytes than this is not recorded at all. The default of 1,024 leaves out the churn nobody asked about — lock files, editor autosaves, log lines. Type 0 to record every change there is, down to a single byte: the most detail this watch can give, and the most rows, which the storage cap then ages out sooner.")
                }
            }


            LabeledContent {
                HStack(spacing: 6) {
                    TextField("5", text: $latencyText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                        .multilineTextAlignment(.trailing)
                    Text("seconds")
                        .foregroundStyle(.secondary)
                }
            } label: {
                HStack(spacing: 4) {
                    Text("Report changes every")
                    HelpTip("How long the watcher gathers changes before reporting them. A shorter wait shows a change sooner and wakes this Mac more often; a longer one is cheaper and groups a burst of edits into fewer rows. Nothing is lost either way — only when you hear about it.")
                }
            }

            Divider()


            HStack(spacing: 4) {
                Text("File size")
                HelpTip("File size, in MB. Leave a field empty for no limit. A file whose size cannot be read — a deletion, usually — is always recorded.")
            }

            HStack(spacing: 8) {
                LabeledContent("At least") {
                    TextField("Any", text: $minimumText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 80)
                        .multilineTextAlignment(.trailing)
                }
                LabeledContent("At most") {
                    TextField("Any", text: $maximumText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 80)
                        .multilineTextAlignment(.trailing)
                }
            }
            .labeledContentStyle(.automatic)

            if let note = boundsNote {
                Label(note, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Divider()


            HStack(spacing: 4) {
                Text("Excluded patterns")
                HelpTip("One gitignore-style pattern per line. Matching files and folders are left out of monitoring. Leave empty to record everything.")
            }

            TextEditor(text: $text)
                .font(.body.monospaced())
                .frame(height: 140)
                .overlay {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(.quaternary)
                }

            HStack {
                Button("Restore Defaults") {
                    text = ActivityExclusionPatterns.defaults.joined(separator: "\n")
                    deltaText = String(LongTermWatchTargetOptions.defaultMinimumRecordedByteDelta)
                    latencyText = Self.latencyText(for: MonitoringStartConfiguration.default.monitorLatency)
                    minimumText = ""
                    maximumText = ""
                }

                Spacer()

                Button("Apply") {
                    onApply(
                        patternLines,
                        deltaBytes ?? LongTermWatchTargetOptions.defaultMinimumRecordedByteDelta,
                        latencySeconds ?? MonitoringStartConfiguration.default.monitorLatency,
                        bytes(from: minimumText),
                        bytes(from: maximumText)
                    )
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(boundsNote != nil || deltaBytes == nil || latencySeconds == nil)
            }
        }
        .padding(16)
        .frame(width: 340)
    }

    /// Why Apply is refused, or nil when the bounds are usable. A field that
    /// cannot be read must not be applied as "no limit": the user would be
    /// told the watch is bounded while it records everything.
    private var boundsNote: String? {
        if !isReadable(minimumText) || !isReadable(maximumText) {
            return "Sizes are in MB, digits only."
        }
        if let minimum = bytes(from: minimumText),
           let maximum = bytes(from: maximumText),
           minimum > maximum {
            return "The smallest size is above the largest, which records nothing."
        }
        return nil
    }

    private func isReadable(_ field: String) -> Bool {
        let trimmed = field.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty || (Double(trimmed).map { $0 >= 0 } == true)
    }

    private func bytes(from field: String) -> Int64? {
        let trimmed = field.trimmingCharacters(in: .whitespaces)
        guard let value = Double(trimmed), value > 0 else {
            return nil
        }
        return Int64(value * Self.bytesPerUnit)
    }

    private static func unitText(for bytes: Int64?) -> String {
        guard let bytes else {
            return ""
        }
        let value = Double(bytes) / bytesPerUnit
        return value == value.rounded() ? String(Int64(value)) : String(value)
    }

    private var patternLines: [String] {
        text
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }
}

private struct ActivityDashboardMetric: View {
    let title: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)

            Text(value)
                .font(.caption.monospacedDigit().weight(.medium))
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

private struct ActivityDashboardTrendSection: View {
    let buckets: [ActivityHistoryPresentation.Bucket]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 6) {
                Label("Space Trend", systemImage: "chart.xyaxis.line")
                    .font(.headline)
                HelpTip("Each bar is one period's net change; the period grows with the span, from 5 minutes to a week. Hover a bar for its bytes and event count.")
            }

            if buckets.isEmpty {
                Text("No recorded history")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 150, alignment: .center)
            } else {
                ActivityDashboardTrendChart(buckets: buckets)
                    .frame(height: 170)
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ActivityDashboardTrendChart: View {
    let buckets: [ActivityHistoryPresentation.Bucket]

    @State private var selectedDate: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Chart(buckets) { bucket in
                BarMark(
                    xStart: .value("Start", bucket.startDate),
                    xEnd: .value("End", bucket.endDate.addingTimeInterval(
                        -bucket.endDate.timeIntervalSince(bucket.startDate) * 0.15
                    )),
                    y: .value("Change", bucket.byteDelta)
                )
                .foregroundStyle(bucket.byteDelta >= 0 ? Color.green.gradient : Color.orange.gradient)
                .cornerRadius(3)

                if let selectedBucket, selectedBucket.id == bucket.id {
                    RuleMark(x: .value("Selected", selectedBucket.startDate))
                        .foregroundStyle(.secondary.opacity(0.4))
                }
            }
            .chartYAxis {
                AxisMarks(position: .trailing) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let bytes = value.as(Int64.self) {
                            Text(ActivityDashboardPresentation.signedSize(bytes))
                                .font(.caption2.monospacedDigit())
                        }
                    }
                }
            }
            .chartXSelection(value: $selectedDate)

            Text(selectionCaption)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    private var selectedBucket: ActivityHistoryPresentation.Bucket? {
        guard let selectedDate else {
            return nil
        }
        return buckets.min { lhs, rhs in
            abs(lhs.startDate.timeIntervalSince(selectedDate)) < abs(rhs.startDate.timeIntervalSince(selectedDate))
        }
    }

    private var selectionCaption: String {
        guard let bucket = selectedBucket else {
            return " "
        }
        let eventLabel = bucket.eventCount == 1 ? "event" : "events"
        return "\(bucket.label) • \(bucket.detail) • \(bucket.eventCount.formatted()) \(eventLabel)"
    }
}

private struct ActivityDashboardTopChangesSection: View {
    let changes: [ActivityDashboardPresentation.TopChange]
    let onLocate: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Top Changes", systemImage: "arrow.up.arrow.down.circle")
                .font(.headline)

            if changes.isEmpty {
                Text("No attributable changes yet")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(changes) { change in
                    ActivityDashboardTopChangeRow(change: change, onLocate: onLocate)
                }
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ActivityDashboardTopChangeRow: View {
    let change: ActivityDashboardPresentation.TopChange
    let onLocate: (URL) -> Void

    var body: some View {
        HStack(spacing: 12) {
            Text(change.title)
                .font(.subheadline.weight(.medium))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(minWidth: 120, alignment: .leading)

            GeometryReader { proxy in
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .fill(barColor.opacity(0.35))
                    .frame(width: max(4, proxy.size.width * change.magnitudeFraction))
                    .frame(maxHeight: .infinity, alignment: .center)
            }
            .frame(height: 8)

            Text(change.changeText)
                .font(.subheadline.monospacedDigit().weight(.medium))
                .foregroundStyle(barColor)
                .lineLimit(1)

            Text(change.eventText)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 76, alignment: .trailing)
        }
        .contentShape(Rectangle())
        .onTapGesture {
            onLocate(change.url)
        }
        .contextMenu {
            CopyPathButton(path: change.url.path)
        }
        .help("Click to locate \(change.title) in Pathlight")
    }

    private var barColor: Color {
        if change.byteDelta > 0 {
            return .green
        }
        if change.byteDelta < 0 {
            return .orange
        }
        return .secondary
    }
}

private struct ActivityDashboardDeletionsSection: View {
    let deletions: [ActivityDashboardPresentation.RecentDeletion]
    let onReveal: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Recently Deleted", systemImage: "trash")
                .font(.headline)

            ForEach(deletions) { deletion in
                HStack(spacing: 10) {
                    Image(systemName: deletion.isInTrash ? "trash.circle.fill" : "minus.circle.fill")
                        .foregroundStyle(deletion.isInTrash ? Color.orange : Color.secondary)
                    Text(deletion.title)
                        .font(.subheadline.weight(.medium))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(deletion.isInTrash ? "In Trash" : "Not in Trash")
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(.quaternary, in: Capsule())
                    Spacer(minLength: 8)
                    Text(deletion.sizeText)
                        .font(.subheadline.monospacedDigit())
                        .foregroundStyle(.secondary)
                    Text(deletion.timestampText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .help(deletion.path)
                .contextMenu {
                    Button("Reveal Folder in Finder") {
                        onReveal((deletion.path as NSString).deletingLastPathComponent)
                    }
                    CopyPathButton(path: deletion.path)
                }
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ActivityDashboardTimelineSection: View {
    let rows: [ActivityHistoryPresentation.Row]
    let query: ActivityHistoryQuery
    /// How many rows matched, not how many are listed.
    let matchedCount: Int
    let onNarrow: (ActivityHistoryQuery) -> Void
    let onLocate: (String) -> Void
    let onReveal: (String) -> Void

    /// What is typed but not asked for yet: a journal re-read on every
    /// keystroke would read it a dozen times for one word.
    @State private var find = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            // The controls drop under the title when the pane is narrow;
            // otherwise the whole detail column overflows and is clipped on
            // its leading edge, icons first.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) {
                    title
                    Spacer()
                    narrowingControls
                }
                VStack(alignment: .leading, spacing: 10) {
                    title
                    narrowingControls
                }
            }

            if rows.isEmpty {
                Text(query.isNarrowed ? "Nothing recorded here matches that" : "No events recorded")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 120, alignment: .center)
            } else {
                LazyVStack(spacing: 8) {
                    ForEach(rows) { row in
                        ActivityDashboardTimelineRow(
                            row: row,
                            onLocate: onLocate,
                            onReveal: onReveal
                        )
                    }
                }

                pageFooter
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
        .onChange(of: query) { _, query in
            find = query.text
        }
    }

    private var title: some View {
        Label("Timeline", systemImage: "list.bullet.rectangle")
            .font(.headline)
    }

    private var narrowingControls: some View {
        HStack(spacing: 10) {
            TextField("Find in paths", text: $find)
                .textFieldStyle(.roundedBorder)
                .frame(minWidth: 120, maxWidth: 180)
                .onSubmit { ask { $0.text = find } }

            Picker("Kind", selection: kind) {
                Text("Any change").tag(DiskActivityEventKind?.none)
                ForEach(DiskActivityEventKind.allCases, id: \.self) { kind in
                    Text(kind.title).tag(DiskActivityEventKind?.some(kind))
                }
            }
            .labelsHidden()
            .frame(width: 130)

            Toggle("Biggest first", isOn: largestFirst)
                .toggleStyle(.checkbox)

            if query != .everything {
                Button("Clear") {
                    find = ""
                    onNarrow(.everything)
                }
            }
        }
        .font(.subheadline)
    }

    private var pageFooter: some View {
        HStack(spacing: 10) {
            Text(
                "Rows \(query.skip + 1)–\(query.skip + rows.count) of \(matchedCount). "
                    + "The totals above cover them all."
            )
            .font(.caption)
            .foregroundStyle(.secondary)

            Spacer()

            Button("Previous") { page(by: -ActivityHistoryService.pageSize) }
                .disabled(query.skip == 0)
            Button("Next") { page(by: ActivityHistoryService.pageSize) }
                .disabled(query.skip + rows.count >= matchedCount)
        }
    }

    private var kind: Binding<DiskActivityEventKind?> {
        Binding(get: { query.kind }, set: { kind in ask { $0.kind = kind } })
    }

    private var largestFirst: Binding<Bool> {
        Binding(get: { query.largestFirst }, set: { isOn in ask { $0.largestFirst = isOn } })
    }

    /// A new question starts at the first page: the row somebody was on has
    /// no meaning once the rows either side of it changed.
    private func ask(_ change: (inout ActivityHistoryQuery) -> Void) {
        var next = query
        change(&next)
        next.skip = 0
        guard next != query else { return }
        onNarrow(next)
    }

    private func page(by rows: Int) {
        var next = query
        next.skip = max(next.skip + rows, 0)
        onNarrow(next)
    }
}

private struct ActivityDashboardTimelineRow: View {
    let row: ActivityHistoryPresentation.Row
    let onLocate: (String) -> Void
    let onReveal: (String) -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: iconName)
                .font(.body.weight(.semibold))
                .foregroundStyle(iconColor)
                .frame(width: 24)

            VStack(alignment: .leading, spacing: 3) {
                Text(row.title)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)

                Text(row.path)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 12)

            VStack(alignment: .trailing, spacing: 3) {
                Text(row.detail)
                    .font(.subheadline.monospacedDigit().weight(.medium))
                Text(row.timestamp, format: .dateTime.month(.abbreviated).day().hour().minute().second())
                    .font(.caption.monospacedDigit())
            }
            .foregroundStyle(.secondary)
            .lineLimit(1)
        }
        .help(row.path)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .onTapGesture {
            onLocate(row.path)
        }
        .contextMenu {
            Button("Reveal in Finder") {
                onReveal(row.path)
            }
            CopyPathButton(path: row.path)
        }
    }

    private var iconName: String {
        switch row.kind {
        case .created:
            return "plus.circle.fill"
        case .deleted:
            return "minus.circle.fill"
        case .moved:
            return "arrow.right.circle.fill"
        case .aggregate:
            return "sum"
        case .modified:
            return "pencil.circle.fill"
        }
    }

    private var iconColor: Color {
        switch row.kind {
        case .created:
            return .green
        case .deleted:
            return .orange
        case .moved:
            return .purple
        case .aggregate:
            return .secondary
        case .modified:
            return .blue
        }
    }
}

private struct ActivityDashboardEmptyState: View {
    let title: String
    let onAddFolder: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "waveform.path.ecg")
                .font(.system(size: 42, weight: .medium))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(.secondary)

            HStack(spacing: 6) {
                Text(title)
                    .font(.title3.weight(.semibold))
                HelpTip("Pathlight records what changes inside the folders you monitor.")
            }

            Button {
                onAddFolder()
            } label: {
                Label("Monitor Folder…", systemImage: "folder.badge.plus")
            }
            .controlSize(.large)
            .keyboardShortcut(.defaultAction)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
