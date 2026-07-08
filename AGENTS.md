# AGENTS.md

This file tells coding agents how to work effectively in this repository.

## Purpose

Pathlight is a native macOS disk space analyzer built in Swift and SwiftUI. When developing Pathlight, prioritize Swift/SwiftUI best practices and modern code.

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

## Project Structure

```
Pathlight/
├── App/                  # App entry point, commands, window management
├── Models/               # Core data types (FileNodeRecord, ScanSnapshot, etc.)
├── Services/             # Scan engine, sunburst geometry, formatters
├── ViewModels/           # AppModel — central state manager
├── Features/             # UI features (workspace, sidebar, file browser,
│   ├── Workspace/        #   visualization, inspector, settings, onboarding)
│   ├── Sidebar/
│   ├── FileList/
│   ├── Visualization/
│   ├── Inspector/
│   ├── Settings/
│   └── Onboarding/
├── Views/                # Reserved for future shared view composition
└── Shared/               # Reusable components (breadcrumbs, helpers)
```

## Project Layout

Important paths:

- `README.md`: product intent, feature summary, high-level architecture
- `Package.swift`: exact package target membership
- `Pathlight/PathlightApp.swift`: app entry
- `Pathlight/ContentView.swift`: root content composition
- `Pathlight/ViewModels/AppModel.swift`: central `@MainActor` app state and UI coordination
- `Pathlight/Models/`: core scan targets, node records, tree storage, snapshots, progress, file actions, and trash safety
- `Pathlight/Services/ScanEngine.swift`: actor-based filesystem scanner
- `Pathlight/Services/SunburstGeometry.swift`: sunburst layout math
- `Pathlight/Services/SystemIntegration.swift`: Finder/open/trash/system-facing actions
- `Pathlight/Shared/`: shared UI helpers
- `PathlightCoreTests/`: package-level unit and benchmark-style tests
- `releases/`: release/update assets
- Sparkle is managed through Xcode Swift Package Manager; do not add vendored Sparkle folders or frameworks to the repo.

## Product Constraints

Pathlight makes several user-facing promises. Do not casually violate them:

- Scans should feel fast and responsive.
- The app should not mutate files unless the user explicitly requests an action.
- The sunburst and file browser are primary navigation surfaces, not secondary embellishments.

## Working Agreement For Changes

When making changes:

- Keep edits consistent with the existing architecture unless existing architecture is problematic.
- Prefer fixing behavior in the core model/service layer when the bug is data-related.
- Prefer fixing behavior in `AppModel` when the issue is coordination, selection, focus, navigation, or settings persistence.
- Prefer adding or updating tests when changing scanner behavior, path normalization, indexing, geometry, or formatting logic.
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

- Scanner bug or data bug: start with `Pathlight/Services/ScanEngine.swift` and the matching tests in `PathlightCoreTests/`
- Selection/navigation/UI state bug: start with `Pathlight/ViewModels/AppModel.swift`
- Tree/index behavior bug: start with `Pathlight/Models/FileTreeStore.swift`
- Search behavior bug: start with `Pathlight/Services/FileBrowserModel.swift`
- Size or display formatting bug: start with `Pathlight/Services/FileSizeFormatter.swift`
- Visualization/layout bug: start with `Pathlight/Services/SunburstGeometry.swift`
