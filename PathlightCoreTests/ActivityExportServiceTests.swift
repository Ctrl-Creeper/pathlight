import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity export")
struct ActivityExportServiceTests {
    private let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

    @Test("writes CSV with escaped fields in chronological order")
    func writesCSV() {
        let later = DiskActivityEvent(kind: .created, path: root.appending(path: "b, \"quoted\".txt"), rootPath: root, timestamp: Date(timeIntervalSince1970: 200), byteDelta: 5, confidence: .confirmed, previousPath: nil, affectedItemCount: 1, processName: "Finder")
        let earlier = DiskActivityEvent(kind: .deleted, path: root.appending(path: "a.txt"), rootPath: root, timestamp: Date(timeIntervalSince1970: 100), byteDelta: nil, confidence: .unknown, previousPath: nil, affectedItemCount: 1)

        let csv = ActivityExportService.csv([later, earlier])
        let lines = csv.split(separator: "\n")

        #expect(lines[0] == "timestamp,kind,path,previousPath,byteDelta,confidence,affectedItemCount,process")
        #expect(lines[1] == "1970-01-01T00:01:40Z,deleted,/Users/example/Downloads/a.txt,,,unknown,1,")
        #expect(lines[2] == "1970-01-01T00:03:20Z,created,\"/Users/example/Downloads/b, \"\"quoted\"\".txt\",,5,confirmed,1,Finder")
    }

    @Test("session report groups by top-level item and lists processes")
    func sessionReport() {
        var session = WatchSessionModel(rootPath: root, startedAt: Date(timeIntervalSince1970: 0))
        session.append([
            DiskActivityEvent(kind: .created, path: root.appending(path: "Installer/payload.pkg"), rootPath: root, timestamp: Date(timeIntervalSince1970: 10), byteDelta: 3_000, confidence: .confirmed, previousPath: nil, affectedItemCount: 1, processName: "Installer"),
            DiskActivityEvent(kind: .deleted, path: root.appending(path: "old.dmg"), rootPath: root, timestamp: Date(timeIntervalSince1970: 20), byteDelta: -1_000, confidence: .estimated, previousPath: nil, affectedItemCount: 1)
        ])

        let report = ActivityExportService.sessionReport(session, endedAt: Date(timeIntervalSince1970: 30))

        #expect(report.hasPrefix("# Pathlight session: Downloads"))
        #expect(report.contains("| Installer | +3 KB | 1 |"))
        #expect(report.contains("| old.dmg | -1 KB | 1 |"))
        #expect(report.contains("- Installer: 1 events"))
        #expect(report.contains("deleted `/Users/example/Downloads/old.dmg` -1 KB"))
    }
}
