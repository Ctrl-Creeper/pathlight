# pathlight-core (Rust)

Platform-neutral core for Pathlight's folder-change monitoring, exposed to
Swift and Kotlin through [UniFFI](https://mozilla.github.io/uniffi-rs/).

## What is here

| Module | Purpose | Status |
|--------|---------|--------|
| `event` | `ActivityEvent` and a JSON encoding byte-compatible with the Swift `JSONLActivityEventStore` | done, fixture-tested against Swift output |
| `journal` | Append-only JSONL journal (0700 dir, 0600 file), newest-first load, `load_history`, skips Swift-encrypted rows | done |
| `monitor` + `fsevents` | `Watcher` emitting `StreamEvent`s to a host listener. macOS: native FSEvents with real event IDs and `since_event_id` resume. Others: `notify` (inotify / ReadDirectoryChangesW) with a counter | done; the macOS app uses it |
| `attribution` | Byte-delta attribution, threshold, window aggregation, in-memory `SizeIndex` | done (Rust API only, not yet on the FFI surface) |
| `exclusion` | gitignore-style noise filter via the `ignore` crate | done (Rust API only) |
| `history` | Buckets, totals, newest-first rows for the dashboard | done, exposed as `Journal.load_history` |

Not yet ported from Swift: retention policy, encrypted rows, size-index
persistence. The macOS app still runs attribution and exclusion in Swift on
top of the Rust event source.

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

- On Linux and Windows event IDs are a process-local counter, so `Watcher`
  cannot resume after a restart there; `since_event_id` triggers a
  `RequiresRescan`. A USN Journal backend would fix Windows.
- Paths are POSIX; `file://` conversion does not handle Windows drive letters.
- Timestamps are written at whole-second precision to match Swift.
