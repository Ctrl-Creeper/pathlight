import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity exclusion filter")
struct ActivityExclusionFilterTests {
    private let root = URL(filePath: "/Users/example", directoryHint: .isDirectory)

    @Test("empty patterns disable filtering entirely")
    func emptyPatternsDisableFiltering() {
        #expect(ActivityExclusionFilter(patterns: [], rootPath: root) == nil)
        #expect(ActivityExclusionFilter(patterns: ["   ", ""], rootPath: root) == nil)
    }

    @Test("matches leaf names and extensions")
    func matchesLeafNamesAndExtensions() throws {
        let filter = try #require(ActivityExclusionFilter(
            patterns: [".DS_Store", "*.tmp"],
            rootPath: root
        ))

        #expect(filter.excludes(root.appending(path: "Documents/.DS_Store")))
        #expect(filter.excludes(root.appending(path: "scratch.tmp")))
        #expect(!filter.excludes(root.appending(path: "Documents/report.pdf")))
    }

    @Test("directory patterns exclude deep descendants")
    func directoryPatternsExcludeDeepDescendants() throws {
        let filter = try #require(ActivityExclusionFilter(
            patterns: [".Trash/", "**/Caches/"],
            rootPath: root
        ))

        #expect(filter.excludes(root.appending(path: ".Trash/old/movie.mov")))
        #expect(filter.excludes(root.appending(path: "Library/Caches/com.app/blob.bin")))
        #expect(filter.excludes(root.appending(path: "Library/Caches")))
        #expect(!filter.excludes(root.appending(path: "Library/Preferences/com.app.plist")))
    }

    @Test("deleted directory leaf still matches directory-only patterns")
    func deletedDirectoryLeafMatchesDirectoryPatterns() throws {
        let filter = try #require(ActivityExclusionFilter(
            patterns: ["node_modules/"],
            rootPath: root
        ))

        // The path is gone, so the filter can't stat it; directory-only
        // patterns must still match the bare leaf.
        #expect(filter.excludes(root.appending(path: "project/node_modules")))
    }

    @Test("paths outside the root are never excluded")
    func pathsOutsideRootAreNeverExcluded() throws {
        let filter = try #require(ActivityExclusionFilter(
            patterns: [".DS_Store"],
            rootPath: root.appending(path: "Downloads")
        ))

        #expect(!filter.excludes(URL(filePath: "/Users/example/Documents/.DS_Store")))
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
        #expect(options.exclusionPatterns == ActivityExclusionFilter.defaultPatterns)

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
