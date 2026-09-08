# Monitoring evidence foundation implementation plan

**Approved design:** `core/research/2026-09-08-monitoring-evidence-review.md`,
accepted in conversation on 2026-09-08. Execution uses independent subagents
with explicit file ownership and main-agent integration/review.

**Goal:** Deliver real native measurements, identity-aware interval snapshots,
loss-aware evidence recording and conservative macOS recovery before privileged
collectors consume the same contracts.

**Architecture:** Keep legacy Swift-compatible activity JSONL readable. Add a
versioned evidence record family with native paths, source identity, measurement
provenance and explicit gaps. Share measurement and reconciliation in Rust;
correct the current Swift recovery path while its migration proceeds.

**Constraints:** Read user files only; no periodic full-tree scan; bounded event
queues; measurements are not physical write I/O; platform support follows actual
test evidence. Existing private agent configuration is not part of commits.

## Tasks and ownership

- [x] Resolve pre-existing merge while retaining newer FD filtering, hint expiry,
  actor annotations, presets and duplicate-journal corrections. Run Swift tests.
- [x] Native measurements (`measurement.rs`, `tests/measurement.rs`, Cargo target
  dependency and `attribution::allocated_size` delegation): separate logical and
  allocated bytes, scoped native identity, link count and file kind. Verify real
  hard links, rename, sparse files, missing paths, no-follow behavior.
- [x] Native name handling (`paths.rs`, `notify_backend.rs`, monitor tests):
  preserve POSIX backslashes; do not forward lossy native paths as another file.
  Keep raw native paths in new evidence, with legacy unsupported-name gaps.
- [x] Rust snapshots (`snapshot.rs`, tests): record scan interval and per-path
  errors, retain native path bindings, count known identities once, and derive
  endpoint changes without pretending to recover operation history.
- [x] Swift recovery (`ActivityBaselineService.swift`, tests and AppModel):
  backward-compatible scan intervals, hard-link accounting, reject incomplete
  baseline corrections, begin observing before scanning, preserve history gaps.
- [x] Evidence persistence and integration (`evidence.rs`, tests and recording
  command): source epoch/sequence, tagged records, measured state distinct from
  events, replay deduplication and explicit loss; retain legacy journal fixtures.
- [x] Build/verify on macOS and Linux: `cargo fmt --check`, `cargo clippy
  --all-targets -D warnings`, `cargo test` (59 tests macOS, 65 Linux incl.
  inotify), `swift test` (96 tests), and a real `pathlight-monitor` recording.
- [ ] Build/verify remaining, CI-only here: Windows and Android compile checks
  need cross toolchains this machine lacks (no rustup targets, no NDK); the
  Xcode app bundle build needs the universal xcframework. CI covers all three.
  Documented capability status still needs the Linux inotify evidence folded in.
- [ ] Commit reviewed increments using Conventional Commits.

## Acceptance cases

One rename yields one normalized change; two real renames remain distinct.
Hard-link aliases remain separate names but have one measured object. Sparse
logical size differs from allocated size. A scan read failure cannot masquerade
as freed space. A current-state recovery never erases an older history gap.
Repeated source records do not apply twice; a reused numeric event ID in a new
source epoch remains a new observation. Invalid native encoding stays explicit.
Existing legacy activity fixtures still round-trip.

Privileged collector rollout follows this foundation: Linux fanotify first,
Windows USN recovery next, and entitled macOS ES on qualified machines. The
entitlement and device testing requirements remain real release gates; no
privileged source is claimed active merely because a type or plan exists.
