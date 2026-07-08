# Pathlight Live Monitor Design

## Summary

Pathlight extends Radix from a static disk space analyzer into a storage change monitor. The first version adds file-level timeline and storage attribution without promising app/process attribution. It uses macOS file system change observation to answer a different question from system monitors: not "how busy is the disk," but "where did storage change, and when?"

The feature has two user-facing modes:

- Short-term Watch Session: detailed, temporary monitoring while the user performs a task.
- Long-term Storage History: background trend tracking for selected locations, with aggregated logs and charts.

Endpoint Security-based app attribution is explicitly out of scope for this first version. It remains a future Pro capability because it requires higher-risk permissions, entitlement review, and a more complex installation story.

## Goals

- Show live file and folder changes for user-selected locations.
- Attribute storage growth or shrinkage to paths and watched folders.
- Provide a detailed timeline for temporary sessions.
- Provide long-term size trends and important historical events for configured folders.
- Keep privacy, energy use, and log volume under user control.
- Preserve Radix's existing scan, sunburst, file browser, and snapshot architecture.

## Non-Goals

- Do not record which app or process caused a change in the first version.
- Do not implement rollback, file restoration, or version history.
- Do not monitor every protected location by default.
- Do not persist every tiny file event indefinitely.
- Do not replace the existing scan snapshot model with a fully live mutable tree.

## Product Shape

### Short-Term Watch Session

Watch Session is an explicit, temporary observation mode.

The user chooses a folder or volume and starts watching. Pathlight records detailed events for that session, shows a live timeline, and summarizes size changes since the session began. When the user stops the session, they can keep the summary or discard it.

Expected examples:

- A `.dmg` appears in Downloads and Downloads grows by 842 MB.
- `DerivedData` shrinks by 1.2 GB after a cleanup.
- A cache directory receives many small files and grows by 126 MB over a few minutes.

Watch Session favors detail over long retention. File-level entries are acceptable here because the user intentionally started the session to inspect near-term behavior.

### Long-Term Storage History

Storage History is an opt-in background feature. The user selects watched locations during onboarding or settings configuration. Pathlight then tracks size samples and important aggregated changes over time.

Each watched location can show:

- A size trend chart over time.
- Net growth or shrinkage for common ranges such as today, 7 days, 30 days, and all time.
- A historical event list for meaningful changes.
- Drill-down into the largest changed child folders when available.

Storage History favors aggregation over exhaustive logs. It should feel like a storage history and trend tool, not surveillance.

## Default Watched Locations

Pathlight may suggest these locations, but should not enable them silently:

- Downloads
- Desktop
- Documents
- Applications
- Developer folders when present, such as `~/Developer`
- User cache folders, such as `~/Library/Caches`, when permission allows

The user must confirm the selected locations before background monitoring begins.

## Event Filtering And Aggregation

The feature needs controls for noise and privacy.

Recommended first-version defaults:

- Minimum recorded change: 10 MB
- Aggregation window: 5 minutes
- Long-term history detail: folder-level by default
- Short-term session detail: file-level when available
- Retention: keep detailed events for a shorter window, keep size samples longer

Supported settings:

- Minimum event size threshold, such as 1 MB, 10 MB, 100 MB, or custom.
- Aggregation window, such as 1 minute, 5 minutes, 15 minutes, or 1 hour.
- Watched locations list.
- Per-location enablement.
- Optional inclusion of file-level names in long-term logs.
- Retention policy for detailed logs and trend samples.

Aggregation rules:

- Many small changes under the same parent during one window should collapse into one folder event.
- Events below the threshold should still affect sampled folder size, but should not create noisy log rows.
- Delete events should show released size when Pathlight has a prior cached size. If not, display the deletion with previous size unknown.
- Rename and move events should be represented as moves when both sides can be correlated; otherwise record as delete plus create.

## Architecture

The implementation should add a live monitoring layer beside the existing scan system.

### Core Types

`DiskActivityMonitor`

- Starts and stops observation for selected roots.
- Emits raw path change notifications.
- Uses macOS file system observation APIs appropriate for path-level monitoring.
- Does not know about UI state.

`DiskActivityEvent`

- Represents normalized user-facing changes.
- Fields include timestamp, path, root path, event kind, estimated byte delta, confidence, and optional previous path for moves.
- Does not include app or process fields in the first version.

`StorageAttributionService`

- Converts raw notifications into size changes.
- Re-reads metadata for affected paths and relevant parent folders.
- Correlates bursts of notifications into meaningful create, modify, delete, move, and aggregate events.

`ActivityLogStore`

- Persists important events and size samples.
- Supports retention and pruning.
- Supports querying by root, time range, event type, and size threshold.

`LiveMonitorCoordinator`

- Coordinates monitor lifecycle for the app.
- Owns active Watch Sessions and long-term watcher state.
- Bridges live events into SwiftUI view models.
- Keeps this behavior separate from `ScanCoordinator`, which remains responsible for static scans and scan snapshots.

### Relationship To Existing Radix Components

`ScanEngine` should remain the source of truth for full scans.

`ScanSnapshot` should remain immutable. Live monitoring may mark paths as changed and trigger focused rescans or refreshed summaries, but should not mutate completed snapshots in place.

`AppModel` can own or depend on `LiveMonitorCoordinator`, similar to its existing relationship with `ScanCoordinator`.

The sunburst and file browser can show live change indicators later, but the first implementation should start with a dedicated Activity surface to avoid destabilizing core navigation.

## Data Flow

### Watch Session

1. User starts a Watch Session for a target.
2. `LiveMonitorCoordinator` creates a session and starts `DiskActivityMonitor`.
3. Raw path notifications flow into `StorageAttributionService`.
4. The service samples affected paths and parent folders.
5. Normalized events update the live timeline and session summary.
6. User stops the session.
7. Pathlight offers to save the session summary or discard it.

### Storage History

1. User enables long-term monitoring and chooses watched locations.
2. Pathlight registers the app as a login item or background helper only after explicit consent.
3. The monitor records aggregated events and periodic size samples.
4. The Activity view queries trend data for charts and important log rows.
5. Retention pruning runs opportunistically.

## UI Design

Add an Activity area to the app.

Primary views:

- Overview: watched locations, current size, recent growth, and last activity.
- Location Detail: trend chart, event summary, and filtered log.
- Watch Session: live timeline and net size change for the active session.
- Settings: watched locations, thresholds, aggregation, retention, and background monitoring.

Important UI principles:

- Do not present disk throughput as the primary experience.
- Lead with changed folders, size deltas, and time.
- Clearly distinguish estimated deltas from confirmed sizes.
- Show when monitoring is paused, unavailable, or missing permission.
- Make background monitoring opt-in and easy to disable.

## Privacy And Permissions

Pathlight should explain monitoring in plain language before enabling it.

First-version monitoring should work with paths the user can already access. For protected locations, the app may guide users toward Full Disk Access, consistent with Radix's existing permission model.

Long-term file-level names should not be enabled silently. Folder-level aggregation should be the default for long-term history.

The app should store activity logs locally. No network behavior is introduced by this feature.

## Error Handling

The app should handle these conditions:

- Watched location no longer exists.
- Permission is revoked.
- Event stream overflows or drops events.
- Size sampling fails for a path.
- External volume disconnects.
- Background monitor is disabled by the user or system.

If an event stream becomes unreliable, Pathlight should mark the affected time range as incomplete and offer a rescan.

## Testing Strategy

Unit tests:

- Event threshold filtering.
- Windowed aggregation.
- Delete size handling with known and unknown prior size.
- Move correlation when create/delete pairs are close in time.
- Retention pruning.
- Query filtering by root, time range, and size.

Integration tests:

- Monitor emits normalized events for temporary test directories.
- Session summaries compute net size changes.
- Storage history samples persist and reload.

UI tests can be added after the Activity surface stabilizes.

## Future Pro Capability

Pathlight Pro can later add app/process attribution through Endpoint Security.

That future version may record:

- Responsible process and app bundle.
- Process identifier.
- Code signing metadata.
- File event action and target path.

This is intentionally separate because it changes the product's permission model, onboarding, entitlement requirements, and review risk.

## First-Version Implementation Decisions

- Persistence should use a local SQLite-backed store accessed through a small Swift protocol. This avoids external package dependencies while still supporting time-range queries, pruning, and per-location filtering.
- Background monitoring should be opt-in and use a non-privileged login item flow. It should not require a privileged helper or Endpoint Security entitlement.
- Charts should use native SwiftUI charting in the app target.
- Live events should initially appear in a dedicated Activity surface. Sunburst and file browser live badges can follow after the event model is stable.
- Saved Watch Sessions should be stored locally as summaries. Exportable session artifacts are deferred.
