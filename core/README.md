# pathlight-core (Rust)

Platform-neutral core for Pathlight's folder-change monitoring, exposed to
Swift and Kotlin through [UniFFI](https://mozilla.github.io/uniffi-rs/).

## What is here

| Module | Purpose | Status |
|--------|---------|--------|
| `event` | `ActivityEvent` and a JSON encoding byte-compatible with the Swift `JSONLActivityEventStore` | done, fixture-tested against Swift output |
| `journal` | Append-only JSONL journal (0700 dir, 0600 file), newest-first load, skips Swift-encrypted rows | done |
| `monitor` | `Watcher` on top of `notify` (FSEvents / inotify / ReadDirectoryChangesW) emitting `StreamEvent`s to a callback listener | skeleton: live stream only |

Not yet ported from Swift: storage attribution (byte deltas), exclusion
patterns, history aggregation, retention policy, encrypted rows.

## Build and test

```sh
cd core
cargo test
```

## Swift bindings and the Xcode app

```sh
core/scripts/build-xcframework.sh          # release; pass "debug" for a debug build
```

The script builds the static library, regenerates
`core/swift/Sources/PathlightRustCore/PathlightRustCore.swift` (committed) and
packages `core/swift/PathlightRustCoreFFI.xcframework` (ignored by git).
`Pathlight.xcodeproj` depends on the local package in `core/swift`, so run the
script once before building the app. The library is universal only when the
`x86_64-apple-darwin` Rust target is installed; otherwise it is host-only.

## Known ceilings

- Event IDs are a process-local counter, so `Watcher` cannot resume after a
  restart. FSEvents IDs (macOS) and USN numbers (Windows) need dedicated
  backends; `notify` does not expose them.
- Paths are POSIX; `file://` conversion does not handle Windows drive letters.
- Timestamps are written at whole-second precision to match Swift.
