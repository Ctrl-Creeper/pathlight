import Foundation
import Testing
@testable import PathlightCore

/// What a watch's stored pattern list holds. Normalizing and matching live in
/// the Rust core and are checked in `core/tests/exclusion.rs`, not twice here.
@Suite("Activity exclusion patterns")
struct ActivityExclusionPatternsTests {
    @Test("legacy persisted options adopt default patterns")
    func legacyPersistedOptionsAdoptDefaultPatterns() throws {
        let legacyJSON = Data("""
        {
            "minimumRecordedByteDelta": 1024,
            "aggregationWindow": 300,
            "recordsFileNames": true
        }
        """.utf8)

        let options = try JSONDecoder().decode(LongTermWatchTargetOptions.self, from: legacyJSON)
        #expect(options.exclusionPatterns == ActivityExclusionPatterns.defaults)

        let roundTripped = try JSONDecoder().decode(
            LongTermWatchTargetOptions.self,
            from: JSONEncoder().encode(
                LongTermWatchTargetOptions(
                    minimumRecordedByteDelta: 1,
                    aggregationWindow: 0,
                    recordsFileNames: true,
                    exclusionPatterns: []
                )
            )
        )
        #expect(roundTripped.exclusionPatterns.isEmpty)
    }
}
