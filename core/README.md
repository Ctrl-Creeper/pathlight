# pathlight-core (Rust)

Platform-neutral core for Pathlight's folder-change monitoring, exposed to
Swift and Kotlin through [UniFFI](https://mozilla.github.io/uniffi-rs/).

## What is here

| Module | Purpose | Status |
|--------|---------|--------|
| `event` | `ActivityEvent` and a JSON encoding byte-compatible with the Swift `JSONLActivityEventStore` | done, fixture-tested against Swift output |
| `journal` | Append-only JSONL journal (0700 dir, 0600 file), shared encryption, retention with daily rollups, newest-first load and `load_history` | done |
| `monitor` + platform backends | `Watcher` emitting `StreamEvent`s. macOS uses resumable FSEvents; Linux/Android pair inotify renames; Windows uses ReadDirectoryChangesW through `notify` | default backends implemented; Windows/Android device qualification remains |
| `measurement` | Logical length, filesystem-reported allocation, link count, kind and scoped native object identity from one metadata observation | implemented and real-filesystem tested on macOS/Linux; Windows compile-tested |
| `snapshot` | Native-name interval traversal, identity-aware totals and continuity-gated endpoint reconciliation | implemented; explicitly not an atomic snapshot or historical replay |
| `evidence` | Locked, versioned JSONL evidence with source epochs, native paths, explicit gaps and bounded durable batches | implemented (Rust API) |
| `recording` | Bounded recording sessions with watcher-first startup, chunked native binding manifests and interval snapshots | implemented as `pathlight-monitor`; no restart resume yet |
| `attribution` | Byte-delta attribution, threshold, window aggregation, `SizeIndex` (persisted beside the journal, sealed with it) | done; attribution is exposed over UniFFI and persistence is owned by each host |
| `exclusion` | gitignore-style noise filter via the `ignore` crate | done (Rust API only) |
| `history` | Buckets, totals, newest-first rows for the dashboard | done, exposed as `Journal.load_history` |

The app, window, and terminal share the encrypted journal line format and the
protected `activity-events.key`. Retention and daily rollups have matching
semantics in the Rust and Swift stores. The macOS host keeps journal I/O and
its size index in Swift, while attribution and exclusion rules come from the
Rust core over UniFFI.

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
Use an official rustup toolchain for release artifacts. The script inspects all
Mach-O members and rejects a Rust standard library whose minimum macOS version
is newer than `MACOSX_DEPLOYMENT_TARGET` (14.0 by default); setting the
environment variable cannot rewrite precompiled standard-library objects.

## Known ceilings

- On Linux and Windows event IDs are a process-local counter, so `Watcher`
  cannot resume after a restart there; `since_event_id` triggers a
  `RequiresRescan`. A USN Journal backend would fix Windows.
- Legacy `StreamEvent` paths still cross UniFFI as UTF-8 strings. Evidence
  manifests preserve native Unix bytes/Windows UTF-16; an unrepresentable live
  event becomes an explicit gap and is recovered only as observable snapshot state.
- Windows verbatim path semantics and real device behavior still require native
  runtime tests before the backend can claim full path coverage.
- Timestamps are written at whole-second precision to match Swift.
