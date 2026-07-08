# Live Activity Core Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first testable foundation for Pathlight's file-level timeline and storage attribution: event models, filtering, window aggregation, delete-size attribution, and Watch Session summaries.

**Architecture:** Add focused core model and service files inside `Pathlight/Services` so the feature can be exercised by SwiftPM tests before any FSEvents or SwiftUI work. The first slice accepts normalized raw path changes from tests or future monitors, samples sizes through injected closures, emits user-facing `DiskActivityEvent` values, and summarizes short-term sessions.

**Tech Stack:** Swift 6, Foundation, SwiftPM `swift test`, existing `PathlightCore` package.

---

## File Structure

- Create `Pathlight/Services/DiskActivityModels.swift`
  - Defines `DiskActivityChange`, `DiskActivityEvent`, `DiskActivityEventKind`, `DiskActivityEventConfidence`, `DiskActivityAggregationOptions`, and `WatchSessionSummary`.
- Create `Pathlight/Services/StorageAttributionService.swift`
  - Converts raw changes into thresholded, aggregated `DiskActivityEvent` values using injected size lookup and prior-size cache.
- Create `Pathlight/Services/WatchSessionModel.swift`
  - Maintains one short-term session timeline and net byte delta.
- Modify `Package.swift`
  - Adds the three new service files to the `PathlightCore` target sources.
- Create `PathlightCoreTests/StorageAttributionServiceTests.swift`
  - Covers threshold filtering, aggregation, delete known/unknown size handling, and move correlation fallback.
- Create `PathlightCoreTests/WatchSessionModelTests.swift`
  - Covers session summary calculations.

## Task 1: Disk Activity Models

**Files:**
- Create: `Pathlight/Services/DiskActivityModels.swift`
- Modify: `Package.swift`
- Test: `PathlightCoreTests/StorageAttributionServiceTests.swift`

- [ ] **Step 1: Write the failing model API test**

Add this test file:

```swift
import Foundation
import Testing
@testable import PathlightCore

@Suite("Storage attribution service")
struct StorageAttributionServiceTests {
    @Test("records a created file above the threshold")
    func recordsCreatedFileAboveThreshold() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        let file = root.appending(path: "movie.dmg")
        let service = StorageAttributionService(
            options: DiskActivityAggregationOptions(
                minimumRecordedByteDelta: 10,
                aggregationWindow: 300,
                longTermRecordsFileNames: false
            ),
            sizeProvider: { url in
                url.path == file.path ? 842 : nil
            }
        )

        let events = service.process([
            DiskActivityChange(kind: .created, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 100))
        ])

        #expect(events == [
            DiskActivityEvent(
                kind: .created,
                path: file,
                rootPath: root,
                timestamp: Date(timeIntervalSince1970: 100),
                byteDelta: 842,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        ])
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `swift test --filter StorageAttributionServiceTests/recordsCreatedFileAboveThreshold`

Expected: compile failure because `StorageAttributionService`, `DiskActivityAggregationOptions`, `DiskActivityChange`, and `DiskActivityEvent` do not exist.

- [ ] **Step 3: Add minimal model and service shells**

Create `Pathlight/Services/DiskActivityModels.swift` with:

```swift
import Foundation

struct DiskActivityChange: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case created
        case modified
        case deleted
        case renamed(previousPath: URL?)
    }

    let kind: Kind
    let path: URL
    let rootPath: URL
    let timestamp: Date
}

enum DiskActivityEventKind: Equatable, Sendable {
    case created
    case modified
    case deleted
    case moved
    case aggregate
}

enum DiskActivityEventConfidence: Equatable, Sendable {
    case confirmed
    case estimated
    case unknown
}

struct DiskActivityEvent: Equatable, Sendable {
    let kind: DiskActivityEventKind
    let path: URL
    let rootPath: URL
    let timestamp: Date
    let byteDelta: Int64?
    let confidence: DiskActivityEventConfidence
    let previousPath: URL?
    let affectedItemCount: Int
}

struct DiskActivityAggregationOptions: Equatable, Sendable {
    let minimumRecordedByteDelta: Int64
    let aggregationWindow: TimeInterval
    let longTermRecordsFileNames: Bool

    static let `default` = DiskActivityAggregationOptions(
        minimumRecordedByteDelta: 10 * 1_024 * 1_024,
        aggregationWindow: 5 * 60,
        longTermRecordsFileNames: false
    )
}
```

Create `Pathlight/Services/StorageAttributionService.swift` with:

```swift
import Foundation

struct StorageAttributionService {
    typealias SizeProvider = @Sendable (URL) -> Int64?

    private let options: DiskActivityAggregationOptions
    private let sizeProvider: SizeProvider

    init(
        options: DiskActivityAggregationOptions = .default,
        sizeProvider: @escaping SizeProvider
    ) {
        self.options = options
        self.sizeProvider = sizeProvider
    }

    func process(_ changes: [DiskActivityChange]) -> [DiskActivityEvent] {
        changes.compactMap { change in
            let size = sizeProvider(change.path)
            guard let size, abs(size) >= options.minimumRecordedByteDelta else {
                return nil
            }
            return DiskActivityEvent(
                kind: .created,
                path: change.path,
                rootPath: change.rootPath,
                timestamp: change.timestamp,
                byteDelta: size,
                confidence: .confirmed,
                previousPath: nil,
                affectedItemCount: 1
            )
        }
    }
}
```

Add both service files to `Package.swift` under `sources`.

- [ ] **Step 4: Run test to verify it passes**

Run: `swift test --filter StorageAttributionServiceTests/recordsCreatedFileAboveThreshold`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Package.swift Pathlight/Services/DiskActivityModels.swift Pathlight/Services/StorageAttributionService.swift PathlightCoreTests/StorageAttributionServiceTests.swift
git commit -m "feat(activity): add disk activity event foundation"
```

## Task 2: Threshold Filtering And Delete Attribution

**Files:**
- Modify: `Pathlight/Services/StorageAttributionService.swift`
- Test: `PathlightCoreTests/StorageAttributionServiceTests.swift`

- [ ] **Step 1: Add failing tests**

Append these tests inside `StorageAttributionServiceTests`:

```swift
@Test("filters events below the configured threshold")
func filtersEventsBelowThreshold() {
    let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
    let smallFile = root.appending(path: "small.txt")
    let service = StorageAttributionService(
        options: DiskActivityAggregationOptions(
            minimumRecordedByteDelta: 100,
            aggregationWindow: 300,
            longTermRecordsFileNames: false
        ),
        sizeProvider: { _ in 99 }
    )

    let events = service.process([
        DiskActivityChange(kind: .modified, path: smallFile, rootPath: root, timestamp: Date(timeIntervalSince1970: 100))
    ])

    #expect(events.isEmpty)
}

@Test("records deleted files with known prior size as negative delta")
func recordsDeletedFileWithKnownPriorSize() {
    let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
    let file = root.appending(path: "old.zip")
    var service = StorageAttributionService(
        options: DiskActivityAggregationOptions(
            minimumRecordedByteDelta: 10,
            aggregationWindow: 300,
            longTermRecordsFileNames: false
        ),
        sizeProvider: { _ in nil },
        priorSizeProvider: { url in
            url.path == file.path ? 500 : nil
        }
    )

    let events = service.process([
        DiskActivityChange(kind: .deleted, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
    ])

    #expect(events.first?.kind == .deleted)
    #expect(events.first?.byteDelta == -500)
    #expect(events.first?.confidence == .estimated)
}

@Test("records deleted files with unknown prior size")
func recordsDeletedFileWithUnknownPriorSize() {
    let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
    let file = root.appending(path: "missing.zip")
    var service = StorageAttributionService(
        options: DiskActivityAggregationOptions(
            minimumRecordedByteDelta: 10,
            aggregationWindow: 300,
            longTermRecordsFileNames: false
        ),
        sizeProvider: { _ in nil },
        priorSizeProvider: { _ in nil }
    )

    let events = service.process([
        DiskActivityChange(kind: .deleted, path: file, rootPath: root, timestamp: Date(timeIntervalSince1970: 200))
    ])

    #expect(events.first?.kind == .deleted)
    #expect(events.first?.byteDelta == nil)
    #expect(events.first?.confidence == .unknown)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `swift test --filter StorageAttributionServiceTests`

Expected: compile failure because the initializer lacks `priorSizeProvider`, or assertion failure because deletes are not handled.

- [ ] **Step 3: Implement delete attribution**

Update `StorageAttributionService` to accept `priorSizeProvider`, map create/modify/delete/rename kinds, and allow unknown-size deletes through even when they have no byte delta.

- [ ] **Step 4: Run test to verify it passes**

Run: `swift test --filter StorageAttributionServiceTests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Pathlight/Services/StorageAttributionService.swift PathlightCoreTests/StorageAttributionServiceTests.swift
git commit -m "feat(activity): attribute deleted file sizes"
```

## Task 3: Window Aggregation

**Files:**
- Modify: `Pathlight/Services/StorageAttributionService.swift`
- Test: `PathlightCoreTests/StorageAttributionServiceTests.swift`

- [ ] **Step 1: Add failing aggregation test**

Append:

```swift
@Test("aggregates many changes under the same parent inside the window")
func aggregatesChangesByParentInsideWindow() {
    let root = URL(filePath: "/Users/example/Library/Caches", directoryHint: .isDirectory)
    let parent = root.appending(path: "Browser")
    let changes = [
        DiskActivityChange(kind: .created, path: parent.appending(path: "a.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 100)),
        DiskActivityChange(kind: .created, path: parent.appending(path: "b.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 130)),
        DiskActivityChange(kind: .modified, path: parent.appending(path: "c.cache"), rootPath: root, timestamp: Date(timeIntervalSince1970: 140))
    ]
    let service = StorageAttributionService(
        options: DiskActivityAggregationOptions(
            minimumRecordedByteDelta: 10,
            aggregationWindow: 300,
            longTermRecordsFileNames: false
        ),
        sizeProvider: { _ in 50 }
    )

    let events = service.process(changes)

    #expect(events.count == 1)
    #expect(events.first?.kind == .aggregate)
    #expect(events.first?.path == parent)
    #expect(events.first?.byteDelta == 150)
    #expect(events.first?.affectedItemCount == 3)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `swift test --filter StorageAttributionServiceTests/aggregatesChangesByParentInsideWindow`

Expected: FAIL because the service emits three file events instead of one aggregate event.

- [ ] **Step 3: Implement parent-window aggregation**

Group thresholded events by root, parent path, and `floor(timestamp / aggregationWindow)`. If a group contains more than one event and `longTermRecordsFileNames == false`, emit one `.aggregate` event at the parent with summed known deltas and `affectedItemCount`.

- [ ] **Step 4: Run test to verify it passes**

Run: `swift test --filter StorageAttributionServiceTests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Pathlight/Services/StorageAttributionService.swift PathlightCoreTests/StorageAttributionServiceTests.swift
git commit -m "feat(activity): aggregate storage changes by window"
```

## Task 4: Watch Session Summary

**Files:**
- Create: `Pathlight/Services/WatchSessionModel.swift`
- Modify: `Package.swift`
- Test: `PathlightCoreTests/WatchSessionModelTests.swift`

- [ ] **Step 1: Write failing Watch Session test**

Create:

```swift
import Foundation
import Testing
@testable import PathlightCore

@Suite("Watch session model")
struct WatchSessionModelTests {
    @Test("summarizes net growth and event counts")
    func summarizesNetGrowthAndEventCounts() {
        let root = URL(filePath: "/Users/example/Downloads", directoryHint: .isDirectory)
        var session = WatchSessionModel(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
            rootPath: root,
            startedAt: Date(timeIntervalSince1970: 100)
        )

        session.append([
            DiskActivityEvent(kind: .created, path: root.appending(path: "a.dmg"), rootPath: root, timestamp: Date(timeIntervalSince1970: 110), byteDelta: 800, confidence: .confirmed, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .deleted, path: root.appending(path: "b.zip"), rootPath: root, timestamp: Date(timeIntervalSince1970: 120), byteDelta: -300, confidence: .estimated, previousPath: nil, affectedItemCount: 1),
            DiskActivityEvent(kind: .deleted, path: root.appending(path: "c.zip"), rootPath: root, timestamp: Date(timeIntervalSince1970: 130), byteDelta: nil, confidence: .unknown, previousPath: nil, affectedItemCount: 1)
        ])

        let summary = session.summary(endedAt: Date(timeIntervalSince1970: 200))

        #expect(summary.netByteDelta == 500)
        #expect(summary.eventCount == 3)
        #expect(summary.unknownSizeEventCount == 1)
        #expect(summary.startedAt == Date(timeIntervalSince1970: 100))
        #expect(summary.endedAt == Date(timeIntervalSince1970: 200))
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `swift test --filter WatchSessionModelTests`

Expected: compile failure because `WatchSessionModel` and `WatchSessionSummary` do not exist.

- [ ] **Step 3: Implement session model**

Add `WatchSessionSummary` to `DiskActivityModels.swift` and implement `WatchSessionModel`.

- [ ] **Step 4: Run test to verify it passes**

Run: `swift test --filter WatchSessionModelTests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Package.swift Pathlight/Services/DiskActivityModels.swift Pathlight/Services/WatchSessionModel.swift PathlightCoreTests/WatchSessionModelTests.swift
git commit -m "feat(activity): summarize watch sessions"
```

## Task 5: Full SwiftPM Verification

**Files:**
- No new files.

- [ ] **Step 1: Run focused activity tests**

Run: `swift test --filter StorageAttributionServiceTests && swift test --filter WatchSessionModelTests`

Expected: PASS.

- [ ] **Step 2: Run full package tests**

Run: `swift test`

Expected: PASS. If unrelated existing tests fail, capture the failures and do not claim full pass.

- [ ] **Step 3: Commit plan if it is not already committed**

```bash
git add docs/superpowers/plans/2026-07-08-live-activity-core-foundation.md
git commit -m "docs: plan live activity core foundation"
```

## Spec Coverage Review

- File-level timeline foundation: covered by `DiskActivityEvent` and service tests.
- Storage attribution: covered by create, modify, delete, and aggregate byte deltas.
- Short-term Watch Session: covered by `WatchSessionModel`.
- Long-term Storage History persistence, FSEvents monitor, background login item, charts, and SwiftUI Activity view are intentionally deferred to the next implementation slices after this core passes tests.
- Pro app/process attribution remains out of scope.
