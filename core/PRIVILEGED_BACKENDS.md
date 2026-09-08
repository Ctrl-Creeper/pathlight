# Optional privileged monitoring backends

Research date: 2026-09-08. This is a design and an implementation sequence, not a
claim that these backends have shipped or passed device tests. The existing core
uses FSEvents on macOS and `notify` on other supported desktop targets. The
privileged collectors below are proposed additions. API facts were checked
against the primary sources linked here. The proposed privileged collectors
have not been exercised; default-backend verification is recorded in
[PLATFORMS.md](PLATFORMS.md).

The product goal is accurate, efficient observation within a declared scope.
Privilege can improve event identity or coverage, but does not make every
filesystem operation observable or reconstruct the exact history of a gap.
A baseline diff establishes observable state differences between snapshots; it
cannot recover a file created and deleted entirely between those snapshots.
Allocated-size deltas also do not measure total physical bytes written: an
in-place overwrite may write gigabytes with no size change. Keep these meanings
separate in the product.

## Route by platform

| Platform | Default | Optional privileged route | Main benefit | Remaining limit |
| --- | --- | --- | --- | --- |
| macOS | FSEvents | Endpoint Security `NOTIFY` client; packaged system extension for distribution | Event-specific process identity and operation metadata | No ES replay; dropped messages possible; no exact byte-write accounting |
| Linux | inotify through `notify` | Small fanotify helper with filesystem marks on supported local filesystems | Kernel process identity and coverage without one mark per directory | No persistent replay; filesystem/kernel constraints; documented mmap and remote-event blind spots |
| Windows | ReadDirectoryChangesW through `notify` | Elevated service reading an existing USN journal; ETW separately for live attribution | Recover retained changes across disconnects/restarts | Journal trimming/reset; USN has no PID and is not a complete syscall history |
| Android | Active sessions on accessible real paths; provider-aware snapshot comparisons | Experimental root helper, or OEM-managed integration, negotiated per device | More accessible local paths and potentially kernel process evidence | Root does not remove SELinux, filesystem, namespace, or background-lifetime constraints |
| iOS/iPadOS | Journal viewer; optional comparisons of granted documents | No supported public equivalent of the desktop privileged collectors | — | No general whole-device filesystem access or persistent monitor |

These routes supplement the product, not replace its default mode. The earlier
claims that all Android cross-app observation is impossible, that all foreground
services have a six-hour limit, and that privilege always provides a strictly
better watcher are too broad. The narrower supported facts are below.

## macOS: Endpoint Security alongside FSEvents

**Documented behavior.** Endpoint Security reports operations and a process
structure containing an audit token, executable, and code-signing information.
`es_message_t.process` describes the process taking the action; extract PID/UID
from the audit token and retain the token or its process-instance identity so PID
reuse cannot attach a later process to an old event. This is stronger evidence
than an `lsof` snapshot. It still does not justify claiming that a process wrote
a particular number of bytes. [Apple process data][es-process]

Use notification events for creates, writes, closes, renames, links, unlinks,
truncations, clones, and relevant metadata changes, according to the events
available on the running OS. A close event's `modified` field is useful, but
waiting only for close would hide ongoing writes to a long-lived file handle.
Subscribe to the necessary live write notifications for live sessions and
coalesce measurements. Do not subscribe to authorization events: Pathlight
observes completed actions and does not permit, deny, or delay user operations.
[ES messages][es-message] [Endpoint Security overview][es]

**Deployment requirements are distinct.**

- The client needs Apple's approved
  `com.apple.developer.endpoint-security.client` entitlement. Root alone cannot
  substitute for it. [Entitlement][es-entitlement]
- `es_new_client` also requires user TCC approval through Full Disk Access.
  Handle a missing entitlement, missing consent, and a stopped collector as
  distinct availability failures. [Client creation][es-new]
- The collector must run as root; Apple's SDK documents
  `ES_NEW_CLIENT_RESULT_ERR_NOT_PRIVILEGED` as “The caller is not running as
  root.” This was checked in the local macOS SDK `EndpointSecurity/ESTypes.h`.
- A system extension is a distribution/lifecycle choice, not a prerequisite for
  every ES API client. Apple explicitly says standalone ES products are
  possible, while recommending system extensions for protections and early
  startup. Use an entitled standalone collector for a development experiment;
  design the distributable version as a signed, notarized system extension with
  the corresponding activation/user-consent flow. Do not disable SIP to ship
  it. [Apple's deployment explanation][es-wwdc]

**Continuity and power.** ES sequence numbers detect dropped messages: `seq_num`
tracks each event type, and `global_seq_num` tracks messages for the client.
Read fields only when the message version supports them. They are delivery
sequence numbers, not FSEvents cursors; the public subscription API does not
provide replay from a stored ES sequence. On a gap/restart, recover retained
filesystem changes through FSEvents and reconcile state, while marking missing
process evidence as unavailable. Never assign a current process to a replayed
old change. [Sequence number][es-seq] [Global sequence number][es-global]

Keep FSEvents for inexpensive background continuity. Its historical stream is
explicitly advisory and has dropped-event/rescan conditions; resumability is
not a promise of an exhaustive operation log. [FSEvents guide][fsevents]
Filter ES subscriptions and target paths at the collector using supported
muting APIs. Use bounded message handling and measure cost under compiler and
sync workloads; privilege is not evidence of low power. Establish one writer
of canonical activity records so FSEvents plus ES cannot double-count a write.
If an ES message cannot be uniquely correlated, keep it as separate process
observation evidence instead of inventing a join.

**Acceptance gate.** Entitled real-machine tests must cover a writer exiting
immediately, PID reuse, rename replacement, long-lived writers, mmap, a dropped
sequence, helper restart, consent revocation, sleep/wake, and changes during
collector downtime. Product claims must match the cases actually observed.

## Linux: fanotify helper, with inotify fallback per filesystem

**Proposed first supported profile.** Start with supported local ext4/XFS
filesystems and a modern LTS kernel, then expand after tests. Feature-probe
`fanotify_init` and `fanotify_mark`; kernel versions alone are insufficient
because distributions backport features. The first complete profile is:

```text
fanotify_init:
  FAN_CLASS_NOTIF | FAN_CLOEXEC | FAN_NONBLOCK
  | FAN_REPORT_DFID_NAME_TARGET | FAN_REPORT_PIDFD

fanotify_mark:
  FAN_MARK_ADD | FAN_MARK_FILESYSTEM

event mask:
  FAN_CREATE | FAN_DELETE | FAN_RENAME | FAN_MODIFY
  | FAN_CLOSE_WRITE | FAN_ATTRIB | FAN_ONDIR
```

This is a planned profile to validate, not an already-tested invocation.
`FAN_REPORT_DFID_NAME_TARGET` includes `DIR_FID | NAME | FID | TARGET_FID`.
`FAN_REPORT_TARGET_FID` requires the other three flags.
`FAN_RENAME` provides one event with old/new directory-name information, rather
than forcing a heuristic join of two move notifications. Mainline support for
`FAN_RENAME` and target FIDs arrived in Linux 5.17, with documented backports to
5.15.154 and 5.10.220. PIDFD reporting arrived in mainline 5.15 and is also
backported to 5.10.220. It cannot be combined with `FAN_REPORT_TID`.
[Initialization flags][fan-init] [Mark/event flags][fan-mark]

**Privilege and coverage.** Filesystem marks require `CAP_SYS_ADMIN` and cover
that filesystem across its mount points. They do not automatically cover a
second filesystem mounted beneath the selected root. Resolve mount topology,
add the appropriate marks for included filesystems, and report unsupported or
inaccessible portions. `FAN_MARK_MOUNT` is not an interchangeable fallback for
this profile: the API rejects mount marks combined with events that require
file handles, including create/move events. Directory marks are not recursive
and reintroduce the new-subdirectory registration race. [Marks][fan-mark]
[Limitations][fanotify]

Prefer an installed service with a bounded capability set and authenticated
local IPC to putting broad capabilities on the desktop executable. If resolving
FIDs with `open_by_handle_at`, the helper also needs `CAP_DAC_READ_SEARCH`;
plan that permission explicitly. Resolve opaque handles against a filesystem
identity and maintain parent/name history for deleted or renamed objects.
A stale handle must remain unresolved evidence, not be silently attached to a
new file at the same path. [Handle resolution][open-handle]

Unprivileged fanotify exists since 5.13 (also backported), but cannot supply
filesystem/mount marks and hides another process's PID. It therefore does not
provide the proposed privileged guarantees. PIDFDs reduce PID-reuse ambiguity,
but the API can fail to obtain one if the process has exited; preserve an
unknown identity in that case. [Initialization and unprivileged limits][fan-init]

**Boundaries that remain even with root.**

- Fanotify has queue and mark limits; filesystem marks reduce mark count, not
  all limits. Keep bounded queues rather than requesting unlimited kernel
  memory; `FAN_Q_OVERFLOW` and userspace drops must initiate an explicit gap.
- FID decoding and filesystem identifiers are not supported everywhere. Handle
  `EOPNOTSUPP`, `ENODEV`, unsupported masks, and missing kernel support as a
  per-watch fallback/failure, never as successful complete coverage.
- Fanotify reports userspace filesystem-API activity, not remote writes made
  elsewhere on a network filesystem. Its manual also explicitly excludes
  accesses/modifications caused by `mmap`, `msync`, and `munmap`. It has no
  persisted event cursor. The default inotify path cannot be assumed to repair
  these blind spots. Reconciliation can repair current state, not recreate
  unseen causal events. [Fanotify limits][fanotify] [Errors][fan-mark]

Use `epoll`/blocking waits, batch reads, close event/pid descriptors promptly,
and compute expensive metadata outside the read loop. A filesystem-wide mark
may receive substantial unrelated traffic; benchmark it against inotify on a
small root instead of always preferring it. Fall back on old/unsupported kernels
with `reports_process = false`; expose incomplete recursive coverage as an
error or degraded state.

**Acceptance gate.** On privileged disposable Linux machines, test ext4 and
XFS, supported/backported flag profiles, watch exhaustion, queue overflow,
renames across watched boundaries, rapid create/delete, mount/unmount/bind
mounts, stale FIDs, process exit, and helper reconnection. Include an mmap and
remote-share test to establish documented limitations. Verify authentication
and root scoping with a second local user.

## Windows: existing USN journal first; ETW attribution separately

**USN collector.** Ship an optional elevated Windows service. Query an existing
journal, enumerate file identities to build the initial parent/name index,
and read forward with `FSCTL_READ_USN_JOURNAL`. Start with NTFS. Treat ReFS as a
separate tested profile because record versions and file identifier widths
vary; parse record lengths and versions rather than assuming `USN_RECORD_V2`.
[USN records][usn-record] [Enumeration][usn-enum]

The supported full-volume route opens a volume handle, for which Microsoft's
`CreateFile` documentation specifies administrative privileges. Do not infer
that `FILE_FLAG_BACKUP_SEMANTICS` itself grants or necessarily requires
`SeBackupPrivilege`: ordinary directory handles use the same flag. Minimize
requested access and validate the actual token/access result. A separate
unprivileged USN interface, if investigated later, must establish its filename
and scope limitations before being treated as equivalent. [Volume handles and
access rules][createfile] [Directory-change handles][rdcw]

Store a cursor as `(volume identity, UsnJournalID, next USN)`, not a bare number.
On every reconnect, validate journal identity and the readable USN range. A
changed journal ID, deletion, `ERROR_JOURNAL_ENTRY_DELETED`, or a cursor older
than retained data requires a reported gap and index/state rebuild. Commit
records and their checkpoint durably together, or make replay idempotent, so a
crash cannot silently skip or double-apply changes. [Journal identity][usn-id]
[Retention bounds][usn-query] [Read parameters and errors][usn-buffer]

USN stores file/parent reference numbers plus a name, not a ready-made full
path. Preserve directory rename history and link relationships; a single
`FRN -> current path` mapping cannot represent every hard link or reconstruct
all old paths. Keep unknown paths explicit when the index is incomplete. Use
journal watermarks around enumeration, drain intervening records, and verify a
second watermark before declaring catch-up complete.

Read the journal already present. Pathlight must not create, resize, or delete
the system's journal as an automatic setup step. If absent, use RDCW and declare
that offline replay is unavailable. This preserves the observation-only scope.

**Live behavior and deduplication.** The journal aggregates reason bits between
opens/closes and Microsoft describes it as a *partial* history. A final close
record can summarize changes already reported in earlier records. Therefore
one record is neither one user action nor a measured write-size delta. The
record has no responsible PID; its `SecurityId` is not process attribution.
[Change-journal semantics][usn-semantics] [Record fields][usn-record]

Use outstanding journal reads (`BytesToWaitFor` with appropriate cancellation)
rather than immediately reissuing empty reads in a busy loop. Never require
`ReturnOnlyOnClose` for a live session where a writer may stay open indefinitely.
RDCW can remain the low-latency trigger source while USN provides durable
continuity, but both must update the same file-state ledger. Require explicit
source identity/watermarks and deduplication before enabling both as activity
producers. RDCW overflow includes a successful return with zero bytes as well
as `ERROR_NOTIFY_ENUM_DIR`; both require enumeration/reconciliation.
[Read behavior][usn-buffer] [RDCW overflow][rdcw]

**Process attribution is another feature.** An optional ETW file-I/O collector
can provide live process/thread evidence; kernel file-I/O events may require
resolving the issuing thread instead of trusting every generic event-header
PID. Evaluate modern event schemas, process lifetime tracking, lost-event
counters, and permission requirements before promising attribution. ETW
observations cannot fill in historical USN PIDs. The documented NT Kernel
Logger control route requires administrator/LocalSystem access.
[File-I/O event data][etw-file] [ETW access][etw-start]

A minifilter is a later, separately justified product investment: it adds a
kernel failure surface, filter-altitude allocation, driver signing, and driver
validation/distribution work. It is not necessary to deliver the first USN
backend, and does not eliminate the need for reconciliation or power testing.
[Altitude assignment][filter-altitude] [Signing policy][filter-sign]

**Acceptance gate.** Use an elevated Windows runner and real NTFS test volume:
long-running writes, retained replay after restart, journal reset/truncation,
absent journal, record-version parsing, directory moves, hard links, reparse
points, volume remount, RDCW overflow, mixed-source deduplication, and crash
between event persistence and checkpoint update. Test unsupported/removable
filesystems and network shares as distinct fallback profiles. Journal mutations
for tests belong only on disposable test volumes under the test harness.

## Android: validate accessible live monitoring; root is experimental

**Do not reject cross-process watching categorically.** The Android
`FileObserver` contract says it uses inotify for files changed by “any process
on the device (including this one).” That is not a guarantee that an app can
access all files, that all storage implementations forward every change, or
that an observer survives process death. AOSP documents multiple external-storage
routes and FUSE bypass/optimization behavior; it does not establish the prior
blanket claim that Android 11+ cannot observe another app's writes.
[FileObserver][android-observer] [AOSP storage][android-storage]

For accessible real local paths, reuse the Rust inotify route or a Kotlin
FileObserver adapter in a user-started session. Maintain recursive registrations
and their failure state; the AOSP native implementation installs one
`inotify_add_watch` for each supplied path, not an automatic recursive watcher.
Test writes from a second app, MediaStore, a direct permitted file path, and the
shell independently on each supported storage/device profile.
[Native implementation][android-observer-src]

A SAF grant is a **document/tree URI**, potentially served by a cloud provider.
It is not a POSIX watch root, and an opened document file descriptor does not
turn an arbitrary document tree into an inotify directory. Add a distinct
provider-backed source using `ContentResolver`/`DocumentsContract`: compare
metadata snapshots when requested, and use provider/MediaStore invalidations
as hints when available. Label the observation interval and incomplete listings
explicitly. Snapshots can be expensive and miss transient files; they are not
inherently a low-cost or instantaneous monitor. [SAF contract][android-saf]
[ContentObserver][android-contentobserver]

`MANAGE_EXTERNAL_STORAGE` broadens access to shared files/direct paths but
still excludes other apps' app-specific directories and does not grant
unrestricted private-data access. Google Play evaluates whether the declared
core use case qualifies; file monitoring is not automatically approved merely
by requesting this permission. [All-files access][android-all-files]

**Background lifetime is a separate capability.** Android 15's documented
six-hours-per-24-hours restriction applies to `dataSync` and `mediaProcessing`
while the app is in the background, not every foreground service type.
`shortService` has its own tighter limit. A valid `specialUse` service can
cover a use case outside the named types, with a declared subtype and Play
review; eligibility is not established for Pathlight. Do not label monitoring
as data sync just to start a service, or advertise indefinite life because
`specialUse` exists. Handle background-start restrictions, user stop, service
termination, and timeout as visible ends/gaps, then reconcile on resumption.
[Timeouts][android-timeout] [Types/specialUse][android-fgs-types]
[Play requirements][android-play-fgs]

**Optional root/OEM tier.** Keep this a separate experimental distribution
profile. On a device already deliberately configured for root, an authorized
native helper may observe more local paths with inotify/fanotify, subject to
kernel configuration, handle support, mount namespaces, credentials, and
SELinux rules. Android enforces SELinux even against root processes. A generic
root switch cannot promise all-app or all-volume monitoring; report a failed
capability probe rather than disabling security policy. An OEM/system-image
integration needs its own signing and SELinux policy work and is not a portable
Play app feature. [Android SELinux][android-selinux] [Fanotify limitations][fanotify]

**Acceptance gate.** Use physical devices covering supported Android versions,
at least a Pixel and another OEM, internal/shared/removable storage, direct-path
and provider writes, provider disconnection, permission revocation, Doze,
user stop, and process kill/restart. Run a separate rooted-device matrix with
SELinux enforcing. Report scope, lifecycle, and observed event coverage per
profile; do not infer universal coverage from one emulator.

## iOS/iPadOS

There is no supported public whole-device privileged collector equivalent to
ES, fanotify, or USN. Endpoint Security SDK entry points are explicitly
unavailable on iOS. Keep the shipping product a desktop-journal viewer with
optional user-requested comparisons within legitimately granted access. Apple's
Background Tasks framework schedules or extends defined tasks; it does not
grant general filesystem access or promise an always-running observer. Do not
make jailbroken-device behavior part of the public compatibility contract.
[Background tasks][ios-background]

## Shared core and helper boundary

This is the first implementation work after the rename duplication fix and its
regression tests. Retain platform-native collection while sharing normalization,
size accounting, exclusion, history, and recovery in Rust.

1. Replace the OS-wide static capability claim with a **negotiated per-watch
   descriptor**, returned only after source activation and coverage validation.
   Include backend/version, actual filesystem/volume/provider scope, identity
   quality, rename semantics, replay conditions, process-evidence availability,
   event-time versus receipt-time meaning, lifecycle, and loss reporting.
   A single `sees_other_processes_writes` boolean cannot express a partially
   accessible Android tree or a remote filesystem.
2. Add a backend-tagged cursor with source epoch/volume identity, stable file
   identity separate from display path, optional process-instance evidence and
   provenance, and explicit coverage/gap state. Preserve compatibility with
   existing journals. A lost process-evidence stream need not invalidate intact
   filesystem totals, but its own coverage must be downgraded.
3. Keep helpers small: probe capabilities, subscribe to scoped roots, stream
   typed records, report health/loss, and stop. No arbitrary shell execution or
   user-file mutation; keep export, UI, and general aggregation unprivileged.
   Authenticate clients through XPC signing/audit identity on macOS, peer
   credentials/policy on Linux, and an ACL-protected named pipe with client
   identity checks on Windows. Scope requests and emitted data to roots the
   authenticated user has explicitly authorized, including symlink/mount changes.
4. Assign each source a stream epoch and ordering token. Deduplicate identical
   source records by source identity; reconcile different sources through file
   identity/state transitions, not path-and-time guesses. Preserve uncertain
   observations as evidence without applying a second byte delta. Test a real
   repeated operation in the same second so deduplication cannot erase it.
5. Keep bounded queues at every boundary and report overflow through the same
   gap channel. Handle helper crash, transport loss, permission loss, and
   backend switching as transitions requiring replay or scoped reconciliation.
   Reconciliation repairs known state and labels the uncertain interval; it
   never advertises that every missed operation or responsible process was
   recovered. Handle topology/coverage loss as well as explicit queue overflow.
6. Establish power and accuracy measurements before defaulting a root to a
   privileged source. Record idle CPU/wakeups, resident memory, event-to-display
   p50/p95/p99 latency, burst throughput, queue high-water marks, lost events,
   and reconciliation cost on fixed hardware/workloads. Use blocking OS event
   waits and batched metadata/journal work; no periodic whole-tree polling.

## Implementation order and release gates

The default-backend duplicate-rename regression is fixed and verified on Linux,
including unmatched moves and repeated operations; see [PLATFORMS.md](PLATFORMS.md).
Next add the shared descriptor/cursor/identity/recovery boundaries without
claiming that privileged collection is active.

Implement Linux fanotify as the first privileged collector because its API can
be exercised on disposable machines without vendor entitlement review. Follow
with Windows USN and its crash-safe path index. In parallel with those coding
stages, obtain the macOS ES entitlement and prepare its distribution setup;
implement and validate that collector once entitlement and test-machine consent
are available. Evaluate Windows ETW only after USN accuracy is established.
Keep Android normal-device and root-device feasibility tracks separate; release
only the profiles supported by device evidence. iOS remains the viewer route.

Each backend must pass parser fixtures, shared actual-filesystem contracts, an
integration test through IPC and journal persistence, forced-loss/restart tests,
and the platform-specific gates above. A hosted CI build alone is not evidence
of root access, ES entitlement, kernel feature support, OEM storage behavior,
or acceptable power consumption. Ship the optional tier only after its measured
coverage and recovery behavior can be described accurately to the user.

## Primary sources

[es]: https://developer.apple.com/documentation/endpointsecurity
[es-new]: https://developer.apple.com/documentation/endpointsecurity/es_new_client(_:_:)
[es-entitlement]: https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.endpoint-security.client
[es-message]: https://developer.apple.com/documentation/endpointsecurity/es_message_t
[es-process]: https://developer.apple.com/documentation/endpointsecurity/es_process_t
[es-seq]: https://developer.apple.com/documentation/endpointsecurity/es_message_t/seq_num
[es-global]: https://developer.apple.com/documentation/endpointsecurity/es_message_t/global_seq_num
[es-wwdc]: https://developer.apple.com/videos/play/wwdc2020/10159/
[fsevents]: https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html
[fan-init]: https://man7.org/linux/man-pages/man2/fanotify_init.2.html
[fan-mark]: https://man7.org/linux/man-pages/man2/fanotify_mark.2.html
[fanotify]: https://man7.org/linux/man-pages/man7/fanotify.7.html
[open-handle]: https://man7.org/linux/man-pages/man2/open_by_handle_at.2.html
[createfile]: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew
[rdcw]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw
[usn-id]: https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier
[usn-query]: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v0
[usn-buffer]: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v0
[usn-record]: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2
[usn-enum]: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data
[usn-semantics]: https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records
[etw-file]: https://learn.microsoft.com/en-us/windows/win32/etw/fileio-readwrite
[etw-start]: https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nf-evntrace-starttracew
[filter-altitude]: https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/minifilter-altitude-request
[filter-sign]: https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later-
[android-observer]: https://developer.android.com/reference/android/os/FileObserver
[android-observer-src]: https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/jni/android_util_FileObserver.cpp
[android-storage]: https://source.android.com/docs/core/storage/scoped
[android-saf]: https://developer.android.com/training/data-storage/shared/documents-files
[android-contentobserver]: https://developer.android.com/reference/android/database/ContentObserver
[android-all-files]: https://developer.android.com/training/data-storage/manage-all-files
[android-timeout]: https://developer.android.com/develop/background-work/services/fgs/timeout
[android-fgs-types]: https://developer.android.com/develop/background-work/services/fgs/service-types
[android-play-fgs]: https://support.google.com/googleplay/android-developer/answer/13392821
[android-selinux]: https://source.android.com/docs/security/features/selinux
[ios-background]: https://developer.apple.com/documentation/backgroundtasks
