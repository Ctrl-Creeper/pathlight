import SwiftUI

struct ActivityTimelinePanel: View {
    let session: WatchSessionModel
    let onStop: () -> Void
    var noiseSuggestion: ActivityNoiseSuggestion? = nil
    var onExcludeNoise: (() -> Void)? = nil

    private var presentation: ActivityTimelinePresentation {
        // ponytail: 200 rows is plenty of scrollback; raise if sessions outgrow it.
        ActivityTimelinePresentation(session: session, eventLimit: 200)
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

            if let noiseSuggestion {
                HStack(spacing: 8) {
                    Image(systemName: "speaker.slash")
                        .foregroundStyle(.secondary)
                    Text("\(noiseSuggestion.eventCount.formatted()) of these events are under \(noiseSuggestion.pattern)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    if let onExcludeNoise {
                        Button("Exclude") {
                            onExcludeNoise()
                        }
                        .controlSize(.small)
                        .help("Add this folder to the long-term watch's exclusion patterns")
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
            }

            if session.droppedEventCount > 0 {
                Text("Showing the latest \(WatchSessionModel.maxRetainedEvents.formatted()) events; \(session.droppedEventCount.formatted()) older events were dropped.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            if presentation.rows.isEmpty {
                Text("No recorded changes yet")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .frame(height: 44, alignment: .center)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(presentation.rows) { row in
                            ActivityTimelineRow(row: row)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
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

            HStack(spacing: 6) {
                Text(row.detail)
                    .font(.caption.monospacedDigit().weight(.medium))

                Text(row.timestampText)
                    .font(.caption)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .foregroundStyle(.secondary)
        }
        .help(row.path)
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
    }
}
