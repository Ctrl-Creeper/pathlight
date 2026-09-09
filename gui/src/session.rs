//! One live watch: kernel events in, attributed rows out, journal on disk.
//!
//! The watcher calls back on its own thread and must not be made to wait, so
//! callbacks only enqueue. A worker thread does the measuring, the attribution
//! and the file I/O, and the UI thread only ever reads a snapshot behind a
//! mutex. Nothing on the paint path touches the disk.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use pathlight_core::attribution::{allocated_size, AggregationOptions, Attributor, SizeIndex};
use pathlight_core::exclusion::{ExclusionFilter, DEFAULT_PATTERNS};
use pathlight_core::monitor::{ActivityListener, Change, StreamEvent, Watcher};
use pathlight_core::snapshot::{
    self, BindingChange, BindingChangeKind, IdentityContinuity, ScanSnapshot,
};
use pathlight_core::{paths, ActivityEvent, Confidence, EventKind};

use crate::store::Storage;

/// The interactive coalescing window. Wide enough that a save is one event
/// rather than five, short enough to feel immediate.
const LATENCY_MS: u64 = 250;
/// How long a batch waits before it is attributed and written.
const FLUSH: Duration = Duration::from_millis(200);
/// Bounded so a burst costs memory it cannot exceed. Overflow is counted and
/// reported, never quietly dropped.
const QUEUE_CAPACITY: usize = 4096;
/// How often the journal is trimmed while a watch runs. Once at the start
/// would leave a months-long watch untrimmed; more often would re-read the
/// whole file for nothing.
const TRIM_EVERY: Duration = Duration::from_secs(3600);
/// Rows the UI keeps. Totals are kept separately and cover every row, because
/// totalling only what fits on screen makes a busy disk read as a quiet one.
const MAX_ROWS: usize = 500;

/// What the UI reads. Every field is cumulative for the life of the session.
#[derive(Debug)]
pub struct Live {
    /// Newest first, at most [`MAX_ROWS`].
    pub rows: VecDeque<ActivityEvent>,
    pub total_byte_delta: i64,
    pub event_count: u64,
    /// Times the backend admitted it lost events. A gap means the numbers
    /// below it are a floor, not a total, and the UI says so.
    pub gaps: u64,
    /// Changes found by comparing the folder against its baseline after a
    /// gap. Counted apart from [`Self::event_count`]'s share of them because
    /// a comparison says what is different now, never what happened.
    pub recovered: u64,
    pub dropped: u64,
    pub started_at: SystemTime,
    /// Set when a row could not be written. The row is still on screen, so
    /// saying nothing here would be the difference between a record and a
    /// display nobody can get back.
    pub error: Option<String>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            rows: VecDeque::new(),
            total_byte_delta: 0,
            event_count: 0,
            gaps: 0,
            recovered: 0,
            dropped: 0,
            started_at: SystemTime::now(),
            error: None,
        }
    }
}

pub struct Session {
    live: Arc<Mutex<Live>>,
    /// Held only to keep the watch open; dropping this ends the watch, which
    /// closes the queue, which ends the worker.
    _watcher: Arc<Watcher>,
}

impl Session {
    pub fn start(root: &str, storage: Storage) -> Result<Self, String> {
        // Built before the watch opens: a filter that failed to compile after
        // events started arriving would record the noise it exists to drop.
        let exclusions =
            ExclusionFilter::new(DEFAULT_PATTERNS, root).map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let live = Arc::new(Mutex::new(Live::default()));
        let watcher = Watcher::start(
            root.to_owned(),
            None,
            LATENCY_MS,
            Arc::new(QueueListener {
                sender,
                dropped: dropped.clone(),
            }),
        )
        .map_err(|error| error.to_string())?;

        let worker = Worker {
            scope: root.to_owned(),
            storage,
            exclusions,
            live: live.clone(),
            dropped,
        };
        std::thread::Builder::new()
            .name("pathlight-session".into())
            .spawn(move || worker.run(receiver))
            .map_err(|error| error.to_string())?;

        Ok(Self {
            live,
            _watcher: watcher,
        })
    }

    /// Reads the live state. Held for as long as one frame's drawing takes.
    pub fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().expect("session state mutex poisoned")
    }
}

struct QueueListener {
    sender: SyncSender<StreamEvent>,
    dropped: Arc<AtomicU64>,
}

impl ActivityListener for QueueListener {
    fn on_event(&self, event: StreamEvent) {
        // try_send, never send: blocking here would stall the kernel callback
        // and turn a busy disk into a hung watch.
        if self.sender.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The last known state of the watched folder, shared with the thread that
/// takes it.
type Baseline = Arc<Mutex<Option<ScanSnapshot>>>;

/// What the folder holds now. A folder that cannot be read has no baseline,
/// which is said out loud: silently having none would turn every later gap
/// into a gap nobody can reconcile.
fn baseline_of(scope: &str, live: &Arc<Mutex<Live>>) -> Option<ScanSnapshot> {
    match snapshot::scan(Path::new(scope)) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            live.lock().unwrap_or_else(PoisonError::into_inner).error = Some(format!(
                "Could not read {scope} to know what it holds, so a dropped event cannot \
                 be recovered here: {error}"
            ));
            None
        }
    }
}

struct Worker {
    scope: String,
    storage: Storage,
    /// The noise nobody asked to be told about: `.DS_Store`, trashes,
    /// caches, half-finished downloads.
    // ponytail: the shipped defaults, not per-watch patterns. Add an editor
    // and a list in `watches.json` when somebody needs a rule of their own;
    // until then one list is the whole feature.
    exclusions: Option<ExclusionFilter>,
    live: Arc<Mutex<Live>>,
    dropped: Arc<AtomicU64>,
}

impl Worker {
    fn run(self, receiver: mpsc::Receiver<StreamEvent>) {
        let index = SizeIndex::default();
        let scope = self.scope.as_str();
        // The size provider records what it measured, so a later deletion of
        // the same path still has a size to report.
        let size = |path: &str| {
            let size = allocated_size(Path::new(path));
            index.record(scope, path, size);
            size
        };
        let prior_size = |path: &str| index.take(scope, path);
        let known_size = |path: &str| index.peek(scope, path);
        let attributor = Attributor::new(
            AggregationOptions::SHORT_TERM,
            &size,
            &prior_size,
            &known_size,
        );
        let mut pending: Vec<Change> = Vec::new();
        // What the folder looked like when the watch opened. Without it a gap
        // can only ever be counted; with it the gap becomes a list of files.
        //
        // Taken on its own thread, because a whole-disk baseline can take
        // minutes and a worker busy walking a tree is a worker not draining
        // the queue — the events that would drop while it walked are exactly
        // the ones the baseline exists to recover. A gap that arrives before
        // it lands finds nothing to compare against, and says so.
        let baseline: Baseline = Arc::new(Mutex::new(None));
        {
            let slot = baseline.clone();
            let scope = self.scope.clone();
            let live = self.live.clone();
            let spawned = std::thread::Builder::new()
                .name("pathlight-baseline".into())
                .spawn(move || {
                    let taken = baseline_of(&scope, &live);
                    *slot.lock().unwrap_or_else(PoisonError::into_inner) = taken;
                });
            if let Err(error) = spawned {
                self.live().error = Some(format!("Could not take a baseline: {error}"));
            }
        }
        let mut due = Instant::now() + FLUSH;
        // Before the first row of this watch, so a journal left over the cap
        // by an earlier run does not have to wait an hour to come back under it.
        self.trim();
        let mut trim_due = Instant::now() + TRIM_EVERY;
        loop {
            let wait = due.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(wait) {
                Ok(event) => self.accept(event, &mut pending, &baseline),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush(&attributor, &mut pending);
                    return;
                }
            }
            let now = Instant::now();
            if now >= due {
                self.flush(&attributor, &mut pending);
                due = now + FLUSH;
            }
            if now >= trim_due {
                self.trim();
                trim_due = now + TRIM_EVERY;
            }
        }
    }

    fn accept(&self, event: StreamEvent, pending: &mut Vec<Change>, baseline: &Baseline) {
        match event {
            StreamEvent::Change { change, .. } => {
                if !self.excluded(&change.path) {
                    pending.push(change);
                }
            }
            StreamEvent::RequiresRescan { .. } => self.reconcile(baseline),
            StreamEvent::HistoryCaughtUp { .. } => {}
        }
    }

    /// The backend admitted it lost events. Compare the folder against the
    /// baseline and record the difference.
    ///
    /// This recovers what is different, never what happened: a file written
    /// and deleted inside the gap leaves nothing to compare, and a rename is
    /// only a rename because two ends matched an inode that the kernel is
    /// free to have reused. Every row it writes says `Estimated`.
    fn reconcile(&self, baseline: &Baseline) {
        self.live().gaps += 1;
        let mut slot = baseline.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(previous) = slot.take() else {
            return;
        };
        let Some(current) = baseline_of(&self.scope, &self.live) else {
            // Keep the old baseline: it is still the last thing known, and a
            // watch with no baseline can never reconcile again.
            *slot = Some(previous);
            return;
        };
        let comparison = snapshot::reconcile(&previous, &current, IdentityContinuity::Unknown);
        let events: Vec<ActivityEvent> = comparison
            .bindings
            .iter()
            .filter_map(|binding| self.recovered_row(binding, &previous, &current))
            .collect();
        let incomplete = !previous.is_complete() || !current.is_complete();
        *slot = Some(current);
        drop(slot);

        let recovered = events.len() as u64;
        self.publish(events, 0);
        let mut live = self.live();
        live.recovered += recovered;
        if incomplete {
            live.error = Some(
                "Parts of this folder could not be read while catching up, so the recovered \
                 list may be short."
                    .to_owned(),
            );
        }
    }

    /// One difference between two observations, as a row — or nothing, when it
    /// is a path no watch here records.
    fn recovered_row(
        &self,
        binding: &BindingChange,
        previous: &ScanSnapshot,
        current: &ScanSnapshot,
    ) -> Option<ActivityEvent> {
        let path = paths::normalize(&binding.path.to_string_lossy());
        if self.excluded(&path) {
            return None;
        }
        let allocated = |snapshot: &ScanSnapshot, at: &Path| {
            snapshot
                .entries
                .get(at)
                .and_then(|entry| entry.allocated_bytes)
                .map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX))
        };
        let gained = allocated(current, &binding.path);
        let lost = allocated(
            previous,
            binding.previous_path.as_deref().unwrap_or(&binding.path),
        );
        Some(ActivityEvent {
            kind: match binding.kind {
                BindingChangeKind::Created | BindingChangeKind::HardLinkAdded => EventKind::Created,
                BindingChangeKind::Removed | BindingChangeKind::HardLinkRemoved => {
                    EventKind::Deleted
                }
                BindingChangeKind::Replaced => EventKind::Modified,
                BindingChangeKind::Renamed => EventKind::Moved,
            },
            path,
            root_path: self.scope.clone(),
            // Now, not when it happened: the whole point of a gap is that
            // nobody knows when inside it anything happened.
            timestamp: SystemTime::now(),
            byte_delta: match (gained, lost) {
                (None, None) => None,
                (gained, lost) => Some(gained.unwrap_or(0) - lost.unwrap_or(0)),
            },
            confidence: Confidence::Estimated,
            previous_path: binding
                .previous_path
                .as_ref()
                .map(|at| paths::normalize(&at.to_string_lossy())),
            affected_item_count: 1,
            process_name: None,
        })
    }

    /// Storage first, then the noise patterns. The two are not the same kind
    /// of rule: the storage guard is what stops the feedback loop and is not
    /// negotiable, the patterns are a convenience.
    fn excluded(&self, path: &str) -> bool {
        self.storage.is_own(path)
            || self
                .exclusions
                .as_ref()
                .is_some_and(|filter| filter.excludes(path))
    }

    /// A failed trim is said out loud rather than retried: the journal is
    /// still readable and still being written, but it is over a limit the
    /// user was promised, and only they can free the disk it sits on.
    fn trim(&self) {
        if let Err(error) = self.storage.trim_journal() {
            self.live().error = Some(format!("Could not trim the journal: {error}"));
        }
    }

    fn flush(&self, attributor: &Attributor<'_>, pending: &mut Vec<Change>) {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if pending.is_empty() && dropped == 0 {
            return;
        }
        let events = attributor.process(pending);
        pending.clear();
        self.publish(events, dropped);
    }

    /// Journal first, then the screen. A row on screen that was never written
    /// is the difference between a record and a display nobody can get back.
    fn publish(&self, events: Vec<ActivityEvent>, dropped: u64) {
        let failure = if events.is_empty() {
            None
        } else {
            self.storage.record(events.clone()).err()
        };

        let mut live = self.live();
        live.dropped += dropped;
        for event in events {
            live.total_byte_delta += event.byte_delta.unwrap_or(0);
            live.event_count += 1;
            live.rows.push_front(event);
            if live.rows.len() > MAX_ROWS {
                live.rows.pop_back();
            }
        }
        if let Some(failure) = failure {
            live.error = Some(format!("Could not write to the journal: {failure}"));
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().expect("session state mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Waits for `condition`, so a slow machine fails by timing out with the
    /// state printed rather than by racing the watcher.
    fn eventually(session: &Session, what: &str, condition: impl Fn(&Live) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            {
                let live = session.live();
                if condition(&live) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; live state: {live:#?}"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn watch(root: &std::path::Path, storage: &std::path::Path) -> Session {
        let root = fs::canonicalize(root).unwrap();
        Session::start(
            &pathlight_core::paths::normalize(&root.to_string_lossy()),
            Storage::at(storage),
        )
        .unwrap()
    }

    /// The whole pipeline on a real filesystem: kernel event, measurement,
    /// attribution, journal. If this passes, the shell records; if it fails,
    /// nothing else about the shell matters.
    #[test]
    fn a_file_written_in_a_watched_folder_is_counted_and_journalled() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let session = watch(root.path(), storage_dir.path());

        fs::write(root.path().join("report.bin"), vec![7u8; 64 * 1024]).unwrap();

        eventually(&session, "the write to be recorded", |live| {
            live.event_count > 0
        });
        let live = session.live();
        assert!(
            live.rows.iter().any(|row| row.path.ends_with("report.bin")),
            "rows: {:#?}",
            live.rows
        );
        assert!(
            live.total_byte_delta > 0,
            "64 KiB arrived but the total is {}",
            live.total_byte_delta
        );
        drop(live);

        let journal = fs::read_to_string(Storage::at(storage_dir.path()).journal()).unwrap();
        assert!(journal.contains("report.bin"), "journal was {journal:?}");
    }

    /// The feedback loop, with the storage deliberately placed inside the
    /// watched tree: recording a journal write appends a row, whose write is
    /// another event. The ordinary file proves the watch was alive the whole
    /// time, so a silent watcher cannot pass this test.
    #[test]
    fn a_journal_write_inside_the_watched_tree_is_never_recorded() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = root.path().join("Pathlight");
        fs::create_dir_all(&storage_dir).unwrap();
        let session = watch(root.path(), &storage_dir);

        fs::write(storage_dir.join("activity-events.jsonl"), b"a row\n").unwrap();
        fs::write(root.path().join("ordinary.txt"), b"hello").unwrap();

        eventually(&session, "the ordinary file to be recorded", |live| {
            live.rows
                .iter()
                .any(|row| row.path.ends_with("ordinary.txt"))
        });
        let live = session.live();
        assert!(
            !live.rows.iter().any(|row| row.path.contains("Pathlight")),
            "Pathlight recorded its own storage: {:#?}",
            live.rows
        );
    }

    /// The noise the shipped patterns exist for. Written in the same batch as
    /// an ordinary file, so a filter that dropped everything cannot pass:
    /// `keep.txt` has to arrive, and the other two must not.
    #[test]
    fn the_noise_the_default_patterns_name_is_not_recorded() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let session = watch(root.path(), storage_dir.path());

        fs::write(root.path().join(".DS_Store"), b"finder").unwrap();
        fs::write(root.path().join("movie.mp4.crdownload"), b"half").unwrap();
        fs::write(root.path().join("keep.txt"), b"hello").unwrap();

        eventually(&session, "the ordinary file to be recorded", |live| {
            live.rows.iter().any(|row| row.path.ends_with("keep.txt"))
        });
        let live = session.live();
        let noise: Vec<&str> = live
            .rows
            .iter()
            .map(|row| row.path.as_str())
            .filter(|path| path.contains(".DS_Store") || path.contains(".crdownload"))
            .collect();
        assert!(noise.is_empty(), "recorded excluded paths: {noise:?}");
    }

    /// The retention wiring, not the retention rule — `core/tests/journal.rs`
    /// owns the rule. What this proves is that a watch actually asks: a
    /// journal left over from months ago comes back under the limit without
    /// the user doing anything.
    #[test]
    fn starting_a_watch_trims_a_journal_that_kept_rows_too_long() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let stale = pathlight_core::ActivityEvent {
            kind: pathlight_core::EventKind::Modified,
            path: "/elsewhere/ancient.bin".to_owned(),
            root_path: "/elsewhere".to_owned(),
            timestamp: SystemTime::now() - Duration::from_secs(400 * 86_400),
            byte_delta: Some(1),
            confidence: pathlight_core::Confidence::Confirmed,
            previous_path: None,
            affected_item_count: 1,
            process_name: None,
        };
        storage.record(vec![stale]).unwrap();

        let session = watch(root.path(), storage_dir.path());
        fs::write(root.path().join("now.txt"), b"hello").unwrap();
        eventually(&session, "the new write to be recorded", |live| {
            live.event_count > 0
        });

        let journal = fs::read_to_string(storage.journal()).unwrap();
        assert!(
            !journal.contains("ancient.bin"),
            "the stale row survived: {journal}"
        );
        assert!(journal.contains("now.txt"), "journal was {journal:?}");
    }

    /// The gap path, driven directly: a baseline, then changes the watcher
    /// never reported, then the reconciliation. What the kernel does under
    /// load cannot be provoked from a test, so the seam is called by hand —
    /// but everything under it is the real scan, the real comparison and the
    /// real journal.
    #[test]
    fn a_gap_becomes_a_list_of_what_actually_changed() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        fs::write(root.join("gone.bin"), vec![0u8; 8 * 1024]).unwrap();
        let scope = paths::normalize(&root.to_string_lossy());
        let worker = Worker {
            exclusions: ExclusionFilter::new(DEFAULT_PATTERNS, &scope).unwrap(),
            scope,
            storage: Storage::at(storage_dir.path()),
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(0)),
        };

        let baseline: Baseline = Arc::new(Mutex::new(baseline_of(&worker.scope, &worker.live)));
        assert!(
            baseline.lock().unwrap().is_some(),
            "no baseline: {:?}",
            worker.live().error
        );

        fs::write(root.join("arrived.bin"), vec![7u8; 16 * 1024]).unwrap();
        fs::write(root.join(".DS_Store"), b"finder").unwrap();
        fs::remove_file(root.join("gone.bin")).unwrap();
        worker.reconcile(&baseline);

        let live = worker.live();
        let recovered: Vec<(&str, Option<i64>)> = live
            .rows
            .iter()
            .map(|row| (row.path.as_str(), row.byte_delta))
            .collect();
        assert_eq!(live.gaps, 1);
        assert_eq!(live.recovered, 2, "recovered: {recovered:?}");
        let arrived = live
            .rows
            .iter()
            .find(|row| row.path.ends_with("arrived.bin"))
            .unwrap_or_else(|| panic!("the new file was not recovered: {recovered:?}"));
        assert_eq!(arrived.kind, EventKind::Created);
        assert!(arrived.byte_delta.unwrap_or(0) > 0);
        // A comparison is not an observation, whatever the sizes say.
        assert_eq!(arrived.confidence, Confidence::Estimated);
        let gone = live
            .rows
            .iter()
            .find(|row| row.path.ends_with("gone.bin"))
            .unwrap_or_else(|| panic!("the deletion was not recovered: {recovered:?}"));
        assert_eq!(gone.kind, EventKind::Deleted);
        assert!(gone.byte_delta.unwrap_or(0) < 0);
        assert!(
            !recovered.iter().any(|(path, _)| path.contains(".DS_Store")),
            "excluded noise was recovered too: {recovered:?}"
        );
        drop(live);

        // The next gap compares against what the last one found, not against
        // a baseline from before it.
        fs::write(root.join("later.bin"), b"more").unwrap();
        worker.reconcile(&baseline);
        let live = worker.live();
        assert_eq!(live.recovered, 3, "rows: {:#?}", live.rows);

        let journal = fs::read_to_string(Storage::at(storage_dir.path()).journal()).unwrap();
        assert!(journal.contains("arrived.bin"), "journal was {journal:?}");
        assert!(journal.contains("later.bin"), "journal was {journal:?}");
    }
}
