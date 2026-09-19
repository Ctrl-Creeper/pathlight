import SwiftUI

extension FullDiskAccessStatus {
    var fullDiskAccessBadgeTitle: String {
        switch self {
        case .granted:
            return "Enabled"
        case .notGranted:
            return "Not Enabled"
        case .unknown:
            return "Unknown"
        }
    }

    var fullDiskAccessSettingsSummary: String {
        switch self {
        case .granted:
            return "Full Disk Access is enabled."
        case .notGranted:
            return "Full Disk Access is not enabled."
        case .unknown:
            return "Full Disk Access could not be verified."
        }
    }

    var fullDiskAccessSystemImage: String {
        switch self {
        case .granted:
            return "checkmark.circle.fill"
        case .notGranted:
            return "xmark.circle.fill"
        case .unknown:
            return "questionmark.circle.fill"
        }
    }

    var fullDiskAccessColor: Color {
        switch self {
        case .granted:
            return .green
        case .notGranted:
            return .orange
        case .unknown:
            return .secondary
        }
    }
}

/// The "?" beside a control. Explanatory prose lives here, not on the page:
/// hover reads it as a tooltip, a click keeps it open.
struct HelpTip: View {
    let text: String
    @State private var isShown = false

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Button {
            isShown.toggle()
        } label: {
            Image(systemName: "questionmark.circle")
                .foregroundStyle(.secondary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Help")
        .help(text)
        .popover(isPresented: $isShown, arrowEdge: .bottom) {
            Text(text)
                .font(.callout)
                .frame(width: 280, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(12)
        }
    }
}

/// One line under a watch while it learns its files' sizes.
struct BaselineProgressRow: View {
    let progress: BaselineProgress

    var body: some View {
        HStack(spacing: 8) {
            if let fraction = progress.fraction {
                ProgressView(value: fraction)
                    .frame(maxWidth: 120)
            } else {
                ProgressView()
                    .controlSize(.small)
            }
            Text(progress.text)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
            HelpTip("Deleted files have no size left to read, so Pathlight learns every file's size up front and keeps the table for next launch. Until the walk reaches a file, its deletion shows an unknown size.")
        }
        .accessibilityElement(children: .combine)
    }
}
