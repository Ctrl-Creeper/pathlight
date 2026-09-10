

# Pathlight

Watches folders and tells you what changed on disk, when, and by how much. A native macOS app, and a window for Windows and Linux. Pick a folder, and Pathlight records every create, modify, delete, and move inside it — live in a floating monitor, or long-term in the background with history, trends, and growth alerts.

The monitoring engine lives in a Rust core (`core/`). Two shells sit on top of it: the macOS app, which takes its FSEvents stream and its byte attribution from the core and keeps exclusion, journal and history in Swift; and `gui/`, a window for Windows and Linux that links the core directly and uses all of it. They also share a journal format, held byte-for-byte by `core/tests/swift_compat.rs`, so either shell reads the other's history.

![Platform](https://img.shields.io/badge/platform-macOS%2014%2B%20%7C%20Windows%20%7C%20Linux-blue)
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

### Windows and Linux

- **A window of its own** (`gui/`, one `pathlight` executable) — pick folders, start and stop watches, and read what changed with the same byte-delta attribution
- **The same journal** the macOS app writes, in the same place an uninstall looks for it
- **Interface tests through accesskit**, so the window is exercised on each OS in CI rather than only where it was written

### Privacy & Storage

- Activity history and the size attribution index are stored locally as journals you control
- Configurable retention for detailed and aggregate history, a storage cap, and optional encryption of new data
- Full Disk Access is only needed for protected locations such as `~/Library/Mail`; Pathlight shows the current status in Settings

## Requirements

The macOS app:

- **macOS Sonoma 14** or later
- **Xcode 26+** with Swift 6.0 toolchain (for building from source)
- **Rust stable** (`rustup` recommended; add `x86_64-apple-darwin` for universal builds)

The Windows and Linux window:

- **Rust stable**, and nothing else. No Node, no WebView2, no GTK development headers
- On Linux, the usual windowing libraries: `libxkbcommon`, `libwayland`, `libGL`

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

The Windows and Linux window is a separate crate and needs none of the above:

```bash
cd gui
cargo test          # includes the interface tests
cargo run --release
```

Step 1 must run once before the Xcode build; the project depends on the local Swift package in `core/swift`, and the xcframework is not checked in. The script produces a universal library when both Apple Rust targets are installed, otherwise a host-only one.

`Package.swift` contains the **PathlightCore** Swift library (session model, journal, history, presentation) with no external dependencies. The Rust crate in `core/` is documented in [`core/README.md`](core/README.md). CI (`.github/workflows/ci.yml`) builds the Rust core on Linux, macOS and Windows, runs the Swift package tests, builds the desktop shell on all three, and builds the app with a universal xcframework. Tagging `v*` runs `.github/workflows/release.yml`, which builds each artifact on its own OS and publishes a pre-release.

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
gui/                      # Windows + Linux window (egui), binary `pathlight`
├── src/main.rs           #   the interface, and its accesskit-driven tests
├── src/session.rs        #   watcher -> attribution -> journal, off the paint path
└── src/store.rs          #   where it records, and the rule that keeps it out of itself
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
- **Two hosts, one core, different depths.** `gui/` links `pathlight-core` directly with no FFI layer and uses the whole crate — watcher, attribution, journal. The macOS app reaches the watcher and attribution over UniFFI and keeps journal, history and exclusion in Swift; its size index stays Swift-side because that one is persistent and encrypted, so only the arithmetic crosses.
- **Pathlight's own storage is excluded from every watch**, in both hosts, before attribution rather than as a setting: recording a journal write produces an event that measures the journal, which writes to it again. No byte threshold stops that, because attribution measures a path before any threshold applies.
- **Moving the macOS app's exclusion matching to the Rust side** is the next step. Attribution moved already: "how many bytes changed" has one answer for every platform, which is the answer a journal both hosts read cannot hold two of.
- **AppModel** is the single `@MainActor` source of truth: it runs live and long-term watches, persists checkpoints, and enforces the storage policy.
- **Mobile** is not a monitoring target: iOS and Android sandboxes cannot watch user folders in the background. If they get an app, it will be a viewer of a desktop journal.
- The Swift package has no external dependencies; the Xcode app target adds Sparkle for updates and the local `PathlightRustCore` package.

## License

MIT. See [LICENSE](LICENSE) for details.
