<p align="center">
    <img src="./icon.png" alt="Pathlight" width="220">
</p>

# Pathlight

A native macOS app that watches folders and tells you what changed on disk, when, and by how much. Pick a folder, and Pathlight records every create, modify, delete, and move inside it — live in a floating monitor, or long-term in the background with history, trends, and growth alerts.

This is a fork of [Ctrl-Creeper/pathlight](https://github.com/Ctrl-Creeper/pathlight) that keeps only the file-change monitoring feature. The disk space analyzer (scanning, sunburst chart, file browser, trash actions) has been removed.

![Platform](https://img.shields.io/badge/platform-macOS%2014%2B-blue)
![Swift](https://img.shields.io/badge/Swift-6.0-orange)
![License](https://img.shields.io/badge/license-MIT-lightgrey)

## Features

### Live Monitor

- **Watch any folder** and see changes stream in as they happen, in a pinnable floating window
- **Noise thresholds** — record every change, or only changes of 1 KB / 1 MB and larger
- **Size attribution** — each event carries the byte delta it caused, so you can see what actually grew or shrank

### Long-Term Monitoring

- **Persistent watches** across multiple folders, restored on every launch (optionally at login)
- **FSEvents cursor resume** — after a relaunch, Pathlight catches up on changes it missed and flags gaps it cannot recover
- **Activity Dashboard** with hourly trend charts, top changes, and a full timeline per folder
- **Growth alerts** — get a notification when a folder grows past a daily threshold
- **gitignore-style exclusions** per folder to drop caches, `.DS_Store`, temp files, and other churn
- **Menu bar summary** of today's changes while any watch is enabled
- **Keeps running** in the menu bar when you close the window or quit, so monitoring never silently stops

### Privacy & Storage

- Activity history and the size attribution index are stored locally as journals you control
- Configurable retention for detailed and aggregate history, a storage cap, and optional encryption of new data
- Full Disk Access is only needed for protected locations such as `~/Library/Mail`; Pathlight shows the current status in Settings

## Requirements

- **macOS Sonoma 14** or later
- **Xcode 26+** with Swift 6.0 toolchain (for building from source)

## Building from Source

```bash
git clone <this fork>
cd pathlight

# Build and run package tests
swift test

# Open in Xcode for the full app
open Pathlight.xcodeproj
```

The `Package.swift` file contains the **PathlightCore** library (monitoring engine, history, presentation models) and has no external package dependencies. The full SwiftUI app is built through the Xcode project, which integrates Sparkle through Xcode's Swift Package Manager support.

```bash
swift test
xcodebuild -project Pathlight.xcodeproj -scheme Pathlight -configuration Debug -destination 'platform=macOS' build
```

### Project Structure

```
Pathlight/
├── App/                  # App delegate (menu bar survival) and menu commands
├── Services/             # FSEvents monitor, attribution, journals, history, presentation
├── ViewModels/           # AppModel — central monitoring state
├── Features/
│   ├── Activity/         # Dashboard, Live Monitor window, menu bar extra
│   └── Settings/
└── Shared/               # Small presentation helpers
```

## Architecture Notes

- **FSEventsDiskActivityMonitor** wraps FSEvents and yields typed changes with event IDs so watches can resume from a checkpoint.
- **StorageAttributionService** turns raw path changes into events with byte deltas, using an on-disk size index to attribute deletions.
- **JSONLActivityEventStore** is an append-only journal; **ActivityHistoryService** folds it into buckets, top changes, and timelines.
- **AppModel** is the single `@MainActor` source of truth: it runs live and long-term watches, persists checkpoints, and enforces the storage policy.
- **PathlightCore** has no external Swift package dependencies; the Xcode app target adds Sparkle for automatic updates.

## License

MIT. See [LICENSE](LICENSE) for details.
