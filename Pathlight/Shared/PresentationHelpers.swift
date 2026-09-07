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
