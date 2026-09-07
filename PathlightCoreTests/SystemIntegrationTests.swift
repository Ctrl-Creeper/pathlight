import AppKit
import XCTest
@testable import PathlightCore

final class SystemIntegrationTests: XCTestCase {
    func testRevealSelectsRequestedURL() {
        let url = URL(filePath: "/tmp/example.txt")
        let workspace = WorkspaceSpy(openResult: true)

        SystemIntegration.reveal(url, workspace: workspace)

        XCTAssertEqual(workspace.revealedSelections, [[url]])
    }

    func testRevealSelectsRequestedURLs() {
        let urls = [
            URL(filePath: "/tmp/first.txt"),
            URL(filePath: "/tmp/second.txt")
        ]
        let workspace = WorkspaceSpy(openResult: true)

        SystemIntegration.reveal(urls, workspace: workspace)

        XCTAssertEqual(workspace.revealedSelections, [urls])
    }

    func testFullDiskAccessStatusUsesInjectedProbes() {
        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: []
            ),
            .unknown
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [successfulProbe, successfulProbe]
            ),
            .notGranted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                userTCCDatabaseProbe: failedProbe,
                protectedDataVaultProbes: [successfulProbe, successfulProbe]
            ),
            .notGranted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                userTCCDatabaseProbe: successfulProbe,
                protectedDataVaultProbes: [successfulProbe, failedProbe]
            ),
            .notGranted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                userTCCDatabaseProbe: successfulProbe,
                protectedDataVaultProbes: [successfulProbe, successfulProbe]
            ),
            .granted
        )
    }

    func testFullDiskAccessStatusKeepsLegacyLogicBeforeMacOS27() {
        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 26,
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [successfulProbe, successfulProbe],
                timeMachinePreferencesProbe: successfulProbe,
                stocksContainerProbe: successfulProbe,
                systemTCCDatabaseProbe: successfulProbe
            ),
            .notGranted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 26,
                userTCCDatabaseProbe: successfulProbe,
                protectedDataVaultProbes: [successfulProbe, successfulProbe],
                timeMachinePreferencesProbe: failedProbe,
                stocksContainerProbe: failedProbe,
                systemTCCDatabaseProbe: failedProbe
            ),
            .granted
        )
    }

    func testFullDiskAccessStatusUsesMacOS27PrimarySentinels() {
        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 27,
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [],
                timeMachinePreferencesProbe: successfulProbe,
                stocksContainerProbe: successfulProbe,
                systemTCCDatabaseProbe: nil
            ),
            .granted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 27,
                userTCCDatabaseProbe: successfulProbe,
                protectedDataVaultProbes: [successfulProbe, successfulProbe],
                timeMachinePreferencesProbe: successfulProbe,
                stocksContainerProbe: failedProbe,
                systemTCCDatabaseProbe: successfulProbe
            ),
            .notGranted
        )
    }

    func testFullDiskAccessStatusUsesMacOS27SystemTCCOnlyAsFallbackEvidence() {
        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 27,
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [],
                timeMachinePreferencesProbe: successfulProbe,
                stocksContainerProbe: nil,
                systemTCCDatabaseProbe: successfulProbe
            ),
            .granted
        )

        XCTAssertEqual(
            SystemIntegration.fullDiskAccessStatus(
                macOSMajorVersion: 27,
                userTCCDatabaseProbe: nil,
                protectedDataVaultProbes: [],
                timeMachinePreferencesProbe: successfulProbe,
                stocksContainerProbe: nil,
                systemTCCDatabaseProbe: failedProbe
            ),
            .unknown
        )
    }

    private var successfulProbe: SystemIntegration.FullDiskAccessProbe {
        {}
    }

    private var failedProbe: SystemIntegration.FullDiskAccessProbe {
        {
            throw NSError(domain: "PathlightTests", code: 1)
        }
    }
}

private final class WorkspaceSpy: SystemWorkspace {
    private let openResult: Bool
    private(set) var openedURLs: [URL] = []
    private(set) var revealedSelections: [[URL]] = []

    init(openResult: Bool) {
        self.openResult = openResult
    }

    func activateFileViewerSelecting(_ fileURLs: [URL]) {
        revealedSelections.append(fileURLs)
    }

    func open(_ url: URL) -> Bool {
        openedURLs.append(url)
        return openResult
    }
}
