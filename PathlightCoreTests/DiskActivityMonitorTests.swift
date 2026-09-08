import Foundation
import XCTest
@testable import PathlightCore

@MainActor
final class DiskActivityMonitorTests: XCTestCase {
    func testFreshMonitorReportsReadinessWithoutFilesystemChanges() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "PathlightReadiness-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let stream = FSEventsDiskActivityMonitor().events(for: root, since: nil, latency: 30)

        let ready = await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                for await event in stream {
                    if case .historyCaughtUp(eventID: 0) = event { return true }
                }
                return false
            }
            group.addTask {
                try? await Task.sleep(for: .seconds(1))
                return false
            }
            let firstResult = await group.next() ?? false
            group.cancelAll()
            return firstResult
        }

        XCTAssertTrue(ready, "SinceNow has no HistoryDone callback; successful startup must announce readiness")
    }
}
