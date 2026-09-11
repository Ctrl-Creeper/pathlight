# Pathlight Reliability Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every reliability, cross-host consistency, retention, lifecycle, and release issue found in the repository audit.

**Architecture:** Make the storage directory the synchronization boundary shared by every host, and make a watch session own and drain its worker. Use a persisted records generation to distinguish data accepted before a reset, use one file-backed encryption key on macOS and Rust hosts, and derive long-lived history and daily growth from the journal rather than process lifetime.

**Tech Stack:** Rust 2021, Swift 6, SwiftUI, Swift concurrency, POSIX file locks on macOS, Rust standard-library file locks, GitHub Actions.

**Spec:** Repository audit reported in the preceding user-visible review.

## Global Constraints

- macOS 14.0+, Swift 6, Rust stable.
- Monitoring remains kernel-driven and does not poll watched folders.
- Pathlight never mutates watched user files.
- Pathlight storage remains excluded from every watch.
- No new dependency unless the standard library and existing code cannot provide the behavior.
- Every persisted format change remains backward compatible.

---

### Task 1: Cross-Process Storage Transactions

**Files:**
- Modify: `core/src/store.rs`
- Modify: `core/src/attribution.rs`
- Modify: `Pathlight/Services/ActivityStorageFileProtection.swift`
- Modify: `Pathlight/Services/ActivityEventStore.swift`
- Modify: `Pathlight/Services/ActivitySizeIndex.swift`
- Test: `core/src/store.rs`
- Test: `core/tests/journal.rs`
- Test: `PathlightCoreTests/ActivityEventStoreTests.swift`

**Interfaces:**
- Produces: one `pathlight.lock` file per storage directory; all journal, index, settings, and reset mutations hold it.
- Produces: settings setters perform read-modify-write while holding the lock and publish a complete replacement file.

- [x] Write tests proving a held storage transaction excludes a competing writer and settings updates retain unrelated fields.
- [x] Run the focused tests and verify the missing transaction behavior fails.
- [x] Implement Rust storage-file locking, locked settings updates, and atomic settings replacement.
- [x] Add the matching Swift lock around event-journal, size-index, and reset mutations.
- [x] Run Rust and Swift-focused tests; where the local Swift Testing module is unavailable, at least compile the Swift library and retain the CI test coverage.

### Task 2: Session Shutdown and Cross-Process Pause

**Files:**
- Modify: `core/src/watch.rs`
- Modify: `core/src/bin/pathlight-monitor.rs`
- Modify: `gui/src/main.rs`
- Test: `core/src/watch.rs`
- Test: `core/tests/cli.rs`
- Test: `gui/src/main.rs`

**Interfaces:**
- Produces: `Session::stop`/`Drop` stop the watcher and join the worker after its final flush.
- Produces: CLI and GUI periodically reconcile their in-memory paused state with shared settings, stopping and reopening the same roots.

- [x] Write a test proving dropping a Session durably flushes an accepted tail event.
- [x] Write host tests proving an externally changed pause is observed without restarting the host.
- [x] Run the focused tests and verify they fail for the current detached/static behavior.
- [x] Make Session own its watcher and `JoinHandle`, and drain it on shutdown.
- [x] Reconcile pause state in CLI and GUI outside event callbacks and the paint hot path.
- [x] Run the focused tests and the isolated two-process pause reproduction.

### Task 3: Durable Reset Boundary and Overflow Evidence

**Files:**
- Modify: `core/src/store.rs`
- Modify: `core/src/watch.rs`
- Modify: `Pathlight/Services/AppDependencies.swift`
- Modify: `Pathlight/Services/ActivityEventStore.swift`
- Modify: `Pathlight/Services/ActivitySizeIndex.swift`
- Modify: `Pathlight/ViewModels/AppModel.swift`
- Test: `core/src/watch.rs`
- Test: `core/tests/cli.rs`
- Test: `PathlightCoreTests/AppModelJournalTests.swift`

**Interfaces:**
- Produces: a backward-compatible `records_epoch` setting incremented by every record reset.
- Produces: workers discard pre-reset pending data and reset their in-memory index/baseline when the epoch changes.
- Produces: queue overflow is recorded as a gap, reconciled from baseline, written to the diary, and printed by CLI.

- [x] Write tests reproducing queued rows returning after reset and overflow leaving no durable evidence.
- [x] Run them and verify both failures.
- [x] Add the generation barrier to Rust reset/watch flow and actor-serialized reset hooks to the macOS app.
- [x] Route queue overflow through the same reconciliation and diary path as backend gaps; print new dropped/gap counts in CLI.
- [x] Run focused tests and repeat the delayed-flush reset reproduction.

### Task 4: Shared Encryption Key

**Files:**
- Modify: `Pathlight/Services/ActivityStorageLineCodec.swift`
- Modify: `Pathlight/App/RustActivityStorageCryptor.swift`
- Modify: `Pathlight/Features/Settings/SettingsView.swift`
- Modify: `core/src/crypt.rs`
- Test: `PathlightCoreTests/ActivityStorageLineCodecTests.swift`
- Test: `core/tests/journal.rs`

**Interfaces:**
- Produces: both macOS app and Rust hosts load `activity-events.key` with mode `0600`.
- Consumes: an existing Keychain key only as a one-time migration source when no shared key file exists.

- [x] Write migration tests: an existing Keychain-style key seeds the shared file, and an existing shared file wins a creation race.
- [x] Verify the tests fail while macOS still reads only Keychain.
- [x] Implement a locked shared-file key provider with legacy Keychain migration and update user-facing storage wording.
- [x] Update core documentation to describe one shared key and verify encrypted journal round trips remain green.

### Task 5: Long-Term History and Daily Growth

**Files:**
- Modify: `core/src/journal.rs`
- Modify: `core/src/store.rs`
- Modify: `core/src/watch.rs`
- Modify: `core/tests/journal.rs`
- Modify: `README.md`
- Modify: `core/README.md`

**Interfaces:**
- Produces: `Journal::trim` rolls expired detailed rows into one UTC-day aggregate per root before enforcing the cap.
- Produces: Session alerts initialize today's positive growth from retained journal rows.

- [x] Write a journal test proving old named rows become one path-free daily aggregate.
- [x] Write an alert test proving growth recorded before Session creation contributes to the threshold.
- [x] Run both tests and verify the absent rollup/restart state fails.
- [x] Implement aggregation with unknown-size/confidence propagation matching Swift.
- [x] Initialize the in-memory daily alert accumulator once at watch startup from journal history.
- [x] Run focused and full core tests.

### Task 6: macOS Gap Acknowledgement and Release Packaging

**Files:**
- Modify: `Pathlight/ViewModels/AppModel.swift`
- Modify: `PathlightCoreTests/AppModelJournalTests.swift`
- Modify: `.github/workflows/release.yml`
- Modify: `README.md`

**Interfaces:**
- Produces: clearing a gap also clears pending copies, and an in-flight stale checkpoint cannot restore it.
- Produces: Linux release installs the same native build dependencies as CI.

- [x] Write an AppModel regression test that acknowledges a gap during a blocked journal append.
- [x] Verify it fails when the in-flight checkpoint restores `hasHistoryGap`.
- [x] Normalize pending and in-flight checkpoints against the latest acknowledged state.
- [x] Add GTK3, libxdo, and Ayatana AppIndicator development packages to the Linux release job and correct build requirements.
- [x] Run YAML sanity checks, repository formatting/lint, all Rust tests, Swift build/tests where available, and confirm a clean scoped diff.
