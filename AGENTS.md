# AGENTS.md

This file tells coding agents how to work effectively in this repository.

## Purpose

Pathlight is a native macOS folder-change monitor built in Swift and SwiftUI. This fork keeps only file-change monitoring (live watches, long-term watches, activity history); the original disk analyzer was removed. When developing Pathlight, prioritize Swift/SwiftUI best practices and modern code.

## Commit Guidelines

- Make small, focused commits
- Each commit should represent a single logical change
- Avoid mixing refactors with behavior changes
- Use Conventional Commits:
  - `fix: correct size totals for nested directories`
  - `feat: remember last opened scan location`
  - `perf: cache formatted file sizes in file list rows`
  - `refactor: remove unused scan coordination code`

## Environment Facts

- Repository root: `Pathlight/`
- Swift 6
- macOS target: macOS 14.0+
- App UI framework: SwiftUI (always preferred over UIKit/AppKit)
- Tests: `PathlightCoreTests/`
- Rust core: `core/` (cargo crate `pathlight-core`, UniFFI bindings; run `cargo test` inside `core/`). The macOS app links it for file watching (`Pathlight/App/RustDiskActivityMonitor.swift`); `cargo test` inside `core/` runs it, and `core/tests/monitor.rs` is the cross-platform watcher contract. Each backend declares what it guarantees via `monitor::Capabilities` rather than every platform being forced to behave alike.

## Project Structure

```
Pathlight/
├── App/                  # App delegate (menu bar survival), menu commands
├── Services/             # FSEvents monitor, attribution, journals, history, presentation
├── ViewModels/           # AppModel — central monitoring state
├── Features/
│   ├── Activity/         # Dashboard, Live Monitor window, menu bar extra
│   └── Settings/
└── Shared/               # Small presentation helpers
```

## Project Layout

Important paths:

- `README.md`: product intent, feature summary, high-level architecture
- `Package.swift`: exact package target membership
- `Pathlight/PathlightApp.swift`: app entry
- `Pathlight/ContentView.swift`: root content composition
- `Pathlight/ViewModels/AppModel.swift`: central `@MainActor` monitoring state (live/long-term watches, history, storage policy)
- `Pathlight/Services/DiskActivityMonitor.swift`: FSEvents wrapper yielding typed changes with event IDs
- `Pathlight/Services/StorageAttributionService.swift`: byte-delta attribution for changes
- `Pathlight/Services/ActivityEventStore.swift` / `ActivityHistoryService.swift`: journal and history aggregation
- `Pathlight/Services/SystemIntegration.swift`: Finder reveal, folder picker, Full Disk Access probes
- `Pathlight/Shared/`: shared UI helpers
- `PathlightCoreTests/`: package-level unit and benchmark-style tests
- `releases/`: release/update assets
- Sparkle is managed through Xcode Swift Package Manager; do not add vendored Sparkle folders or frameworks to the repo.

## Product Constraints

Pathlight makes several user-facing promises. Do not casually violate them:

- Monitoring must stay cheap: wide FSEvents latency for background watches, no polling.
- The app never mutates user files; it only reads and records.
- Monitoring keeps running when the window closes or the user quits into the menu bar.

## Working Agreement For Changes

When making changes:

- Keep edits consistent with the existing architecture unless existing architecture is problematic.
- Prefer fixing behavior in the core model/service layer when the bug is data-related.
- Prefer fixing behavior in `AppModel` when the issue is coordination, selection, focus, navigation, or settings persistence.
- Prefer adding or updating tests when changing monitor behavior, attribution, journal format, history aggregation, exclusion matching, or formatting logic.
- Avoid introducing new dependencies unless explicitly justified. The project is intentionally light on external packages.

Use Context7 when working with external libraries, frameworks, or APIs and you need current, version-aware documentation.

Prefer Context7 for:

- SwiftUI or Apple framework usage where exact modern APIs matter
- Cases where examples from memory may be outdated

Do not use Context7 for:

- Understanding this repository’s internal architecture
- Answering questions already resolved by local code, tests, or README
- Simple edits that can be completed by following existing patterns in the codebase

## If You Need A Starting Point

- Missing or wrong events: `Pathlight/Services/DiskActivityMonitor.swift`, then `StorageAttributionService.swift`
- Watch lifecycle, checkpoints, reconnect: `Pathlight/ViewModels/AppModel.swift`
- Journal, retention, encryption: `Pathlight/Services/ActivityEventStore.swift`, `ActivityStorageLineCodec.swift`
- Dashboard numbers: `Pathlight/Services/ActivityHistoryService.swift`, `ActivityDashboardPresentation.swift`
- Exclusion patterns: `Pathlight/Services/ScanExclusionMatcher.swift`, `ActivityExclusionFilter.swift`
- Size or display formatting bug: `Pathlight/Services/FileSizeFormatter.swift`
