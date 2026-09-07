

# Pathlight

A native macOS app that watches folders and tells you what changed on disk, when, and by how much. Pick a folder, and Pathlight records every create, modify, delete, and move inside it — live in a floating monitor, or long-term in the background with history, trends, and growth alerts.

This is a fork of [Ctrl-Creeper/pathlight](https://github.com/Ctrl-Creeper/pathlight) that keeps only the file-change monitoring feature. The disk space analyzer (scanning, sunburst chart, file browser, trash actions) has been removed. The monitoring engine is being moved into a Rust core (`core/`) so the same journal, attribution, and history logic can back Linux and Windows builds later; the macOS app already sources its FSEvents stream from it.

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
- **Rust stable** (`rustup` recommended; add `x86_64-apple-darwin` for universal builds)

## Building from Source

```bash
git clone <this fork>
cd pathlight

# 1. Rust core: tests, then the xcframework the app links against
(cd core && cargo test)
core/scripts/build-xcframework.sh        # -> core/swift/PathlightRustCoreFFI.xcframework

# 2. Swift package tests (pure Swift, no Rust needed)
swift test

# 3. The app
xcodebuild -project Pathlight.xcodeproj -scheme Pathlight -configuration Debug -destination 'platform=macOS' build
# or: open Pathlight.xcodeproj
```

Step 1 must run once before the Xcode build; the project depends on the local Swift package in `core/swift`, and the xcframework is not checked in. The script produces a universal library when both Apple Rust targets are installed, otherwise a host-only one.

`Package.swift` contains the **PathlightCore** Swift library (session model, journal, history, presentation) with no external dependencies. The Rust crate in `core/` is documented in [`core/README.md`](core/README.md). CI (`.github/workflows/ci.yml`) builds the Rust core on Linux, macOS and Windows, runs the Swift package tests, and builds the app with a universal xcframework.

### Project Structure

```
core/                     # Rust crate `pathlight-core` (UniFFI)
├── src/
│   ├── fsevents.rs       #   macOS FSEvents backend with real event IDs
│   ├── monitor.rs        #   Watcher + notify backend for Linux/Windows
│   ├── attribution.rs    #   byte-delta attribution, aggregation, size index
│   ├── exclusion.rs      #   gitignore-style noise filter
│   ├── journal.rs        #   JSONL journal shared with the Swift app
│   └── history.rs        #   dashboard buckets and totals
├── swift/                # Local SwiftPM package wrapping the xcframework
└── scripts/              # build-xcframework.sh
Pathlight/                # macOS app (SwiftUI)
├── App/                  # App delegate, menu commands, Rust monitor adapter
├── Services/             # Session model, journal, history, presentation
├── ViewModels/           # AppModel — central monitoring state
├── Features/
│   ├── Activity/         # Dashboard, Live Monitor window, menu bar extra
│   └── Settings/
└── Shared/               # Small presentation helpers
PathlightCoreTests/       # Swift package tests
```

## Architecture Notes

- **Shared core, native shells.** Event sources are per platform because that is where low power comes from: FSEvents on macOS, inotify on Linux, `ReadDirectoryChangesW` on Windows, all kernel-driven with no polling. Everything above the event source (attribution, exclusion, journal, history) is platform-neutral Rust.
- **Rust `Watcher`** yields typed changes tagged with event IDs. On macOS the IDs are FSEvents' own, so long-term watches resume from a stored checkpoint after a relaunch; other platforms get a counter and ask the host to re-baseline.
- **Journal format is shared.** Rust writes the same JSONL rows as Swift's `JSONLActivityEventStore` (fixture-tested against `JSONEncoder` output), so any shell can read any other shell's history.
- **The macOS app today** uses the Rust event source via `RustDiskActivityMonitor` and still runs attribution and exclusion in Swift; moving those to the Rust side is the next step.
- **AppModel** is the single `@MainActor` source of truth: it runs live and long-term watches, persists checkpoints, and enforces the storage policy.
- **Mobile** is not a monitoring target: iOS and Android sandboxes cannot watch user folders in the background. If they get an app, it will be a viewer of a desktop journal.
- The Swift package has no external dependencies; the Xcode app target adds Sparkle for updates and the local `PathlightRustCore` package.

## License

MIT. See [LICENSE](LICENSE) for details.
