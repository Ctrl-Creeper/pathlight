import CoreServices
import Foundation

protocol DiskActivityMonitoring: Sendable {
    nonisolated func events(
        for root: URL,
        since eventID: UInt64?,
        latency: TimeInterval
    ) -> AsyncStream<DiskActivityStreamEvent>
}

enum FSEventsChangeMapper {
    nonisolated static func event(
        path: String,
        root: URL,
        flags: FSEventStreamEventFlags,
        eventID: UInt64,
        timestamp: Date = Date()
    ) -> DiskActivityStreamEvent {
        if flags.contains(kFSEventStreamEventFlagHistoryDone) {
            return .historyCaughtUp(eventID: eventID)
        }
        if flags.contains(anyOf: [
            kFSEventStreamEventFlagMustScanSubDirs,
            kFSEventStreamEventFlagUserDropped,
            kFSEventStreamEventFlagKernelDropped,
            kFSEventStreamEventFlagEventIdsWrapped,
            kFSEventStreamEventFlagRootChanged,
            kFSEventStreamEventFlagMount,
            kFSEventStreamEventFlagUnmount
        ]) {
            return .requiresRescan(eventID: eventID)
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

        return .change(
            DiskActivityChange(
                kind: kind,
                path: eventPath,
                rootPath: root.standardizedFileURL,
                timestamp: timestamp
            ),
            eventID: eventID
        )
    }
}

enum FSEventsPathDecoder {
    // Positions must stay aligned with the callback's flags/eventID arrays,
    // so nil path pointers are preserved instead of compacted away.
    nonisolated static func paths(
        eventCount: Int,
        eventPaths: UnsafeMutableRawPointer
    ) -> [String?] {
        let pathPointers = eventPaths.assumingMemoryBound(to: UnsafePointer<CChar>?.self)
        return (0..<eventCount).map { index in
            pathPointers[index].map { String(cString: $0) }
        }
    }
}

final class FSEventsDiskActivityMonitor: DiskActivityMonitoring, @unchecked Sendable {
    nonisolated func events(
        for root: URL,
        since eventID: UInt64?,
        latency: TimeInterval
    ) -> AsyncStream<DiskActivityStreamEvent> {
        let watchedRoot = root.standardizedFileURL
        return AsyncStream { continuation in
            let streamBox = FSEventsStreamBox(
                root: watchedRoot,
                sinceEventID: eventID,
                latency: latency,
                continuation: continuation
            )
            continuation.onTermination = { @Sendable [weak streamBox] _ in
                streamBox?.stop()
            }
            streamBox.start()
        }
    }
}

private final class FSEventsStreamBox: @unchecked Sendable {
    private let root: URL
    private let sinceEventID: UInt64?
    private let latency: TimeInterval
    private let continuation: AsyncStream<DiskActivityStreamEvent>.Continuation
    private let queue: DispatchQueue
    private let lock = NSLock()
    nonisolated(unsafe) private var stream: FSEventStreamRef?

    nonisolated init(
        root: URL,
        sinceEventID: UInt64?,
        latency: TimeInterval,
        continuation: AsyncStream<DiskActivityStreamEvent>.Continuation
    ) {
        self.root = root
        self.sinceEventID = sinceEventID
        self.latency = latency
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
        // NoDefer fires the first event immediately, which is what an interactive
        // live monitor wants; relaxed background watches let the kernel batch the
        // full latency window so the process wakes far less often.
        var rawFlags = kFSEventStreamCreateFlagFileEvents
        if latency < 1 {
            rawFlags |= kFSEventStreamCreateFlagNoDefer
        }
        let flags = FSEventStreamCreateFlags(rawFlags)

        guard let createdStream = FSEventStreamCreate(
            nil,
            Self.callback,
            &context,
            [root.path] as CFArray,
            sinceEventID.map { FSEventStreamEventId($0) } ?? FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
            latency,
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
        eventFlags: UnsafePointer<FSEventStreamEventFlags>,
        eventIDs: UnsafePointer<FSEventStreamEventId>
    ) {
        let stringPaths = FSEventsPathDecoder.paths(
            eventCount: eventCount,
            eventPaths: eventPaths
        )

        let timestamp = Date()
        for index in 0..<eventCount {
            guard let path = stringPaths[index] else {
                continue
            }
            let event = FSEventsChangeMapper.event(
                path: path,
                root: root,
                flags: eventFlags[index],
                eventID: UInt64(eventIDs[index]),
                timestamp: timestamp
            )
            continuation.yield(event)
        }
    }

    nonisolated(unsafe) private static let callback: FSEventStreamCallback = { _, info, eventCount, eventPaths, eventFlags, eventIDs in
        guard let info else {
            return
        }

        let streamBox = Unmanaged<FSEventsStreamBox>.fromOpaque(info).takeUnretainedValue()
        streamBox.handle(
            eventCount: eventCount,
            eventPaths: eventPaths,
            eventFlags: eventFlags,
            eventIDs: eventIDs
        )
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
