# Monitoring evidence: architecture fact-check

Reviewed 2026-09-08 against primary documentation and the installed Apple SDK.
This note checks architectural claims; it does not certify new privileged
backends through runtime tests. Statements marked **Recommendation** or
**Analysis** are design conclusions, not API guarantees.

## macOS: observation is not a write-byte measurement

1. **Documented: Endpoint Security write events have no byte count.**
   `es_event_write_t` contains `es_file_t *target` and reserved bytes. The public
   payload has no write length, offset, or completed-byte count. A process is
   identified separately in the enclosing message. Neither counting these
   messages nor subtracting `stat` sizes yields that process's bytes written.
   [Apple write event][es-write]; installed SDK `ESMessage.h`, lines 733–743.

2. **Documented: `was_mapped_writable` is a risk indicator, not proof of a write.**
   The SDK says it indicates that “at some point in the lifetime of the target
   file vnode it was mapped into a process as writable.” It explicitly “does
   not indicate whether the file has actually been written to” through mapped
   memory and does not indicate whether it is **currently** mapped writable.
   The state is about the vnode lifetime, not just the current descriptor,
   closing process, or observation interval. Do not attribute a dirty page or a
   write to the closer merely because this field is true.
   [Apple close event][es-close]; SDK `ESMessage.h`, lines 626–653.

3. **Documented: the runtime gate is `message.version >= 6`.**
   The SDK marks this close-event field “available only if message version >= 6.”
   It also states that `modified` reflects filesystem syscalls; if modification
   happened only through a memory mapping, `modified` can be false while
   `was_mapped_writable` is true. That is not evidence that *every* writable
   mapping modified the file. The public field documentation fetched for this
   review provides no `introducedAt` macOS version, and the installed SDK has
   no per-field OS annotation. Therefore an exact “introduced in macOS X” claim
   remains unverified here; use the documented message-version check rather
   than an inferred deployment-target check. [Field documentation][es-mapped]

   **Recommendation:** A true flag may trigger a scoped remeasurement or mark
   content-change evidence as uncertain. Remeasuring size cannot detect an
   equal-length overwrite. A content hash can compare sampled contents at a
   cost; it still cannot prove how many intermediate writes occurred.

## Linux: the exact fanotify profile matters

4. **Documented: filesystem and mount marks are not interchangeable.**
   `FAN_MARK_MOUNT` rejects masks requiring file-handle identification, with
   examples including `FAN_CREATE`, `FAN_ATTRIB`, and `FAN_MOVE`, returning
   `EINVAL`. `FAN_RENAME` also requires a group identifying objects by file
   handles. Use a supported `FAN_MARK_FILESYSTEM` profile for the proposed
   directory-entry/FID pipeline, or explicitly negotiate a weaker profile.
   A filesystem mark covers that filesystem across mount points; it does not
   imply that a different filesystem nested under the watch root is covered.
   [fanotify_mark(2)][fan-mark]

5. **Documented: the combined FID/PIDFD profile has dependencies and failures.**
   `FAN_REPORT_DFID_NAME_TARGET` is shorthand for
   `FAN_REPORT_DFID_NAME | FAN_REPORT_FID | FAN_REPORT_TARGET_FID`;
   `DFID_NAME` itself includes `DIR_FID | NAME`. `TARGET_FID` requires those
   other flags. `FAN_RENAME` and target FIDs arrived in mainline 5.17 with
   documented backports to 5.15.154 and 5.10.220. PIDFD reporting arrived in
   mainline 5.15, also backported to 5.10.220; it cannot be combined with
   `FAN_REPORT_TID`. A PIDFD may be unavailable for an already-exited process.
   Probe actual flag combinations, kernel support, and filesystem support;
   version strings or an installed root helper do not prove the profile works.
   [fanotify_init(2)][fan-init] [fanotify(7)][fanotify]

6. **Documented: root fanotify retains the mmap and remote-write blind spots.**
   Its manual states: “does not report file accesses and modifications that may
   occur because of mmap(2), msync(2), and munmap(2).” It also excludes remote
   events occurring on network filesystems. An event-driven baseline repair
   needs a trigger; a silent blind spot does not necessarily produce an
   overflow marker that tells the application to scan. [fanotify(7)][fanotify]

   **Analysis: eBPF is an investigative extension, not a generic completeness
   guarantee.** Instrumenting a syscall sees that syscall, not all CPU stores
   through existing mappings. A page-fault/dirtying observation does not count
   every later store to an already-writable/dirty page. A writeback observation
   concerns later cache-to-storage work and does not automatically identify the
   process that dirtied each byte. The kernel VFS documentation distinguishes
   application writes into the address space from later page writeback.
   Kernel cgroup documentation even warns that simultaneous writers to one
   inode can cause a “significant portion of IOs” to be attributed incorrectly
   in its own writeback attribution model. This is evidence that attribution
   requires a carefully defined measurement point, not that every possible BPF
   approach fails. BPF output buffers can also fail reservations when full.
   [VFS writeback][vfs] [Cgroup writeback][cgroup] [BPF ring buffer][bpf-ring]

   **Recommendation:** Before scheduling an eBPF backend, define the exact
   observable (write-syscall return bytes, dirty-page transitions, or block
   requests), supported hooks/kernel profiles, loss accounting, and overhead
   tests. Keep these measurements separate from allocated-storage deltas.

## Android: quantify only a denominator the app can observe

7. **Documented: access boundaries preclude an assumed whole-device denominator.**
   Even `MANAGE_EXTERNAL_STORAGE` does not expose other apps' app-specific
   storage directories. SAF returns a selected document/tree **URI**, possibly
   from a cloud provider; it does not guarantee a POSIX directory suitable for
   inotify. A user grant therefore does not establish complete live-event
   visibility over that provider. [All-files access][all-files] [SAF][saf]

   **Analysis:** “93% coverage” is not meaningful without a defined denominator.
   Counting accessible entries cannot establish a percentage of all device
   entries, all changed files, or all operations in inaccessible private trees.
   Those denominators are unobserved. A defensible metric could be “93 of 100
   requested, enumerated roots activated successfully,” with the root set,
   weighting, time, and exclusions stated. It measures activation coverage,
   not event recall. Test-operation recall on a controlled corpus is another
   valid metric and should be labelled as a test result, not a device guarantee.

8. **Documented: FileObserver supports cross-process events within its scope.**
   Its API describes inotify events after files change by “any process on the
   device (including this one).” This contradicts a blanket “other apps' writes
   can never be seen” claim; it does not guarantee arbitrary access or every
   OEM/provider storage path. Validate direct paths, second-app writers,
   MediaStore/provider writers, and lifecycle on actual devices.
   [FileObserver][file-observer]

9. **Documented: the six-hour rule applies to named service types.**
   Android 15 limits background `dataSync` and `mediaProcessing` foreground
   services to six hours per 24 hours, tracked separately by type.
   `shortService` has a different tighter limit. `specialUse` is a distinct
   category whose declared use case is subject to Play review, not a guaranteed
   permission to keep this monitor resident. A foreground service does not
   expand storage access. Root also does not erase Android SELinux enforcement.
   [Timeouts][fgs-timeout] [Service types][fgs-types] [SELinux][selinux]

## iOS/iPadOS: continued processing is task completion

10. **Documented: `BGContinuedProcessingTask` begins with iOS/iPadOS 26.0.**
    Apple describes a task that starts in the foreground and continues in the
    background as needed. The request must be submitted from the foreground
    “as a result of a person's action,” for example a button press. Progress is
    displayed in a Live Activity; the user can cancel. The system may terminate
    work under runtime resource constraints and prioritizes termination of
    tasks showing little or no progress. This is support for completing
    user-initiated work, not an always-resident filesystem observer. “Bounded”
    here means a task with progress/completion and cancellation; the cited API
    pages do not establish a fixed universal N-minute execution limit.
    [Task][continued] [Request][continued-request] [WWDC25 explanation][bg-wwdc]

    **Recommendation:** Use it, where appropriate, to finish a user-requested
    export or granted-document comparison. It provides no new whole-device
    file-access authority, and journal viewing/sync remains a separate function.

## Sources and review limits

The local SDK source was read from
`/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk/usr/include/EndpointSecurity/ESMessage.h`.
Apple's developer-document JSON was used to inspect symbol metadata and prose.
No privileged client, eBPF probe, or mobile device was exercised in this review.
Exact ES field OS-introduction dating, provider-specific visibility, BPF hook
coverage, and power budgets still need qualification; none is inferred from
these documentation checks.

[es-write]: https://developer.apple.com/documentation/endpointsecurity/es_event_write_t
[es-close]: https://developer.apple.com/documentation/endpointsecurity/es_event_close_t
[es-mapped]: https://developer.apple.com/documentation/endpointsecurity/es_event_close_t/was_mapped_writable-5iaxq
[fan-mark]: https://man7.org/linux/man-pages/man2/fanotify_mark.2.html
[fan-init]: https://man7.org/linux/man-pages/man2/fanotify_init.2.html
[fanotify]: https://man7.org/linux/man-pages/man7/fanotify.7.html
[vfs]: https://docs.kernel.org/filesystems/vfs.html
[cgroup]: https://docs.kernel.org/admin-guide/cgroup-v2.html#writeback
[bpf-ring]: https://docs.kernel.org/bpf/ringbuf.html
[all-files]: https://developer.android.com/training/data-storage/manage-all-files
[saf]: https://developer.android.com/training/data-storage/shared/documents-files
[file-observer]: https://developer.android.com/reference/android/os/FileObserver
[fgs-timeout]: https://developer.android.com/develop/background-work/services/fgs/timeout
[fgs-types]: https://developer.android.com/develop/background-work/services/fgs/service-types
[selinux]: https://source.android.com/docs/security/features/selinux
[continued]: https://developer.apple.com/documentation/backgroundtasks/bgcontinuedprocessingtask
[continued-request]: https://developer.apple.com/documentation/backgroundtasks/bgcontinuedprocessingtaskrequest
[bg-wwdc]: https://developer.apple.com/videos/play/wwdc2025/227/

## Identity, accounting and scan consistency

Additional primary-source checks and repository inspection by the main agent.
These are architectural recommendations, not implemented changes.

### A scan is an observation interval, not necessarily an atomic snapshot

Apple's FSEvents guide explicitly requires starting monitoring **before** a
metadata scan, then rescanning directories changed during the scan. It warns
against determining this solely by comparing event timestamps to scan timestamps.
Linux `stat(2)` additionally notes that returned fields may describe different
moments during the system call. A live recursive traversal does not inherit the
consistency guarantees of a filesystem-native snapshot. [FSEvents snapshot
construction][review-fsevents] [stat consistency][review-stat]

Record `scan_started_at`, `scan_finished_at`, read errors, source epoch/cursor
boundaries where available, and directories dirtied during traversal. Start
collection first, scan, and recheck affected directories within a bounded budget.
If churn, gaps or unreadable entries prevent convergence, expose partial or
unstable state. Neither timestamps nor one final volume-size total prove that
the complete directory state existed at a single instant. Filesystem-native
snapshots can improve consistency where available, but creating one is a separate
privileged/system-state action with retention and storage cost; it is not assumed
for Pathlight's ordinary scans.

Gap-triggered, startup/resume and user-requested reconciliation fit the current
no-polling product promise. Repeated full-tree scans as a default would change
that promise. Sources with silent blind spots cannot provide continuous-state
completeness merely by waiting for an overflow flag: declare those unsupported
operations, and optionally offer a separately budgeted consistency audit with
its own observation interval. This is an explicit trade-off, not a free fix.

### Object identity has a scope and a lifetime

Linux inode numbers are unique within a filesystem; an inode alone is not a
machine-wide identity. Windows documents that file IDs may be reused and may
change on some filesystems; a ReFS identity requires the wider supported ID.
The supported ID plus volume identity is evidence for linking observations, not
an eternal global UUID. [Linux inode][review-inode] [Windows ID limits][review-id]
[128-bit file IDs][review-id128]

Use native identity plus filesystem/provider scope and lifecycle evidence.
Preserve a generation/incarnation if the source actually provides one; otherwise
record the weaker lifetime guarantee. Do not invent continuity across deletion,
unknown gaps, remounts, restores or provider resets. Keep source epochs separately
from object identity, since two sources may observe the same object.

Maintain directory-entry bindings `(parent identity, native name) -> object`
separately from object state. A rename changes a binding, replacement can change
which object occupies a name, and hard links create several bindings to one
object. Clones usually create separate objects that share physical extents.
Name encodings also remain platform-specific: POSIX bytes, Windows UTF-16 and
provider URIs must not be collapsed into a lossy display string for identity.

### Per-file allocated size is not exclusive physical usage

Apple's installed `man 2 clonefile` says a clone shares its source's data blocks
and later writes are private through copy-on-write. The local `man 2 unlink`
says an unlinked file can retain resources until outstanding references close.
Linux `st_blocks` describes allocated blocks, whereas `st_size` describes file
length. Windows exposes `AllocationSize` separately from `EndOfFile`.
[Linux allocation fields][review-inode] [Windows allocation fields][review-allocation]

Consequences: deduplicating hard links by object identity is necessary, but
summing allocated bytes of unique objects still does not prove exclusive,
reclaimable or device-wide physical space. Clones/reflinks and retained snapshots
share extents. A link leaving the watched tree can reduce the tree's attributed
size while releasing no storage on the volume. Keep the accounting scope and
shared-extent knowledge with each metric; use unknown where unsupported.

### Notification counts are observations, not a universal operation count

`inotify(7)` explicitly says coalescing means it cannot reliably count file
events. Microsoft documents several writes resulting in one
`USN_REASON_DATA_OVERWRITE` record; final close can summarize previous reasons.
Neither number of callbacks nor number of reason bits is a count of user actions.
[Inotify coalescing][review-inotify] [USN partial history][review-usn]

Expose separate metrics with time window, scope and freshness:

| Metric | Meaning |
| --- | --- |
| Observed change notifications/s | Received observations after explicitly described filtering; not syscall count |
| Distinct observed objects changed/window | Identity-deduplicated observed objects, only where identity permits it |
| Logical-size change | Difference in measured logical lengths for the chosen scope |
| Reported allocated-size change | Difference in filesystem-reported allocation with stated sharing assumptions |
| Observed application I/O bytes | Advanced telemetry's specified layer; distinguish attempted and completed I/O |
| Device/block I/O bytes | Device-layer counter, with defined scope; does not imply accurate per-file/process attribution |

Do not infer physical NAND write bytes from host/block I/O either. Firmware and
storage-layer behavior require their own measurements. A baseline sampled over
an interval cannot support an exact instantaneous MB/s claim.

## Proposed evidence model for this repository

The user's field list is a good starting point. It combines several entities
whose lifetimes and certainty differ; separating them avoids attaching a
confirmed process identity to a merely estimated size delta.

| Record | Minimum responsibility |
| --- | --- |
| Source observation | Source/backend, stream epoch, sequence/cursor when provided, event kind, native object/name evidence, receipt time, optional source event time and timestamp semantics |
| Object and name state | Scoped native identity, lifetime evidence, parent/name bindings, logical/allocation observations with their measurement times and errors |
| Derived change | Referenced observations and before/after measurements, normalization rule version, field-level certainty, accounting scope |
| Watch coverage and recovery | Authorized scope, activated backend/features, exclusions, known blind spots, health, loss/replay interval, scan consistency and reconciliation status |

Use a monotonic receipt clock for in-process latency and wall time for display;
never infer total ordering between independent clocks/sources. Preserve schema
versions, source epochs and provenance without requiring an unlimited raw-event
archive. Existing privacy choices and bounded retention still apply. Commit
journal output and its cursor atomically, or guarantee idempotent replay;
otherwise a crash can invalidate a correct in-memory deduplication algorithm.

`GapState` needs two separate outcomes: current state may become reconciled while
historical evidence remains permanently incomplete. A missing sequence number
and an API's documented inability to report mmap are different conditions. No
normalizer should erase that distinction.

A coverage percentage is meaningful only for a known denominator and a defined
quantity, such as successfully registered directories among enumerated eligible
directories. It says nothing about inaccessible unknown directories or the
probability of observing every operation. Prefer scoped statuses to an invented
"93% of shared storage" claim.

## Concrete gaps found in the current code

These findings come from code inspection; this research task did not modify
runtime behavior or run new tests.

- `core/src/attribution.rs::allocated_size` returns `metadata.len()` on non-Unix
  platforms. On Windows that is logical length, despite the function promising
  allocated size. A versioned measurement type should eliminate the silent mix.
- `SizeIndex` is keyed only by path. `ActivityBaselineService.swift` similarly
  deduplicates traversal by path, so two hard links can contribute twice to a
  storage total. Neither structure records shared-extents knowledge.
- `ActivityEvent` has one `byte_delta`, one timestamp, one confidence and an
  optional process name. It cannot express the evidence/measurement separation
  above or distinguish source event time from delayed receipt/measurement time.
- `ActivityBaselineSnapshot.capturedAt` is supplied at scan start; the walk can
  yield under pressure, yet there is no scan interval or consistency marker.
  Reconciliation compares totals and event wall times; it cannot prove complete
  operation history or an atomic state.
- `Capabilities` is still a platform-wide constant. Per-watch access, filesystem
  support, observer lifetime and partial registration need runtime state.
- `paths::normalize` replaces backslashes on all platforms, and notify conversion
  uses `to_string_lossy`. POSIX backslashes are valid name characters and invalid
  UTF-8 names can collide after display conversion. Native path keys must remain
  separate from presentation normalization.

## Revised priority and acceptance evidence

1. Specify measurement units/scopes, native object/name identity, provenance and
   schema compatibility. Correct the Windows logical/allocation mismatch and
   preserve native path names before adding more event sources.
2. Implement identity-aware state transitions and interval-aware reconciliation.
   Test rename, replacement, hard links inside/outside the root, unlinked open
   files, sparse files, clones and identity reuse across discontinuities.
3. Make loss, partial coverage and recovery explicit end to end, including
   callback queues, IPC, durable journal output, cursor commit and backend switch.
4. Integrate fanotify, USN and ES one at a time against that same model. Keep
   Android device/provider coverage negotiated and advanced I/O telemetry opt-in.
5. Publish workload-specific accuracy and power results alongside limits.

Benchmarks need an independent workload log of **successful filesystem actions**,
expected resulting state and object/name relationships. Do not define ground
truth as the output of the watcher under test. Report observation loss separately
from valid coalescing; compare normalized semantics and state convergence, not
one callback per syscall. Separate warm steady-state throughput from registration,
recovery and event-to-display latency; include admitted/dropped counts and backlog
so a low p99 among surviving events cannot hide massive loss. State OS/kernel,
filesystem, hardware, power mode, dataset, coalescing settings, observation window
and repeated-run variance. A target such as 100k operations/s is a workload to
qualify, not a universal promise for all filesystems and operating systems.

The proposed API choices are credible engineering choices. Numerical rankings
such as 9.5/10 and parity with a commercial EDR collection stack do not follow
from API documentation alone; they require a defined comparison and measured
reliability, deployment and operational evidence.

[review-fsevents]: https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html
[review-stat]: https://man7.org/linux/man-pages/man2/stat.2.html
[review-inode]: https://man7.org/linux/man-pages/man7/inode.7.html
[review-id]: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information
[review-id128]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info
[review-allocation]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_standard_info
[review-inotify]: https://man7.org/linux/man-pages/man7/inotify.7.html
[review-usn]: https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records
