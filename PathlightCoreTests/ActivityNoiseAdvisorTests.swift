import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity noise advisor")
struct ActivityNoiseAdvisorTests {
    private let root = URL(filePath: "/Users/example/Project", directoryHint: .isDirectory)

    @Test("suggests the directory that dominates a storm")
    func suggestsDominantDirectory() {
        var events: [DiskActivityEvent] = []
        for index in 0..<300 {
            events.append(event(path: "node_modules/pkg/file\(index).js"))
        }
        for index in 0..<50 {
            events.append(event(path: "src/file\(index).swift"))
        }

        let suggestion = ActivityNoiseAdvisor.suggestion(for: events, rootPath: root)

        #expect(suggestion?.pattern == "node_modules/pkg/")
        #expect(suggestion?.eventCount == 300)
        #expect(suggestion.map { $0.shareOfTotal > 0.8 } == true)
    }

    @Test("stays quiet for small or evenly spread sessions")
    func staysQuietWhenNotNoisy() {
        let few = (0..<50).map { event(path: "cache/\($0)") }
        #expect(ActivityNoiseAdvisor.suggestion(for: few, rootPath: root) == nil)

        let spread = (0..<400).map { event(path: "dir\($0 % 10)/file\($0)") }
        #expect(ActivityNoiseAdvisor.suggestion(for: spread, rootPath: root) == nil)

        let atRoot = (0..<400).map { event(path: "file\($0)") }
        #expect(ActivityNoiseAdvisor.suggestion(for: atRoot, rootPath: root) == nil)
    }

    private func event(path: String) -> DiskActivityEvent {
        DiskActivityEvent(
            kind: .modified,
            path: root.appending(path: path),
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 100),
            byteDelta: 1,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
    }
}
