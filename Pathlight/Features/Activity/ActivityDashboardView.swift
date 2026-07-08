import SwiftUI

struct ActivityDashboardActions {
    let refreshActivityHistory: (URL) -> Void
    let refreshActivityDashboardHistories: ([URL]) -> Void
    let setLongTermWatchEnabled: (Bool, URL) -> Void
    let removeLongTermWatchTarget: (URL) -> Void
    let revealInFinder: (URL) -> Void
}

struct ActivityDashboardView: View {
    let targets: [LongTermWatchTarget]
    let histories: [ActivityHistorySnapshot]
    let actions: ActivityDashboardActions

    @State private var selectedTargetID: String?

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
            histories: histories
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            dashboardContent
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .toolbar {
            ToolbarItemGroup(placement: .automatic) {
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
            ActivityDashboardEmptyState(title: presentation.emptyStateTitle ?? "No Activity")
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
                        }
                    )
                }
            }
            .padding(14)
        }
        .background(Color(nsColor: .underPageBackgroundColor))
    }

    private var selectedTargetDetail: some View {
        VStack(alignment: .leading, spacing: 0) {
            ActivityDashboardTrendSection(buckets: presentation.trendBuckets)

            Divider()

            ActivityDashboardTimelineSection(rows: presentation.timelineRows)
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

    var body: some View {
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
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .onTapGesture(perform: onSelect)
    }

    private var rowBackground: Color {
        isSelected ? Color.accentColor.opacity(0.12) : Color(nsColor: .controlBackgroundColor)
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

    var body: some View {
        HStack(alignment: .bottom, spacing: 6) {
            ForEach(buckets) { bucket in
                Capsule(style: .continuous)
                    .fill(color(for: bucket))
                    .frame(maxWidth: .infinity)
                    .frame(height: max(10, 150 * bucket.magnitudeFraction))
                    .help("\(bucket.label): \(bucket.detail), \(bucket.eventCount.formatted()) events")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        .padding(.top, 12)
    }

    private func color(for bucket: ActivityHistoryPresentation.Bucket) -> Color {
        if bucket.byteDelta > 0 {
            return .green
        }
        if bucket.byteDelta < 0 {
            return .orange
        }
        return .secondary
    }
}

private struct ActivityDashboardTimelineSection: View {
    let rows: [ActivityHistoryPresentation.Row]

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
                            ActivityDashboardTimelineRow(row: row)
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
    }

    private var iconName: String {
        if row.title.hasPrefix("Created") {
            return "plus.circle.fill"
        }
        if row.title.hasPrefix("Deleted") {
            return "minus.circle.fill"
        }
        return "pencil.circle.fill"
    }

    private var iconColor: Color {
        if row.title.hasPrefix("Created") {
            return .green
        }
        if row.title.hasPrefix("Deleted") {
            return .orange
        }
        return .blue
    }
}

private struct ActivityDashboardEmptyState: View {
    let title: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "waveform.path.ecg")
                .font(.system(size: 42, weight: .medium))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(.secondary)

            Text(title)
                .font(.title3.weight(.semibold))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
