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

        let event = FSEventsChangeMapper.event(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemCreated),
            eventID: 101,
            timestamp: Date(timeIntervalSince1970: 10)
        )

        #expect(event == .change(DiskActivityChange(
            kind: .created,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 10)
        ), eventID: 101))
    }

    @Test("maps item removed flags")
    func mapsRemovedFlags() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "old.zip")

        let event = FSEventsChangeMapper.event(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemRemoved),
            eventID: 102,
            timestamp: Date(timeIntervalSince1970: 11)
        )

        #expect(event == .change(DiskActivityChange(
            kind: .deleted,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 11)
        ), eventID: 102))
    }

    @Test("maps item renamed flags without previous path")
    func mapsRenamedFlags() {
        let root = URL(filePath: "/Users/example/Desktop", directoryHint: .isDirectory)
        let file = root.appending(path: "renamed.mov")

        let event = FSEventsChangeMapper.event(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemRenamed),
            eventID: 103,
            timestamp: Date(timeIntervalSince1970: 12)
        )

        #expect(event == .change(DiskActivityChange(
            kind: .renamed(previousPath: nil),
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 12)
        ), eventID: 103))
    }

    @Test("maps metadata and content changes as modified")
    func mapsModifiedFlags() {
        let root = URL(filePath: "/Users/example/Library/Caches", directoryHint: .isDirectory)
        let file = root.appending(path: "cache.db")

        let event = FSEventsChangeMapper.event(
            path: file.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagItemModified | kFSEventStreamEventFlagItemInodeMetaMod),
            eventID: 104,
            timestamp: Date(timeIntervalSince1970: 13)
        )

        #expect(event == .change(DiskActivityChange(
            kind: .modified,
            path: file,
            rootPath: root,
            timestamp: Date(timeIntervalSince1970: 13)
        ), eventID: 104))
    }

    @Test("ignores stream lifecycle flags")
    func mapsHistoryCompletionAndRescanFlags() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)

        let history = FSEventsChangeMapper.event(
            path: root.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagHistoryDone),
            eventID: 105,
            timestamp: Date(timeIntervalSince1970: 14)
        )
        let rescan = FSEventsChangeMapper.event(
            path: root.path,
            root: root,
            flags: FSEventStreamEventFlags(kFSEventStreamEventFlagMustScanSubDirs),
            eventID: 106,
            timestamp: Date(timeIntervalSince1970: 14)
        )

        #expect(history == .historyCaughtUp(eventID: 105))
        #expect(rescan == .requiresRescan(eventID: 106))
    }
}
