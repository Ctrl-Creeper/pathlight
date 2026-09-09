# Live Monitor FSEvents Foundation

## Goal

Add the first real macOS listening layer for Pathlight activity tracking. This slice should convert FSEvents file-level notifications into `DiskActivityChange` values without adding UI or rollback behavior.

## Scope

- Add a `DiskActivityMonitoring` protocol that returns an `AsyncStream<DiskActivityChange>` for a watched root.
- Add a pure `FSEventsChangeMapper` so FSEvents flag semantics are testable without relying on live filesystem timing.
- Add `FSEventsDiskActivityMonitor` using macOS FSEvents with file-level notifications.
- Register the new source file in `Package.swift`.

## Tests

- Verify create, delete, rename, and modify flags map to the expected `DiskActivityChange.Kind`.
- Verify stream lifecycle indirectly through compilation; live FSEvents integration will be covered later by an integration test harness once UI/session wiring exists.

## Notes

- Process/app attribution is intentionally out of scope for this slice.
- Rename history is best-effort: FSEvents reports a renamed item but does not reliably provide the old path in a single event, so `previousPath` starts as `nil`.
