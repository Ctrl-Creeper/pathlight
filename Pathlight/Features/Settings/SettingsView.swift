import PathlightRustCore
import SwiftUI

struct SettingsView: View {
    @AppStorage("selectedSettingsTab") private var selectedTab = SettingsTab.general.rawValue

    var body: some View {
        TabView(selection: $selectedTab) {
            GeneralSettingsPane()
                .tabItem {
                    Label("General", systemImage: "gearshape")
                }
                .tag(SettingsTab.general.rawValue)

            PrivacySettingsPane()
                .tabItem {
                    Label("Privacy", systemImage: "hand.raised")
                }
                .tag(SettingsTab.privacy.rawValue)

            ActivityStorageSettingsPane()
                .tabItem {
                    Label("Storage", systemImage: "internaldrive")
                }
                .tag(SettingsTab.storage.rawValue)

            UninstallSettingsPane()
                .tabItem {
                    Label("Uninstall", systemImage: "trash")
                }
                .tag(SettingsTab.uninstall.rawValue)
        }
        .scenePadding()
        .frame(width: 560, height: 530)
    }
}

private enum SettingsTab: String {
    case general
    case privacy
    case storage
    case uninstall
}

private struct GeneralSettingsPane: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var commandLineReport: String?
    @State private var isShowingDiary = false
    @State private var diary = ""

    var body: some View {
        Form {
            Section("Long-Term Monitoring") {
                Toggle(
                    "Launch Pathlight at Login",
                    isOn: Binding(
                        get: { appModel.launchAtLoginStatus == .enabled },
                        set: { appModel.setLaunchAtLoginEnabled($0) }
                    )
                )

                if appModel.launchAtLoginStatus == .requiresApproval {
                    Button("Review Login Items") {
                        appModel.openLoginItemsSettings()
                    }
                }

                Text(appModel.launchAtLoginStatus.monitoringSummary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Command Line") {
                Text("pathlight-monitor records a folder from a terminal and reads the same history this app does. Installing puts it in your own home directory — no administrator, nothing outside your account.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                Button("Install Command Line Tool") {
                    commandLineReport = Self.installCommandLineTool()
                }

                if let commandLineReport {
                    Text(commandLineReport)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Section("About") {
                LabeledContent("Version", value: Self.appVersion)
                LabeledContent("Monitoring core", value: "Rust \(coreVersion())")
                LabeledContent("Watcher guarantees", value: Self.watcherGuarantees)
            }

            // What the watches wrote down while nobody was looking: starts,
            // reconnects, gaps caught up, alerts and failures. The terminal
            // host prints the same file with `pathlight-monitor log`.
            Section("What the Watches Wrote") {
                DisclosureGroup(isExpanded: $isShowingDiary) {
                    Text(diary.isEmpty ? "Nothing written yet." : diary)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)

                    HStack {
                        if !diary.isEmpty {
                            CopyPathButton(path: diary, title: "Copy")
                        }
                        Button("Refresh") { diary = appModel.diaryTail() }
                        if let url = appModel.diaryFileURL {
                            Button("Show the File") { appModel.revealURLInFinder(url) }
                        }
                    }
                    .padding(.top, 4)
                } label: {
                    Text("The last \(AppModel.diaryLinesShown) lines")
                }
                .onChange(of: isShowingDiary) { _, expanded in
                    if expanded { diary = appModel.diaryTail() }
                }
            }
        }
        .formStyle(.grouped)
    }

    /// The command's own words, whether it worked or not: it knows where it
    /// went and what a shell still needs, and this pane has no better sentence
    /// than the one it prints — including the line to paste into a profile.
    private static func installCommandLineTool() -> String {
        do {
            return try CommandLineToolInstaller.live.install()
        } catch {
            return error.localizedDescription
        }
    }

    /// Which build this is. The other two hosts print the same pair — their
    /// own version and the core's — because a bug report that names only one
    /// of them cannot be reproduced.
    private static var appVersion: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "unknown"
        guard let build = info?["CFBundleVersion"] as? String, build != short else {
            return short
        }
        return "\(short) (\(build))"
    }

    /// What this platform's watcher promises. The guarantees differ per OS by
    /// design, so a report of "it missed something" is only readable if the
    /// user can see which promises were in force.
    private static var watcherGuarantees: String {
        let capabilities = watcherCapabilities()
        let granted = [
            (capabilities.resumableCursor, "resumes after relaunch"),
            (capabilities.pairsRenames, "pairs renames"),
            (capabilities.reportsProcess, "names processes"),
        ].filter(\.0).map(\.1)
        return granted.isEmpty ? "Live changes only" : granted.joined(separator: ", ")
    }
}

private extension LaunchAtLoginStatus {
    var monitoringSummary: String {
        switch self {
        case .enabled:
            return "Pathlight will start after you sign in and restore enabled long-term watches."
        case .disabled:
            return "Pathlight restores enabled long-term watches whenever the app is opened."
        case .requiresApproval:
            return "macOS needs approval before Pathlight can launch at login."
        }
    }
}

private struct PrivacySettingsPane: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        Form {
            Section("Full Disk Access") {
                Text("Pathlight can monitor ordinary folders immediately. For protected macOS locations such as Mail, Safari, Messages, and Library content, grant Full Disk Access in System Settings.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                Label(
                    appModel.fullDiskAccessStatus.fullDiskAccessSettingsSummary,
                    systemImage: appModel.fullDiskAccessStatus.fullDiskAccessSystemImage
                )
                    .foregroundStyle(appModel.fullDiskAccessStatus.fullDiskAccessColor)
                    .font(.callout)

                HStack {
                    Button("Open Full Disk Access Settings") {
                        appModel.prepareAndOpenFullDiskAccessSettings()
                    }

                    Button("Recheck") {
                        appModel.refreshFullDiskAccessStatus()
                    }
                }
            }

            Section("Activity Data") {
                Text("Activity history and the size attribution index stay on this Mac. Manage retention and encryption in the Storage tab.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
    }
}

private struct ActivityStorageSettingsPane: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var isConfirmingReset = false

    private let retentionOptions = [30, 90, 180, 365, 730, 1_825, 36_500]
    private let storageLimitOptions: [Int64] = [
        500 * 1_024 * 1_024,
        1_024 * 1_024 * 1_024,
        5 * 1_024 * 1_024 * 1_024,
        10 * 1_024 * 1_024 * 1_024
    ]

    var body: some View {
        Form {
            Section("Current Storage") {
                StatValueRow("Total activity data", value: sizeText(appModel.activityStorageUsage.totalBytes))
                StatValueRow("Activity events", value: sizeText(appModel.activityStorageUsage.eventJournalBytes))
                StatValueRow("Size attribution index", value: sizeText(appModel.activityStorageUsage.sizeIndexJournalBytes))
                StatValueRow("Activity event rows", value: countText(appModel.activityStorageUsage.eventEntryCount))
                StatValueRow("Size index rows", value: countText(appModel.activityStorageUsage.sizeIndexEntryCount))

                HStack {
                    Button("Refresh") {
                        appModel.refreshActivityStorageUsage()
                    }

                    Button("Compact Now") {
                        appModel.compactActivityStorage()
                    }

                    Button("Reset Activity Storage", role: .destructive) {
                        isConfirmingReset = true
                    }
                    .disabled(appModel.activityStorageUsage.totalBytes == 0)
                }
            }

            Section("Retention") {
                Picker("Detailed file-level logs", selection: $appModel.activityDetailedRetentionDays) {
                    ForEach(retentionOptions, id: \.self) { days in
                        Text(retentionLabel(days)).tag(days)
                    }
                }
                .pickerStyle(.menu)

                Picker("Aggregate history", selection: $appModel.activityAggregateRetentionDays) {
                    ForEach(retentionOptions, id: \.self) { days in
                        Text(retentionLabel(days)).tag(days)
                    }
                }
                .pickerStyle(.menu)

                Picker("Storage limit", selection: $appModel.activityStorageLimitBytes) {
                    ForEach(storageLimitOptions, id: \.self) { bytes in
                        Text(PathlightFormatters.size(bytes)).tag(bytes)
                    }
                }
                .pickerStyle(.menu)
            }

            Section("Privacy") {
                Toggle("Encrypt new activity data", isOn: $appModel.activityEncryptNewData)
                Text("New activity journals are sealed with AES-GCM before they are written. The key is stored in macOS Keychain.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Section {
                Button("Restore Activity Storage Defaults") {
                    appModel.restoreDefaultActivityStoragePreferences()
                }
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(
            "Reset Activity Storage?",
            isPresented: $isConfirmingReset,
            titleVisibility: .visible
        ) {
            Button("Reset Activity Storage", role: .destructive) {
                appModel.resetActivityStorage()
            }

            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This removes locally stored activity history and the size attribution index on this Mac.")
        }
    }

    private func retentionLabel(_ days: Int) -> String {
        switch days {
        case 36_500:
            return "Forever"
        case 365:
            return "1 year"
        case 730:
            return "2 years"
        case 1_825:
            return "5 years"
        default:
            return "\(days) days"
        }
    }

    private func sizeText(_ bytes: Int64) -> String {
        guard bytes > 0 else { return "0 bytes" }
        return PathlightFormatters.size(bytes)
    }

    private func countText(_ value: Int) -> String {
        value.formatted()
    }
}

private struct UninstallSettingsPane: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var pendingScope: UninstallScope?

    var body: some View {
        Form {
            Section("Uninstall Pathlight") {
                Text("Both options stop monitoring, remove the login item, and move Pathlight to the Trash.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                Button("Move Pathlight to the Trash…") {
                    pendingScope = .appOnly
                }

                Text("Keeps your activity history, settings, and encryption key, so reinstalling picks up where you left off.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Section {
                Button("Remove Pathlight and All Its Data…", role: .destructive) {
                    pendingScope = .everything
                }

                Text(Self.everythingDetail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Section("One Thing Pathlight Cannot Remove") {
                Text("macOS keeps its own record of the Full Disk Access you granted, and no app is allowed to delete that entry. Remove Pathlight from System Settings › Privacy & Security › Full Disk Access yourself.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(
            pendingScope == .everything ? "Remove Pathlight and All Its Data?" : "Move Pathlight to the Trash?",
            isPresented: Binding(get: { pendingScope != nil }, set: { if !$0 { pendingScope = nil } }),
            titleVisibility: .visible
        ) {
            Button(
                pendingScope == .everything ? "Remove Everything" : "Move to Trash",
                role: .destructive
            ) {
                if let scope = pendingScope {
                    Task { await appModel.uninstall(scope: scope) }
                }
                pendingScope = nil
            }

            Button("Cancel", role: .cancel) {
                pendingScope = nil
            }
        } message: {
            Text(
                pendingScope == .everything
                    ? "Pathlight quits, and nothing it recorded stays on this Mac. This cannot be undone. The folders you monitored are never touched."
                    : "Pathlight quits and moves to the Trash. Your activity history and settings stay on this Mac."
            )
        }
    }

    /// Named out loud, because "all its data" is the one claim the user cannot
    /// verify before agreeing to it.
    private static let everythingDetail = """
        Deletes the activity history journal, the size attribution index, all \
        settings, the Keychain encryption key, and cached files. The folders \
        you monitored are never touched.
        """
}

private struct StatValueRow: View {
    private let title: String
    private let value: String

    init(_ title: String, value: String) {
        self.title = title
        self.value = value
    }

    var body: some View {
        LabeledContent(title) {
            Text(value)
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
    }
}
