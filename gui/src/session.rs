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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use pathlight_core::attribution::{allocated_size, AggregationOptions, Attributor, SizeIndex};
use pathlight_core::monitor::{ActivityListener, Change, StreamEvent, Watcher};
use pathlight_core::{ActivityEvent, Journal};

use crate::store::Storage;

/// The interactive coalescing window. Wide enough that a save is one event
/// rather than five, short enough to feel immediate.
const LATENCY_MS: u64 = 250;
/// How long a batch waits before it is attributed and written.
const FLUSH: Duration = Duration::from_millis(200);
/// Bounded so a burst costs memory it cannot exceed. Overflow is counted and
/// reported, never quietly dropped.
const QUEUE_CAPACITY: usize = 4096;
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

struct Worker {
    scope: String,
    storage: Storage,
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
        let journal = Journal::new(self.storage.journal().to_string_lossy().into_owned());

        let mut pending: Vec<Change> = Vec::new();
        let mut due = Instant::now() + FLUSH;
        loop {
            let wait = due.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(wait) {
                Ok(event) => self.accept(event, &mut pending),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush(&attributor, &journal, &mut pending);
                    return;
                }
            }
            if Instant::now() >= due {
                self.flush(&attributor, &journal, &mut pending);
                due = Instant::now() + FLUSH;
            }
        }
    }

    fn accept(&self, event: StreamEvent, pending: &mut Vec<Change>) {
        match event {
            StreamEvent::Change { change, .. } => {
                if !self.storage.is_own(&change.path) {
                    pending.push(change);
                }
            }
            // Nothing to resume from on a fresh watch, so this is only a
            // marker that the numbers are incomplete from here on.
            StreamEvent::RequiresRescan { .. } => self.live().gaps += 1,
            StreamEvent::HistoryCaughtUp { .. } => {}
        }
    }

    fn flush(&self, attributor: &Attributor<'_>, journal: &Journal, pending: &mut Vec<Change>) {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if pending.is_empty() && dropped == 0 {
            return;
        }
        let events = attributor.process(pending);
        pending.clear();
        let failure = journal.append(events.clone()).err();

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
}
