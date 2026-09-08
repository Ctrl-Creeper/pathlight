# Platform routes

Pathlight prioritizes efficient, accurate monitoring on each OS. Backends may
provide different guarantees; missing events or missing access must be visible
instead of silently producing a complete-looking history.

Default backends remain usable without installing a privileged helper. Optional
privileged backends are now part of the roadmap, enabled separately per platform
and per user choice. They complement the default sources where useful; they are
not inherently lossless replacements. The detailed design, primary sources,
permission boundaries and acceptance gates are in
[PRIVILEGED_BACKENDS.md](PRIVILEGED_BACKENDS.md).

## Current implementation

| Platform | Source in the repository | Current guarantees and limits |
|---|---|---|
| macOS | Native FSEvents | Kernel cursor and replay while history is retained; inode-based rename pairing; no authoritative process identity; gaps require reconciliation. |
| Linux | notify/inotify with Pathlight cookie pairing | Observed rename halves are paired; no persistent cursor or process identity; per-directory watches, registration races and overflow remain. |
| Windows | notify/ReadDirectoryChangesW | Recursive event delivery without polling; separate rename halves; no persistent cursor or process identity. |
| Android | Rust inotify route compiles for Android | No Android host yet. Compiling the core does not prove storage access, background lifetime or cross-app event coverage on a device. |
| iOS | No monitoring host | Plan a desktop-history viewer and explicitly scoped foreground features; no public API for an arbitrary system-wide privileged monitor. |

`monitor::Capabilities` describes the currently selected implementation.
`pairs_renames` means ordinary observed halves can be paired within its matching
window, not that cross-root moves, queue gaps or expired halves have both paths.
Future helpers need per-watch runtime capabilities, including filesystem,
authorized scope and source health; an OS-wide constant cannot express those.

## Linux rename correction

The pinned notify 8.2.0 inotify source emits `From`, `To`, and then a redundant
`Both` for one kernel rename. Forwarding all three used to produce three rows.
`src/inotify.rs` now pairs the halves by cookie and ignores the redundant `Both`
only on the inotify route. It handles interleaved cookies itself, since notify
only remembers one pending departure.

Unmatched halves wait at most 250 ms after processing before being forwarded,
so a move into or out of the root is preserved even when the filesystem goes
idle. Late halves remain separate; they do not acquire a third row. There is
no timer while the pairing buffer is empty. Memory is bounded: 1,024 pending
halves and a 4,096-event callback queue. Exhaustion emits `RequiresRescan` and
clears pairing state at the gap. Accepted events are drained on stop, and event
IDs are assigned in output order. This is event normalization, not periodic
filesystem polling; power consumption has not yet been benchmarked.

`tests/monitor.rs` uses real filesystem operations to check single-row file and
directory renames, successive renames, root-boundary moves, shutdown and event
ID order. The original single-rename regression failed with three rows on Linux
before the fix. Unit tests cover interleaved cookies, late halves, gaps and
pairing-buffer exhaustion. The existing Linux/macOS/Windows CI matrix runs these
contracts; a cross-compilation check alone is not a Windows or Android runtime
qualification.

Local verification counts are recorded in the evidence-foundation plan after
each complete verification pass. Linux tests run on a real Linux filesystem in
a container. Windows x86_64 GNU and Android aarch64 checks prove compilation
only; Windows/Android device execution, power measurements and privileged
helpers remain unverified.

## Losing the watch root

A watch whose root is deleted, renamed or unmounted is the one failure a host
cannot detect on its own: the stream stays open and silent, so a dead watch
looks exactly like a quiet folder. Every backend has to surface it as
`RequiresRescan`, and `tests/monitor.rs` holds all of them to it by deleting a
live root.

| Platform | Kernel signal | Route |
|---|---|---|
| macOS | `kFSEventStreamEventFlagRootChanged` | Only delivered when the stream is created with `kFSEventStreamCreateFlagWatchRoot`, which `src/fsevents.rs` now sets. Without that flag the root can vanish and FSEvents reports nothing at all. |
| Linux | `IN_DELETE_SELF`, `IN_MOVE_SELF` | notify watches the top level with `watch_self`, then the kernel drops the watch right after delivering. `src/notify_backend.rs` recognizes an event naming the root itself and reports the gap instead of filing one ordinary change inside a watch that is already dead. |
| Windows | none | `ReadDirectoryChangesW` holds an open handle to the watched directory, so the directory cannot be deleted while the watch runs, and a rename keeps the handle valid on the moved directory. There is no equivalent signal to normalize and none is fabricated; the loss modes are unmount and revoked access, which surface as a read failure. |

The `RequiresRescan` contract stops at reporting. Deciding whether the root is
gone for good or merely renamed is the host's job: the Swift app re-probes the
path and reports `rootMissing` or reconnects.

## Platform-specific next steps

1. Replace platform-wide capability constants with a negotiated per-watch
   coverage descriptor before combining default and privileged sources. Do not
   append two sources independently and recreate the duplicate-recording bug.
2. Linux: prototype an optional fanotify helper on explicitly tested kernels and
   filesystems. Probe supported flags, handle overflow and mount changes, and
   fall back visibly to inotify. Whole-filesystem marks remove per-directory
   watch registration, not every limit or coverage gap.
3. Windows: add an optional USN reader for persistent recovery on supported local
   volumes, with journal identity/cursor validation and a file-reference/path
   index. Keep ReadDirectoryChangesW for ordinary live watches. USN does not
   identify the writing process; evaluate ETW separately if needed.
4. macOS: pursue the Endpoint Security entitlement and prototype notification
   events for kernel process attribution. Keep FSEvents replay and reconciliation
   available; detect ES sequence gaps and avoid synchronous authorization events
   for this read-only product.
5. Android: build the Kotlin host and qualify app-private paths, authorized shared
   storage and provider-backed SAF trees separately. SAF content URIs are not
   generally inotify paths. Use MediaStore/provider invalidations and snapshot
   reconciliation where appropriate. A rooted-device helper is an experimental,
   separately qualified route, not a Play-distributed guarantee.

## Accuracy gates before claiming support

- Run the watcher contract on the actual OS/filesystem and test overflow,
  interrupted access, restart, rename chains and cross-root moves.
- Report partial watch registration and unsupported filesystems as partial or
  failed coverage, including inotify watch-limit errors and new-subtree races.
- Display observed delivery time separately from any source-provided event time.
  Do not claim precise historical timing from a delayed snapshot.
- Measure idle CPU/wakeups, backlog growth, write amplification and recovery
  cost. Tune batching on evidence; do not add periodic full-tree scans as a
  silent reliability fallback.

See [PRIVILEGED_BACKENDS.md](PRIVILEGED_BACKENDS.md) for the primary documentation
and platform-specific acceptance criteria.
