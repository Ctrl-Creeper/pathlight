import SwiftUI

struct ActivityTimelinePanel: View {
    let session: WatchSessionModel
    let onStop: () -> Void

    private var presentation: ActivityTimelinePresentation {
        ActivityTimelinePresentation(session: session, eventLimit: 6)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                Label(presentation.title, systemImage: "waveform.path.ecg")
                    .font(.headline)

                Text(presentation.summaryText)
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(.secondary)

                Spacer(minLength: 12)

                Button {
                    onStop()
                } label: {
                    Label("Stop Watching", systemImage: "stop.circle")
                }
                .labelStyle(.iconOnly)
                .help("Stop Watching")
            }

            if presentation.rows.isEmpty {
                Text("No recorded changes yet")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .frame(height: 44, alignment: .center)
            } else {
                ScrollView(.horizontal) {
                    HStack(spacing: 8) {
                        ForEach(presentation.rows) { row in
                            ActivityTimelineRow(row: row)
                        }
                    }
                    .padding(.bottom, 1)
                }
                .scrollIndicators(.never)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.regularMaterial)
    }
}

private struct ActivityTimelineRow: View {
    let row: ActivityTimelinePresentation.Row

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
