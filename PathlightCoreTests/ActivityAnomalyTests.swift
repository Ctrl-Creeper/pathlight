import Foundation
import Testing
@testable import PathlightCore

/// What a finding says. What counts as a finding is the core's policy, and it
/// is checked in `core/tests/anomaly.rs`.
@Suite("Activity anomaly")
struct ActivityAnomalyTests {
    private let root = URL(filePath: "/Users/example/Documents", directoryHint: .isDirectory)

    @Test("names the folder and the window it was measured over")
    func describesTheFinding() {
        let burst = ActivityAnomaly(
            kind: .burst,
            rootPath: root,
            bytes: 6_000_000_000,
            itemCount: 3,
            windowMinutes: 10
        )
        #expect(burst.title == "Documents is filling up fast")
        #expect(burst.body.hasSuffix("written in the last 10 minutes."))
        #expect(burst.identifier == "pathlight-anomaly-burst-/Users/example/Documents")
    }

    /// A removal reports the size of what went, not a negative delta: the
    /// wording already says which direction this went.
    @Test("reports a removal as a count and a size")
    func describesARemoval() {
        let removal = ActivityAnomaly(
            kind: .massDeletion,
            rootPath: root,
            bytes: 2_000_000_000,
            itemCount: 120,
            windowMinutes: 10
        )
        #expect(removal.title == "Many files deleted in Documents")
        #expect(removal.body.hasPrefix("120 items ("))
        #expect(!removal.body.contains("-"))
    }
}
