import Charts
import SwiftUI

struct ActivityDashboardActions {
    let refreshActivityHistory: (URL) -> Void
    let refreshActivityDashboardHistories: ([URL]) -> Void
    let setLongTermWatchEnabled: (Bool, URL) -> Void
    let removeLongTermWatchTarget: (URL) -> Void
    let revealInFinder: (URL) -> Void
    let clearHistoryGap: (URL) -> Void
    let setGrowthAlertThreshold: (Int64?, URL) -> Void
    let setExclusionPatterns: ([String], URL) -> Void
    let enableLaunchAtLogin: () -> Void
    let dismissLaunchAtLoginNudge: () -> Void
    let addFolder: () -> Void
    let addPreset: (MonitoringPreset) -> Void
    let exportHistory: (URL) -> Void
    let startLiveMonitor: (DiskActivityAggregationOptions) -> Void
    let stopLiveMonitor: () -> Void
}

struct ActivityDashboardView: View {
    let targets: [LongTermWatchTarget]
    let histories: [ActivityHistorySnapshot]
    let runtimeStatuses: [LongTermWatchTarget.ID: LongTermWatchRuntimeStatus]
    let showsLaunchAtLoginNudge: Bool
    var monitoringStatusMessage: String? = nil
    let isLiveMonitorActive: Bool
    let actions: ActivityDashboardActions

    @State private var selectedTargetID: String?
    @State private var presets = MonitoringPreset.available()

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
                        actions.addFolder()
                    }
                    if !untrackedPresets.isEmpty {
                        Divider()
                        ForEach(untrackedPresets) { preset in
                            Button(preset.title) {
                                actions.addPreset(preset)
                            }
                        }
                    }
                } label: {
                    Label("Monitor Folder", systemImage: "folder.badge.plus")
                }
                .help("Monitor a Folder Long-Term")

                if isLiveMonitorActive {
                    Button {
                        actions.stopLiveMonitor()
                    } label: {
                        Label("Stop Live Monitor", systemImage: "stop.circle")
                    }
                    .help("Stop Live Monitor")
                } else {
                    Menu {
                        Button("Every file change") {
                            actions.startLiveMonitor(.shortTermDefault)
                        }
                        Button("Changes 1 KB and larger") {
                            actions.startLiveMonitor(.shortTerm(minimumRecordedByteDelta: 1_024))
                        }
                        Button("Changes 1 MB and larger") {
                            actions.startLiveMonitor(.shortTerm(minimumRecordedByteDelta: 1_024 * 1_024))
                        }
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
                onAddFolder: actions.addFolder
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
                        onSetExclusionPatterns: {
                            actions.setExclusionPatterns($0, row.rootPath)
                        }
                    )
                    .transition(.opacity)
                }
            }
            .padding(14)
            .animation(PathlightMotion.state, value: presentation.targetRows.count)
            .animation(PathlightMotion.state, value: showsLaunchAtLoginNudge)
        }
        .background(Color(nsColor: .underPageBackgroundColor))
    }

    private var selectedTargetDetail: some View {
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
                onLocate: { actions.revealInFinder(URL(filePath: $0)) },
                onReveal: { actions.revealInFinder(URL(filePath: $0)) }
            )
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

    private func refreshSelectedHistory() {
        guard let selectedRootPath else { return }
        actions.refreshActivityHistory(selectedRootPath)
    }

    private func refreshAllTargetHistories() {
        actions.refreshActivityDashboardHistories(targets.map(\.rootPath))
    }
}

private struct ActivityDashboardTargetRow: View {
    let row: ActivityDashboardPresentation.TargetRow
    let isSelected: Bool
    let onSelect: () -> Void
    let onSetEnabled: (Bool) -> Void
    let onReveal: () -> Void
    let onRemove: () -> Void
    let onClearHistoryGap: () -> Void
    let onSetGrowthAlertThreshold: (Int64?) -> Void
    let onSetExclusionPatterns: ([String]) -> Void

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

                    HStack(spacing: 8) {
                        ActivityDashboardMetric(title: row.statusText, value: row.changeText)
                        ActivityDashboardMetric(title: row.eventText, value: row.thresholdText)
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
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .labelStyle(.iconOnly)
                .fixedSize()
                .help("Notify when this folder grows past a daily threshold")

                Button {
                    showsExclusionEditor = true
                } label: {
                    Label(
                        "Ignored Patterns",
                        systemImage: row.exclusionPatterns.isEmpty
                            ? "line.3.horizontal.decrease.circle"
                            : "line.3.horizontal.decrease.circle.fill"
                    )
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Edit ignored patterns")
                .popover(isPresented: $showsExclusionEditor, arrowEdge: .bottom) {
                    ActivityExclusionPatternEditor(
                        patterns: row.exclusionPatterns,
                        onApply: onSetExclusionPatterns
                    )
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
            Label("Monitoring stops when Pathlight quits", systemImage: "power")
                .font(.subheadline.weight(.semibold))

            Text("Start Pathlight at login so monitoring resumes automatically.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

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

private struct ActivityExclusionPatternEditor: View {
    let onApply: ([String]) -> Void

    @State private var text: String
    @Environment(\.dismiss) private var dismiss

    init(patterns: [String], onApply: @escaping ([String]) -> Void) {
        self.onApply = onApply
        _text = State(initialValue: patterns.joined(separator: "\n"))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Ignored Patterns")
                .font(.headline)

            Text("One gitignore-style pattern per line. Matching files and folders are left out of monitoring. Leave empty to record everything.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            TextEditor(text: $text)
                .font(.body.monospaced())
                .frame(height: 180)
                .overlay {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(.quaternary)
                }

            HStack {
                Button("Restore Defaults") {
                    text = ActivityExclusionFilter.defaultPatterns.joined(separator: "\n")
                }

                Spacer()

                Button("Apply") {
                    onApply(patternLines)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(16)
        .frame(width: 340)
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
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

private struct ActivityDashboardTrendSection: View {
    let buckets: [ActivityHistoryPresentation.Bucket]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Space Trend", systemImage: "chart.xyaxis.line")
                .font(.headline)

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
                    x: .value("Time", bucket.startDate),
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
            return "Hover for details"
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
                }
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ActivityDashboardTimelineSection: View {
    let rows: [ActivityHistoryPresentation.Row]
    let onLocate: (String) -> Void
    let onReveal: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Timeline", systemImage: "list.bullet.rectangle")
                .font(.headline)

            if rows.isEmpty {
                Text("No events recorded")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
            } else {
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(rows) { row in
                            ActivityDashboardTimelineRow(
                                row: row,
                                onLocate: onLocate,
                                onReveal: onReveal
                            )
                        }
                    }
                }
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
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

            Text(row.detail)
                .font(.subheadline.monospacedDigit().weight(.medium))
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

            Text(title)
                .font(.title3.weight(.semibold))

            Text("Pathlight records what changes inside the folders you monitor.")
                .font(.subheadline)
                .foregroundStyle(.secondary)

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
