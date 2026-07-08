import CoreServices
import Foundation

protocol DiskActivityMonitoring: Sendable {
    nonisolated func changes(for root: URL) -> AsyncStream<DiskActivityChange>
}

enum FSEventsChangeMapper {
    nonisolated static func change(
        path: String,
        root: URL,
        flags: FSEventStreamEventFlags,
        timestamp: Date = Date()
    ) -> DiskActivityChange? {
        guard !flags.contains(anyOf: [
            kFSEventStreamEventFlagHistoryDone,
            kFSEventStreamEventFlagRootChanged,
            kFSEventStreamEventFlagMount,
            kFSEventStreamEventFlagUnmount
        ]) else {
            return nil
        }

        let eventPath = URL(filePath: path)
        let kind: DiskActivityChange.Kind
        if flags.contains(kFSEventStreamEventFlagItemRemoved) {
            kind = .deleted
        } else if flags.contains(kFSEventStreamEventFlagItemRenamed) {
            kind = .renamed(previousPath: nil)
        } else if flags.contains(kFSEventStreamEventFlagItemCreated) {
            kind = .created
        } else {
            kind = .modified
        }

        return DiskActivityChange(
            kind: kind,
            path: eventPath,
            rootPath: root.standardizedFileURL,
            timestamp: timestamp
        )
    }
}

final class FSEventsDiskActivityMonitor: DiskActivityMonitoring, @unchecked Sendable {
    nonisolated func changes(for root: URL) -> AsyncStream<DiskActivityChange> {
        let watchedRoot = root.standardizedFileURL
        return AsyncStream { continuation in
            let streamBox = FSEventsStreamBox(root: watchedRoot, continuation: continuation)
            continuation.onTermination = { @Sendable [weak streamBox] _ in
                streamBox?.stop()
            }
            streamBox.start()
        }
    }
}

private final class FSEventsStreamBox: @unchecked Sendable {
    private let root: URL
    private let continuation: AsyncStream<DiskActivityChange>.Continuation
    private let queue: DispatchQueue
    private let lock = NSLock()
    nonisolated(unsafe) private var stream: FSEventStreamRef?

    nonisolated init(root: URL, continuation: AsyncStream<DiskActivityChange>.Continuation) {
        self.root = root
        self.continuation = continuation
        queue = DispatchQueue(label: "app.pathlight.disk-activity.fsevents.\(root.path.hashValue)")
    }

    nonisolated func start() {
        lock.lock()
        defer { lock.unlock() }

        guard stream == nil else {
            return
        }

        var context = FSEventStreamContext(
            version: 0,
            info: Unmanaged.passUnretained(self).toOpaque(),
            retain: Self.retainContext,
            release: Self.releaseContext,
            copyDescription: nil
        )
        let flags = FSEventStreamCreateFlags(
            kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer
        )

        guard let createdStream = FSEventStreamCreate(
            nil,
            Self.callback,
            &context,
            [root.path] as CFArray,
            FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
            0.25,
            flags
        ) else {
            continuation.finish()
            return
        }

        stream = createdStream
        FSEventStreamSetDispatchQueue(createdStream, queue)
        guard FSEventStreamStart(createdStream) else {
            stream = nil
            FSEventStreamInvalidate(createdStream)
            FSEventStreamRelease(createdStream)
            continuation.finish()
            return
        }
    }

    nonisolated func stop() {
        lock.lock()
        let activeStream = stream
        stream = nil
        lock.unlock()

        guard let activeStream else {
            return
        }

        FSEventStreamStop(activeStream)
        FSEventStreamInvalidate(activeStream)
        FSEventStreamRelease(activeStream)
        continuation.finish()
    }

    private nonisolated func handle(
        eventCount: Int,
        eventPaths: UnsafeMutableRawPointer,
        eventFlags: UnsafePointer<FSEventStreamEventFlags>
    ) {
        let paths = unsafeBitCast(eventPaths, to: NSArray.self)
        guard let stringPaths = paths as? [String] else {
            return
        }

        let timestamp = Date()
        for index in 0..<min(eventCount, stringPaths.count) {
            guard let change = FSEventsChangeMapper.change(
                path: stringPaths[index],
                root: root,
                flags: eventFlags[index],
                timestamp: timestamp
            ) else {
                continue
            }
            continuation.yield(change)
        }
    }

    nonisolated(unsafe) private static let callback: FSEventStreamCallback = { _, info, eventCount, eventPaths, eventFlags, _ in
        guard let info else {
            return
        }

        let streamBox = Unmanaged<FSEventsStreamBox>.fromOpaque(info).takeUnretainedValue()
        streamBox.handle(eventCount: eventCount, eventPaths: eventPaths, eventFlags: eventFlags)
    }

    nonisolated(unsafe) private static let retainContext: CFAllocatorRetainCallBack = { info in
        guard let info else {
            return nil
        }
        return UnsafeRawPointer(Unmanaged<FSEventsStreamBox>.fromOpaque(info).retain().toOpaque())
    }

    nonisolated(unsafe) private static let releaseContext: CFAllocatorReleaseCallBack = { info in
        guard let info else {
            return
        }
        Unmanaged<FSEventsStreamBox>.fromOpaque(info).release()
    }
}

private extension FSEventStreamEventFlags {
    nonisolated func contains(_ flag: Int) -> Bool {
        self & FSEventStreamEventFlags(flag) != 0
    }

    nonisolated func contains(anyOf flags: [Int]) -> Bool {
        flags.contains { contains($0) }
    }
}
