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

## Swift bindings

```sh
core/scripts/generate-swift-bindings.sh   # -> core/bindings/swift/
```

That emits `PathlightRustCore.swift`, a C header and a modulemap. Wiring it
into `Pathlight.xcodeproj` (XCFramework with `aarch64-apple-darwin` and
`x86_64-apple-darwin` static libs) is the next step and is not done yet.

## Known ceilings

- Event IDs are a process-local counter, so `Watcher` cannot resume after a
  restart. FSEvents IDs (macOS) and USN numbers (Windows) need dedicated
  backends; `notify` does not expose them.
- Paths are POSIX; `file://` conversion does not handle Windows drive letters.
- Timestamps are written at whole-second precision to match Swift.
