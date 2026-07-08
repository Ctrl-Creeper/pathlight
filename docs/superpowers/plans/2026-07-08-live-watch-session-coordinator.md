# Live Watch Session Coordinator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect raw disk activity changes to continuously updated watch session timelines.

**Architecture:** `LiveWatchSessionCoordinator` consumes a `DiskActivityMonitoring` stream, processes each change through `StorageAttributionService`, appends attributed events to `WatchSessionModel`, and yields session snapshots. Tests use a fake monitor so behavior stays deterministic.

**Tech Stack:** Swift 6, Swift Testing, AsyncStream, existing Pathlight activity models.

---

### Task 1: Stream Attributed Session Updates

**Files:**
- Create: `Pathlight/Services/LiveWatchSessionCoordinator.swift`
- Modify: `Package.swift`
- Test: `PathlightCoreTests/LiveWatchSessionCoordinatorTests.swift`

- [ ] **Step 1: Write failing tests**

Add tests that expect a coordinator to publish an empty initial session, then a session containing attributed events from a fake monitor. Add a second test that verifies filtered events do not publish empty update snapshots.

- [ ] **Step 2: Run the focused test**

Run: `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --filter LiveWatchSessionCoordinatorTests`

Expected: fail because `LiveWatchSessionCoordinator` does not exist yet.

- [ ] **Step 3: Implement the coordinator**

Create a small coordinator with `sessions(rootPath:startedAt:options:sizeProvider:priorSizeProvider:) -> AsyncStream<WatchSessionModel>`.

- [ ] **Step 4: Verify**

Run the focused test, full `swift test`, and the macOS Xcode build.

- [ ] **Step 5: Commit and push**

Commit with `feat(activity): stream live watch session updates` and push `main`.
