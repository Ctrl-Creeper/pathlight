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
| Linux | notify/inotify with Pathlight cookie pairing | Observed rename halves are paired; no persistent cursor or process identity; per-directory watches, registration races and overflow remain. Hitting `fs.inotify.max_user_watches` is reported by name and current value rather than as "No space left on device". |
| Linux, privileged | `src/fanotify.rs`, opt-in with `PATHLIGHT_PRIVILEGED_WATCH=1` and `CAP_SYS_ADMIN` | One `FAN_MARK_FILESYSTEM` mark instead of per-directory watches, so no watch limit and no registration race, and each change names the writing process. Still no persistent cursor, and queue overflow is reported as a gap. Renames are paired by the kernel where `FAN_RENAME` exists (Linux 5.17); on older kernels the two halves are reported and `pairs_renames` says so. Events arrive for the whole filesystem and are filtered to the root. |
| Windows | notify/ReadDirectoryChangesW | Recursive event delivery without polling; separate rename halves; no persistent cursor or process identity. |
| Windows, privileged | `src/usn.rs`, opt-in with `PATHLIGHT_PRIVILEGED_WATCH=1` and an administrator token | The volume's own NTFS change journal: one handle covers the whole volume with no per-directory buffer to overrun, and the USN it numbers records with is monotonic and survives a reboot, so a stored cursor really resumes — the only Windows source where a restart can replay what it missed. A cursor the journal no longer covers is reported as a gap rather than replayed from the middle. Renames are paired by file reference number. USN carries no pid, so `reports_process` stays false. Records arrive for the whole volume and are filtered to the root; a root on a drive with no journal (exFAT, a network share) falls back to `ReadDirectoryChangesW`. |
| Android | Rust inotify route compiles for Android | No Android host yet. Compiling the core does not prove storage access, background lifetime or cross-app event coverage on a device. |
| iOS | No monitoring host | Plan a desktop-history viewer and explicitly scoped foreground features; no public API for an arbitrary system-wide privileged monitor. |

## Hosts

A backend is only half of a platform; something has to show it to a user.

| Platform | Host | Source in the repository |
|---|---|---|
| macOS | SwiftUI app, menu bar extra, full history and dashboard | `Pathlight/` |
| Windows, Linux | One window: folders, live watches, attributed changes | `gui/` (egui, links this crate directly) |
| any | `pathlight-monitor`, one recorded session to a JSONL journal | `src/bin/pathlight-monitor.rs` |

Both windowed hosts write the same journal format to the directory
`uninstall::data_dir` names, so either one reads the other's history and one
uninstall finds both. Neither ever records inside that directory: the write
produces an event, attribution measures the file, and measuring it writes
again — a loop no byte threshold stops, because attribution runs before any
threshold applies. `gui/src/store.rs` and the macOS `ActivityStorageIsolation`
each hold that rule with the spelling their watcher reports, symlinks resolved.

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
`RequiresRescan`, and `tests/monitor.rs` holds every backend that has a signal
to it by deleting a live root — which is every backend except Windows, for the
reason in the table below.

| Platform | Kernel signal | Route |
|---|---|---|
| macOS | `kFSEventStreamEventFlagRootChanged` | Only delivered when the stream is created with `kFSEventStreamCreateFlagWatchRoot`, which `src/fsevents.rs` now sets. Without that flag the root can vanish and FSEvents reports nothing at all. |
| Linux | `IN_DELETE_SELF`, `IN_MOVE_SELF` | notify watches the top level with `watch_self`, then the kernel drops the watch right after delivering. `src/notify_backend.rs` recognizes an event naming the root itself and reports the gap instead of filing one ordinary change inside a watch that is already dead. |
| Windows | none | `ReadDirectoryChangesW` holds an open handle to the watched directory, so the directory cannot be deleted while the watch runs, and a rename keeps the handle valid on the moved directory. There is no equivalent signal to normalize and none is fabricated; the loss modes are unmount and revoked access, which surface as a read failure. |

The `RequiresRescan` contract stops at reporting. Deciding whether the root is
gone for good or merely renamed is the host's job: the Swift app re-probes the
path and reports `rootMissing` or reconnects.

## Uninstall

A monitor has to be removable without a hunt through the system's data
directories, and removal is the one operation where a wrong path deletes
somebody's data. `src/uninstall.rs` owns the per-OS layout. Every entry is
derived from the platform's data roots and a fixed application name — never
from a watch target or a journal path a user typed — and
`tests/uninstall.rs` holds all three layouts to that on every host, because
they are plain functions rather than `cfg`-gated bodies.

| Platform | Storage removed | Notes |
|---|---|---|
| macOS | `~/Library/Application Support/Pathlight` | The Swift host owns the full list: preferences, caches, saved state, the Keychain key, the login item and the app bundle. `Pathlight/Services/UninstallService.swift` is the authority; this module deliberately does not restate it, because two lists that have to agree eventually do not. |
| Linux | `$XDG_DATA_HOME`, `$XDG_STATE_HOME`, `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME` (each `/pathlight`) | Unset roots fall back to the specification's defaults under `$HOME`. A root holding a relative path is ignored, per the specification — honouring one would delete relative to whatever the working directory happened to be. |
| Windows | `%APPDATA%\Pathlight`, `%LOCALAPPDATA%\Pathlight` | Both fall back to their standard place under `%USERPROFILE%` when unset. |

`pathlight-monitor uninstall` lists what exists and removes nothing;
`--yes` removes it and reports every path that survived. A destructive command
that needs no confirmation is a command someone runs by accident. The list
includes a command installed by `install-cli`, which is not storage — but a
binary left on somebody's PATH is exactly the litter an uninstall is for.

`install-cli` is how the command reaches a shell at all: no platform's install
touches PATH, since one is a dragged bundle and the others are unpacked
archives. It links the running binary into the user's own home —
`~/.local/bin`, or `%LOCALAPPDATA%\Programs\Pathlight` on Windows, which
copies because a symlink there needs privilege — and prints how to add that
directory to PATH when it is not there. Never `/usr/local/bin`: it needs an
administrator on a clean macOS. `tests/cli.rs` drives the real binary for each
of these, with the home directory and every data root redirected at a temporary
directory, because uninstall derives what it deletes from them.

Two things no uninstall can reach. macOS keeps its own record of a granted Full
Disk Access, and the TCC database is protected from every app including the one
it names, so the user has to remove that entry themselves; the Swift host says
so on the uninstall pane. Journals a caller named explicitly — the second
argument to `pathlight-monitor` — are never guessed at, so a host that stores
its journal outside the standard layout has to remove that path itself.

## Platform-specific next steps

1. Replace platform-wide capability constants with a negotiated per-watch
   coverage descriptor before combining default and privileged sources. Do not
   append two sources independently and recreate the duplicate-recording bug.
2. Linux: fanotify is implemented in-process (`src/fanotify.rs`) and probed, not
   version-checked; CI runs the watcher contract twice, once unprivileged and
   once under `sudo`. What is still missing before it can be the default for a
   root: the power and accuracy measurements below, mount-change handling, and a
   helper with its own privilege boundary instead of the whole app running as
   root. Whole-filesystem marks remove per-directory watch registration, not
   every limit or coverage gap.
3. Windows: the USN reader is implemented in-process (`src/usn.rs`) with journal
   identity and cursor validation, and CI runs the watcher contract twice, the
   second pass on an elevated runner. What is still missing before it can be the
   default for a root: the measurements below, a path index instead of resolving
   each parent reference on demand, volume-arrival/removal handling, and a
   helper holding the administrator boundary instead of the whole app. USN does
   not identify the writing process; evaluate ETW separately if needed.
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
