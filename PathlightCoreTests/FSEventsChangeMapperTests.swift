import CoreServices
import Foundation
import Testing
@testable import PathlightCore

@Suite("FSEvents change mapper")
struct FSEventsChangeMapperTests {
    @Test("maps item created flags")
    func mapsCreatedFlags() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "new.dmg")

        let change = FSEventsChangeMapper.change(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemCreated),
            timestamp: Date(timeIntervalSince1970: 10)
        )

        #expect(change?.kind == .created)
        #expect(change?.path == file)
        #expect(change?.rootPath == root)
    }

    @Test("maps item removed flags")
    func mapsRemovedFlags() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "old.zip")

        let change = FSEventsChangeMapper.change(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemRemoved),
            timestamp: Date(timeIntervalSince1970: 11)
        )

        #expect(change?.kind == .deleted)
    }

    @Test("maps item renamed flags without previous path")
    func mapsRenamedFlags() {
        let root = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let file = root.appending(path: "renamed.mov")

        let change = FSEventsChangeMapper.change(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemRenamed),
            timestamp: Date(timeIntervalSince1970: 12)
        )

        #expect(change?.kind == .renamed(previousPath: nil))
    }

    @Test("maps metadata and content changes as modified")
    func mapsModifiedFlags() {
        let root = URL(filePath: "/Users/example/Library/Caches", directoryHint: .isDirectory)
        let file = root.appending(path: "cache.db")

        let change = FSEventsChangeMapper.change(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemModified | kFSEventStreamEventFlagItemInodeMetaMod),
            timestamp: Date(timeIntervalSince1970: 13)
        )

        #expect(change?.kind == .modified)
    }

    @Test("ignores stream lifecycle flags")
    func ignoresLifecycleFlags() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

        let change = FSEventsChangeMapper.change(
            path: root.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagHistoryDone),
            timestamp: Date(timeIntervalSince1970: 14)
        )

        #expect(change == nil)
    }
}
