# Platform routes

Per-OS technical route for the watcher, and *why* each one is the route. The
capability declarations themselves live in `src/monitor.rs` and are enforced by
`tests/monitor.rs`; this file records the decisions and privilege trade-offs
behind them, which the code cannot show.

The rule: platforms are not made to behave alike. Each backend uses the
strongest API it can reach without changing how the app is distributed, and
declares what it therefore cannot do. Accuracy means a backend is never wrong
about its own limits.

## The privilege line

Every platform has a cheap unprivileged API and a strictly better privileged
one, and the privileged one always changes the shipping story (root install,
elevation prompt, or Apple review). So each platform gets a **default tier**
that ships as a normal app, and an optional **privileged tier** behind an
explicit user action.

| | default tier | privileged tier | what privilege buys |
|---|---|---|---|
| macOS | FSEvents | Endpoint Security | real PID |
| Linux | inotify | fanotify (`CAP_SYS_ADMIN`) | real PID, whole-volume, no watch cap |
| Windows | ReadDirectoryChangesW | USN Journal (admin) | cursor that survives reboot |
| Android | — | — | nothing worth the policy risk |

Only the default tiers are planned work. The privileged tiers are documented so
nobody re-discovers them as "missing features".

## macOS — done

FSEvents with `kFSEventStreamCreateFlagFileEvents` + `UseExtendedData`. Kernel
event IDs are a real resumable cursor, and the inode in the extended data lets
`fsevents.rs` pair rename halves itself. Wide latency + `NoDefer` keeps
background watches cheap.

No process attribution: only Endpoint Security names the process, and it needs
the `com.apple.developer.endpoint-security.client` entitlement (individual
Apple approval), a system extension, and Full Disk Access. That turns Pathlight
into a different kind of product. `reports_process: false` stays, and the `lsof`
hint stays labelled a hint.

## Linux — inotify, with the watch cap as a first-class failure

inotify pairs renames properly via the `IN_MOVED_FROM`/`IN_MOVED_TO` cookie, so
`pairs_renames` is achievable here — CI on Ubuntu is what proves it.

Two things must be handled rather than hoped away:

- **Watch cap.** One watch per directory, capped by
  `/proc/sys/fs/inotify/max_user_watches` (8192 on many distros). A large tree
  hits `ENOSPC` mid-arming, and a partial watch that reports itself as complete
  is the worst outcome available. It has to surface as a real error.
- **New-subtree race.** A directory created after arming needs its own watch
  added before anything inside it is visible; changes in that gap are lost.
  `tests/monitor.rs` already encodes the delay this causes.

No kernel cursor exists, so `resumable_cursor: false` and the host re-baselines.

The privileged tier is genuinely better — `FAN_MARK_FILESYSTEM` +
`FAN_REPORT_DFID_NAME` + `FAN_REPORT_PIDFD` covers a whole volume with one mark
and reports the responsible process — but unprivileged fanotify is explicitly
barred from all of it: no mount or filesystem marks, and the pid of another
process is never reported ([`fanotify_init(2)`]). It needs a `setcap` helper
daemon, i.e. a `sudo` install step. Optional, later, never the default.

## Windows — ReadDirectoryChangesW

Cheaper than inotify: one directory handle with `bWatchSubtree` covers an entire
tree, with no per-directory cost and no cap. Renames arrive as consecutive
`FILE_ACTION_RENAMED_OLD_NAME` / `_NEW_NAME`, so they can be paired. Buffer
overflow reports `ERROR_NOTIFY_ENUM_DIR`, which maps onto `RequiresRescan`. No
PID, no cursor.

USN Journal is the only thing on any platform that gives a cursor surviving a
reboot, but Microsoft's own docs require system administrator privileges for
every change-journal operation, and the volume handle needs
`FILE_FLAG_BACKUP_SEMANTICS`, i.e. `SeBackupPrivilege` from an elevated token
([`FSCTL_READ_USN_JOURNAL`]). On top of that, USN records identify files by
`FileReferenceNumber`, so turning them into paths means maintaining an
FRN→path index of the volume. Large subsystem, elevation required — an advanced
option at best.

`paths::normalize` already removes the `\\?\` prefix trap this backend would
otherwise walk into.

## Android — compatible, but not as a monitor

The Rust core itself needs no changes: `notify`'s inotify backend is already
`cfg(target_os = "android")`, and UniFFI emits Kotlin bindings. The blockers are
all platform policy, and all three are load-bearing:

1. **inotify does not see other apps' writes on `/sdcard`.** External storage is
   FUSE-served since Android 11, and fsnotify was never integrated with FUSE
   upstream. Your own writes may raise events; other processes' writes generally
   do not ([libfuse: Fsnotify and FUSE]). `FileObserver` on `/sdcard` for
   cross-app changes is unsupported in practice, not merely flaky.
2. **A 24/7 foreground service does not exist.** Targeting Android 15 (API 35),
   `dataSync` foreground services get 6 hours total per 24, then `onTimeout()`
   requires `stopSelf()` within seconds; restarting needs the user to foreground
   the app ([FGS timeouts]).
3. `MANAGE_EXTERNAL_STORAGE` is restricted by Play policy to file-manager-class
   apps, and granting it does not fix (1) anyway.

So Android does the parts it can actually do correctly:

- **Snapshot diff (primary).** On open, baseline-diff the authorized trees and
  report what changed since last time. Same code path as the desktop gap
  reconciliation, unaffected by FUSE, and cheap on battery precisely because
  nothing runs in the background.
- **MediaStore `ContentObserver`** with `notifyForDescendants = true` — the
  sanctioned mechanism, since MediaStore is where other apps' writes land.
  Coarse and index-delayed, so it reports low confidence.
- **Live watching of the app's own directories and SAF-granted trees**, where
  inotify is reliable, as a user-started session with a visible end.
- **Journal viewer** for records synced from a desktop.

This forces a new capability bit, `sees_other_processes_writes`, because that is
the real difference between Android and every other platform and the current
matrix cannot express it. A new platform demanding a new declaration — rather
than demanding a lie — is the matrix working.

## iOS — viewer only

Sandboxed to the app container plus Files-granted directories, with stricter
background execution than Android. Ships as a journal viewer and declares no
monitoring capability at all.

## Order of work

1. **"One operation is recorded once" as a portable test, then fix the
   `notify` rename triple.** `notify`'s inotify backend emits `RenameMode::From`,
   `RenameMode::To`, *and* a paired `RenameMode::Both` for a single rename, and
   `monitor.rs::forward` handles all three — so one rename becomes three rows on
   Linux. This is the duplicate-recording bug already fixed on macOS, waiting on
   another platform. The existing rename test cannot catch it: it asserts the
   rename is reconstructable, not that it is counted once.
2. Add `sees_other_processes_writes`; flip Linux `pairs_renames` to true and let
   CI prove it.
3. Move baseline diffing into the core. It is the only remedy for
   `resumable_cursor: false`, and Linux, Windows and Android all need it.
4. Windows ReadDirectoryChangesW backend.
5. Android: Kotlin bindings, snapshot diff, MediaStore observer.
6. Optional privileged tiers: Linux fanotify helper, Windows USN.

[`fanotify_init(2)`]: https://man7.org/linux/man-pages/man2/fanotify_init.2.html
[`FSCTL_READ_USN_JOURNAL`]: https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier
[libfuse: Fsnotify and FUSE]: https://github.com/libfuse/libfuse/wiki/Fsnotify-and-FUSE
[FGS timeouts]: https://developer.android.com/develop/background-work/services/fgs/timeout
