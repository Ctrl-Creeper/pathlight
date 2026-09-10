import Foundation
import Testing
@testable import PathlightCore

/// What a typed pattern is stored as. The matching those patterns drive lives
/// in the Rust core and is checked in `core/tests/exclusion.rs`, which is why
/// none of it is checked twice here.
@Suite("Activity exclusion patterns")
struct ActivityExclusionPatternsTests {
    @Test("a pattern is stored one way however it was typed")
    func aPatternIsStoredOneWayHoweverItWasTyped() {
        #expect(
            ActivityExclusionPatterns.normalized([
                " node_modules/ ",
                #"build\"#,
                "./target/",
                "/dist/",
                "Library//Caches/",
                "node_modules/",
                "",
                "   "
            ]) == [
                "node_modules/",
                "build/",
                "target/",
                "dist/",
                "Library/Caches/"
            ]
        )
    }

    @Test("patterns that are nothing but separators and space are dropped")
    func emptyPatternsAreDropped() {
        #expect(ActivityExclusionPatterns.normalized([]).isEmpty)
        #expect(ActivityExclusionPatterns.normalized(["   ", "", "/", "//", "./"]).isEmpty)
    }

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
