import SwiftUI

struct ActivityHistoryPanel: View {
    let snapshot: ActivityHistorySnapshot

    private var presentation: ActivityHistoryPresentation {
        ActivityHistoryPresentation(snapshot: snapshot, eventLimit: 4, bucketLimit: 18)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                Label(presentation.title, systemImage: "clock.arrow.circlepath")
                    .font(.headline)

                Text(presentation.summaryText)
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(.secondary)

                Spacer(minLength: 12)
            }

            HStack(alignment: .bottom, spacing: 14) {
                if presentation.buckets.isEmpty {
                    Text("No recorded history yet")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .frame(height: 46, alignment: .center)
                } else {
                    ActivityHistoryTrend(buckets: presentation.buckets)

                    if presentation.rows.isEmpty {
                        Spacer(minLength: 0)
                    } else {
                        ScrollView(.horizontal) {
                            HStack(spacing: 8) {
                                ForEach(presentation.rows) { row in
                                    ActivityHistoryRow(row: row)
                                }
                            }
                            .padding(.bottom, 1)
                        }
                        .scrollIndicators(.never)
                    }
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.regularMaterial)
    }
}

private struct ActivityHistoryTrend: View {
    let buckets: [ActivityHistoryPresentation.Bucket]

    var body: some View {
        HStack(alignment: .bottom, spacing: 4) {
            ForEach(buckets) { bucket in
                Capsule(style: .continuous)
                    .fill(color(for: bucket))
                    .frame(width: 9, height: max(8, 44 * bucket.magnitudeFraction))
                    .help("\(bucket.label): \(bucket.detail), \(bucket.eventCount.formatted()) events")
            }
        }
        .frame(width: 230, height: 48, alignment: .bottomLeading)
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

private struct ActivityHistoryRow: View {
    let row: ActivityHistoryPresentation.Row

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(row.title)
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)

            Text(row.detail)
                .font(.caption.monospacedDigit().weight(.medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .help(row.path)
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(width: 180, alignment: .leading)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
    }
}
