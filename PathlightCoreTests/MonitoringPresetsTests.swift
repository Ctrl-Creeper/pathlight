import Foundation
import Testing
@testable import PathlightCore

@Suite("Monitoring presets")
struct MonitoringPresetsTests {
    @Test("offers only presets whose folder exists and merges exclusions")
    func filtersByExistence() {
        let home = URL(filePath: "/Users/example", directoryHint: .isDirectory)
        let existing: Set<String> = ["/Users/example/Downloads", "/Users/example/Library"]

        let presets = MonitoringPreset.available(home: home) { existing.contains($0.standardizedFileURL.path) }

        #expect(presets.map(\.id) == ["downloads", "library"])
        let library = presets[1].options
        #expect(library.exclusionPatterns.contains(".DS_Store"))
        #expect(library.exclusionPatterns.contains("Logs/"))
        #expect(library.minimumRecordedByteDelta == LongTermWatchTargetOptions.default.minimumRecordedByteDelta)
    }
}
