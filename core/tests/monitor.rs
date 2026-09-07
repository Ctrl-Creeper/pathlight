//! Cross-platform conformance for the watcher backends.
//!
//! FSEvents, inotify and ReadDirectoryChangesW disagree about nearly
//! everything: coalescing, whether a rename arrives as one event or two, what
//! an event ID means. These tests perform real filesystem operations and
//! assert the *normalized* contract every backend has to honour, so "Pathlight
//! supports this OS" means "this file passes on this OS". Anything a single
//! platform guarantees more strongly declares it in `Capabilities`, and the
//! tests at the bottom hold it to that declaration — a backend may differ, but
//! it may not be wrong about how it differs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pathlight_core::paths;
use pathlight_core::{
    watcher_capabilities, ActivityListener, Change, ChangeKind, StreamEvent, Watcher,
};

#[derive(Default)]
struct Collector(Mutex<Vec<StreamEvent>>);

impl ActivityListener for Collector {
    fn on_event(&self, event: StreamEvent) {
        self.0.lock().unwrap().push(event);
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    root: PathBuf,
    watcher: Arc<Watcher>,
    collector: Arc<Collector>,
}

impl Harness {
    /// Watches a fresh directory, seeded before the watcher arms so setup
    /// writes never show up as changes.
    fn start(seed: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical_root(dir.path());
        seed(&root);
        let collector = Arc::new(Collector::default());
        let watcher = Watcher::start(
            root.to_string_lossy().into_owned(),
            None,
            100,
            collector.clone(),
        )
        .unwrap();
        // Every backend needs a moment to arm; a change produced before that
        // is legitimately lost and would make these tests flaky, not wrong.
        std::thread::sleep(Duration::from_millis(500));
        Self {
            _dir: dir,
            root,
            watcher,
            collector,
        }
    }

    fn events(&self) -> Vec<StreamEvent> {
        self.collector.0.lock().unwrap().clone()
    }

    fn changes(&self) -> Vec<Change> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                StreamEvent::Change { change, .. } => Some(change),
                _ => None,
            })
            .collect()
    }

    /// Polls until `done` accepts the changes seen so far. On timeout it panics
    /// with the whole event log, which is the only way one of these failures is
    /// debuggable on a CI runner.
    fn wait_for(&self, what: &str, done: impl Fn(&[Change]) -> bool) -> Vec<Change> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let changes = self.changes();
            if done(&changes) {
                return changes;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; events: {:#?}",
                self.events()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.watcher.stop();
    }
}

/// `canonicalize` is needed on macOS (FSEvents reports `/private/var`, never
/// the `/var` symlink); `normalize` then undoes the `\\?\` prefix it adds on
/// Windows, which is the same job the crate does to every path it emits.
fn canonical_root(path: &Path) -> PathBuf {
    PathBuf::from(paths::normalize(
        &path.canonicalize().unwrap().to_string_lossy(),
    ))
}

fn named<'a>(changes: &'a [Change], name: &str) -> Vec<&'a Change> {
    changes
        .iter()
        .filter(|change| change.path.ends_with(name))
        .collect()
}

#[test]
fn announces_it_is_live_before_reporting_changes() {
    let harness = Harness::start(|_| {});
    let events = harness.events();
    assert!(
        matches!(events.first(), Some(StreamEvent::HistoryCaughtUp { .. })),
        "the host baselines on this marker; without it, replay and live \
         events cannot be told apart: {events:#?}"
    );
}

#[test]
fn a_write_reports_an_arrival_scoped_to_the_root() {
    let harness = Harness::start(|_| {});
    std::fs::write(harness.root.join("hello.txt"), b"hi").unwrap();

    let changes = harness.wait_for("the created file", |changes| {
        !named(changes, "hello.txt").is_empty()
    });

    let root = harness.root.to_string_lossy().into_owned();
    for change in named(&changes, "hello.txt") {
        assert_ne!(
            change.kind,
            ChangeKind::Deleted,
            "a write must never surface as a deletion: {change:#?}"
        );
        assert_eq!(change.root_path, root, "wrong root on {change:#?}");
        assert!(
            change.path.starts_with(&root),
            "path escaped the watched root: {change:#?}"
        );
    }
}

#[test]
fn a_delete_reports_a_removal() {
    let harness = Harness::start(|root| std::fs::write(root.join("doomed.txt"), b"bye").unwrap());
    std::fs::remove_file(harness.root.join("doomed.txt")).unwrap();

    harness.wait_for("the deletion", |changes| {
        named(changes, "doomed.txt")
            .iter()
            .any(|change| change.kind == ChangeKind::Deleted)
    });
}

#[test]
fn a_rename_inside_the_root_stays_reconstructable() {
    let harness = Harness::start(|root| std::fs::write(root.join("draft.txt"), b"draft").unwrap());
    std::fs::rename(
        harness.root.join("draft.txt"),
        harness.root.join("final.txt"),
    )
    .unwrap();

    // FSEvents pairs by inode, inotify by cookie, Windows hands over both
    // halves at once. The portable floor is only that the host can still see
    // both sides — either paired into one event or as two separate ones.
    harness.wait_for("the rename", |changes| {
        let arrivals = named(changes, "final.txt");
        let paired = arrivals.iter().any(|change| {
            matches!(
                &change.kind,
                ChangeKind::Renamed { previous_path: Some(previous) } if previous.ends_with("draft.txt")
            )
        });
        paired || (!arrivals.is_empty() && !named(changes, "draft.txt").is_empty())
    });
}

#[test]
fn changes_in_a_directory_created_after_arming_are_reported() {
    let harness = Harness::start(|_| {});
    let nested = harness.root.join("nested").join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    // inotify watches one directory at a time, so `notify` has to add a watch
    // for a brand new subtree before anything inside it is visible. FSEvents
    // and ReadDirectoryChangesW are recursive from the start and don't care.
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(nested.join("buried.txt"), b"deep").unwrap();

    harness.wait_for("the nested file", |changes| {
        !named(changes, "buried.txt").is_empty()
    });
}

#[test]
fn event_ids_never_go_backwards() {
    let harness = Harness::start(|_| {});
    for index in 0..5 {
        std::fs::write(harness.root.join(format!("file-{index}.txt")), b"x").unwrap();
    }
    harness.wait_for("five changes", |changes| changes.len() >= 5);

    let ids: Vec<u64> = harness
        .events()
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Change { event_id, .. } => Some(*event_id),
            _ => None,
        })
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] <= pair[1]),
        "IDs are the resume cursor; out-of-order means replaying old changes: {ids:?}"
    );
}

/// Holds a backend to its own `pairs_renames` claim: one user-visible rename
/// must produce exactly one row carrying both paths, with no loose half left
/// for the host to pair. Backends that don't claim it are covered by the
/// weaker portable test above.
#[test]
fn a_backend_claiming_to_pair_renames_really_does() {
    if !watcher_capabilities().pairs_renames {
        return;
    }
    let harness = Harness::start(|root| std::fs::write(root.join("draft.txt"), b"draft").unwrap());
    std::fs::rename(
        harness.root.join("draft.txt"),
        harness.root.join("final.txt"),
    )
    .unwrap();

    let changes = harness.wait_for("the paired rename", |changes| {
        named(changes, "final.txt").iter().any(|change| {
            matches!(
                &change.kind,
                ChangeKind::Renamed { previous_path: Some(previous) } if previous.ends_with("draft.txt")
            )
        })
    });
    assert!(
        named(&changes, "draft.txt").is_empty(),
        "the departure half must be suppressed once it is paired: {changes:#?}"
    );
}

/// Holds a backend to its own `resumable_cursor` claim. A process-local
/// counter restarts at 1 every launch, so passing it back as `since_event_id`
/// would replay history the host already has; only kernel-issued IDs resume.
#[test]
fn a_resumable_cursor_is_not_a_process_local_counter() {
    if !watcher_capabilities().resumable_cursor {
        return;
    }
    let harness = Harness::start(|_| {});
    std::fs::write(harness.root.join("cursor.txt"), b"x").unwrap();

    let changes = harness.wait_for("a change", |changes| !changes.is_empty());
    let id = harness
        .events()
        .iter()
        .find_map(|event| match event {
            StreamEvent::Change { event_id, .. } => Some(*event_id),
            _ => None,
        })
        .unwrap();
    assert!(
        id > 1_000,
        "a process-local counter cannot resume across launches; got {id} for {changes:#?}"
    );
}

/// The declaration itself has to stay meaningful: a backend that can silently
/// lose events and does not admit it would let the host trust an under-count.
#[test]
fn a_backend_that_cannot_lose_events_is_the_only_one_allowed_to_say_so() {
    let capabilities = watcher_capabilities();
    // ponytail: forcing a real kernel queue overflow is not worth the test
    // time, so this checks the honest default rather than the overflow path.
    assert!(
        capabilities.may_drop_events,
        "no shipping backend has a lossless queue; {capabilities:#?} claims one"
    );
}
