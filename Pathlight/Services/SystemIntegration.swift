//
//  SystemIntegration.swift
//  Pathlight
//
//  Created by Codex on 4/2/26.
//

import AppKit
import Darwin
import Foundation

enum FullDiskAccessStatus: Equatable, Sendable {
    case granted
    case notGranted
    case unknown
}

protocol SystemWorkspace {
    func activateFileViewerSelecting(_ fileURLs: [URL])
    func open(_ url: URL) -> Bool
}

extension NSWorkspace: SystemWorkspace {}


enum SystemIntegration {
    typealias FullDiskAccessProbe = () throws -> Void
    private nonisolated static let requiredReadableDataVaultProbeCount = 2
    private nonisolated static let requiredReadableMacOS27SentinelCount = 2
    private nonisolated static let macOS27MajorVersion = 27
    private static var isRunningInsideXcodePreview: Bool {
        ProcessInfo.processInfo.environment["XCODE_RUNNING_FOR_PREVIEWS"] == "1"
    }

    @MainActor
    static func presentFolderPanel(prompt: String, message: String) -> URL? {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = false
        panel.prompt = prompt
        panel.message = message

        guard panel.runModal() == .OK, let url = panel.url else {
            return nil
        }

        return url.standardizedFileURL
    }

    static func reveal(_ url: URL, workspace: SystemWorkspace = NSWorkspace.shared) {
        reveal([url], workspace: workspace)
    }

    static func reveal(_ urls: [URL], workspace: SystemWorkspace = NSWorkspace.shared) {
        workspace.activateFileViewerSelecting(urls)
    }

    @discardableResult
    static func openFullDiskAccessSettings() -> Bool {
        guard !isRunningInsideXcodePreview else {
            return false
        }

        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") else {
            return false
        }

        return NSWorkspace.shared.open(url)
    }

    @discardableResult
    static func prepareAndOpenFullDiskAccessSettings() -> Bool {
        guard !isRunningInsideXcodePreview else {
            return false
        }

        primeFullDiskAccessListEntry()
        return openFullDiskAccessSettings()
    }

    static func primeFullDiskAccessListEntry() {
        _ = probeFullDiskAccess()
    }

    nonisolated static func fullDiskAccessStatus() -> FullDiskAccessStatus {
        probeFullDiskAccess()
    }

    private nonisolated static func probeFullDiskAccess() -> FullDiskAccessStatus {
        let fileManager = FileManager.default
        let homeDirectory = fileManager.homeDirectoryForCurrentUser
        let macOSMajorVersion = ProcessInfo.processInfo.operatingSystemVersion.majorVersion

        guard macOSMajorVersion < macOS27MajorVersion else {
            let timeMachinePreferencesProbe = makeFullDiskAccessProbe(
                for: ProtectedPathProbe(
                    url: URL(filePath: "/Library/Preferences/com.apple.TimeMachine.plist"),
                    kind: .file
                ),
                using: fileManager
            )
            let stocksContainerProbe = makeFullDiskAccessProbe(
                for: ProtectedPathProbe(
                    url: homeDirectory.appending(path: "Library/Containers/com.apple.stocks", directoryHint: .isDirectory),
                    kind: .directory
                ),
                using: fileManager
            )
            let systemTCCDatabaseProbe = makeFullDiskAccessProbe(
                for: ProtectedPathProbe(
                    url: URL(filePath: "/Library/Application Support/com.apple.TCC/TCC.db"),
                    kind: .file
                ),
                using: fileManager
            )

            return fullDiskAccessStatus(
                macOSMajorVersion: macOSMajorVersion,
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [],
                timeMachinePreferencesProbe: timeMachinePreferencesProbe,
                stocksContainerProbe: stocksContainerProbe,
                systemTCCDatabaseProbe: systemTCCDatabaseProbe
            )
        }

        let protectedDataVaultProbes = [
            ProtectedPathProbe(url: homeDirectory.appending(path: "Library/Mail", directoryHint: .isDirectory), kind: .directory),
            ProtectedPathProbe(url: homeDirectory.appending(path: "Library/Messages", directoryHint: .isDirectory), kind: .directory),
            ProtectedPathProbe(url: homeDirectory.appending(path: "Library/Safari", directoryHint: .isDirectory), kind: .directory),
            ProtectedPathProbe(url: homeDirectory.appending(path: "Library/HomeKit", directoryHint: .isDirectory), kind: .directory)
        ]
            .compactMap { candidate in
                makeFullDiskAccessProbe(for: candidate, using: fileManager)
            }

        let userTCCDatabaseProbe = makeFullDiskAccessProbe(
            for: ProtectedPathProbe(
                url: homeDirectory.appending(path: "Library/Application Support/com.apple.TCC/TCC.db"),
                kind: .file
            ),
            using: fileManager
        )

        return fullDiskAccessStatus(
            macOSMajorVersion: macOSMajorVersion,
            userTCCDatabaseProbe: userTCCDatabaseProbe,
            protectedDataVaultProbes: protectedDataVaultProbes,
            timeMachinePreferencesProbe: nil,
            stocksContainerProbe: nil,
            systemTCCDatabaseProbe: nil
        )
    }

    nonisolated static func fullDiskAccessStatus(
        macOSMajorVersion: Int,
        userTCCDatabaseProbe: FullDiskAccessProbe?,
        protectedDataVaultProbes: [FullDiskAccessProbe],
        timeMachinePreferencesProbe: FullDiskAccessProbe?,
        stocksContainerProbe: FullDiskAccessProbe?,
        systemTCCDatabaseProbe: FullDiskAccessProbe?
    ) -> FullDiskAccessStatus {
        guard macOSMajorVersion < macOS27MajorVersion else {
            return macOS27FullDiskAccessStatus(
                timeMachinePreferencesProbe: timeMachinePreferencesProbe,
                stocksContainerProbe: stocksContainerProbe,
                systemTCCDatabaseProbe: systemTCCDatabaseProbe
            )
        }

        return legacyFullDiskAccessStatus(
            userTCCDatabaseProbe: userTCCDatabaseProbe,
            protectedDataVaultProbes: protectedDataVaultProbes
        )
    }

    nonisolated static func fullDiskAccessStatus(
        userTCCDatabaseProbe: FullDiskAccessProbe?,
        protectedDataVaultProbes: [FullDiskAccessProbe]
    ) -> FullDiskAccessStatus {
        legacyFullDiskAccessStatus(
            userTCCDatabaseProbe: userTCCDatabaseProbe,
            protectedDataVaultProbes: protectedDataVaultProbes
        )
    }

    private nonisolated static func legacyFullDiskAccessStatus(
        userTCCDatabaseProbe: FullDiskAccessProbe?,
        protectedDataVaultProbes: [FullDiskAccessProbe]
    ) -> FullDiskAccessStatus {
        let foundProtectedCandidate = userTCCDatabaseProbe != nil || !protectedDataVaultProbes.isEmpty
        guard foundProtectedCandidate else { return .unknown }
        guard let userTCCDatabaseProbe,
              canReadFullDiskAccessProbe(userTCCDatabaseProbe) else {
            return .notGranted
        }

        let readableDataVaultProbeCount = protectedDataVaultProbes.reduce(into: 0) { count, probe in
            if canReadFullDiskAccessProbe(probe) {
                count += 1
            }
        }

        return readableDataVaultProbeCount >= requiredReadableDataVaultProbeCount ? .granted : .notGranted
    }

    private nonisolated static func macOS27FullDiskAccessStatus(
        timeMachinePreferencesProbe: FullDiskAccessProbe?,
        stocksContainerProbe: FullDiskAccessProbe?,
        systemTCCDatabaseProbe: FullDiskAccessProbe?
    ) -> FullDiskAccessStatus {
        let primaryProbes = [timeMachinePreferencesProbe, stocksContainerProbe]

        if primaryProbes.allSatisfy({ $0 != nil }) {
            return primaryProbes.allSatisfy { probe in
                guard let probe else { return false }
                return canReadFullDiskAccessProbe(probe)
            } ? .granted : .notGranted
        }

        let fallbackReadableCount = [timeMachinePreferencesProbe, stocksContainerProbe, systemTCCDatabaseProbe]
            .reduce(into: 0) { count, probe in
                if let probe, canReadFullDiskAccessProbe(probe) {
                    count += 1
                }
            }

        return fallbackReadableCount >= requiredReadableMacOS27SentinelCount ? .granted : .unknown
    }

    private nonisolated static func makeFullDiskAccessProbe(
        for candidate: ProtectedPathProbe,
        using fileManager: FileManager
    ) -> FullDiskAccessProbe? {
        guard fileManager.fileExists(atPath: candidate.url.path) else { return nil }
        return {
            try candidate.probe(using: fileManager)
        }
    }

    private nonisolated static func canReadFullDiskAccessProbe(_ probe: FullDiskAccessProbe) -> Bool {
        do {
            try probe()
            return true
        } catch {
            return false
        }
    }
}

private struct ProtectedPathProbe {
    enum Kind {
        case directory
        case file
    }

    var url: URL
    var kind: Kind

    nonisolated func probe(using fileManager: FileManager) throws {
        switch kind {
        case .directory:
            _ = try fileManager.contentsOfDirectory(
                at: url,
                includingPropertiesForKeys: nil
            )
        case .file:
            let handle = try FileHandle(forReadingFrom: url)
            try? handle.close()
        }
    }
}
