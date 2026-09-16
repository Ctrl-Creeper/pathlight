//! One live watch: kernel events in, attributed rows out, journal on disk.
//!
//! Shared by every host that is not the macOS app — the egui window and the
//! command line both open watches through this, so a folder watched from a
//! terminal is recorded exactly the way the window would record it.
//!
//! The watcher calls back on its own thread and must not be made to wait, so
//! callbacks only enqueue. A worker thread does the measuring, the attribution
//! and the file I/O, and the UI thread only ever reads a snapshot behind a
//! mutex. Nothing on the paint path touches the disk.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::attribution::{allocated_size, AggregationOptions, Attributor, SizeIndex};
use crate::baseline;
use crate::exclusion::ExclusionFilter;
use crate::monitor::{
    bounded_monitor_latency_ms, ActivityListener, Change, ChangeKind, StreamEvent, Watcher,
};
use crate::snapshot::{self, BindingChange, BindingChangeKind, IdentityContinuity, ScanSnapshot};
use crate::{paths, ActivityEvent, Confidence, EventKind};

use crate::anomaly::{anomalies, growth, Anomaly, AnomalyKind, GROWTH_WINDOW, WINDOW};
use crate::store::Storage;

/// How long a batch waits before it is attributed and written.
#[cfg(any(target_os = "macos", test))]
const MACOS_WORKER_FLUSH: Duration = Duration::from_millis(50);
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

/// Where a finding goes: a desktop notification from the window, a line on
/// stderr from the command line. Which findings are worth saying is
/// [`crate::anomaly`]'s decision; saying them is the host's.
type Announcer = Box<dyn Fn(&Anomaly, &str) + Send + Sync>;

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
    watcher: Option<Arc<Watcher>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    pub fn start(
        root: &str,
        storage: Storage,
        announce: impl Fn(&Anomaly, &str) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let options = storage.options();
        let latency_ms = storage.latency_ms();
        Self::start_configured(root, storage, options, latency_ms, announce)
    }

    /// Opens one watch with per-run recording choices. The store still owns
    /// exclusions, alerts, journals and pause state; only the two startup
    /// controls are overridden.
    pub fn start_configured(
        root: &str,
        storage: Storage,
        options: AggregationOptions,
        latency_ms: u64,
        announce: impl Fn(&Anomaly, &str) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let latency_ms = bounded_monitor_latency_ms(latency_ms);
        // The pause is checked here rather than in each host: a host that
        // forgot would record through a pause the user asked for, and there is
        // no message for that afterwards.
        if storage.paused() {
            return Err(crate::text::PAUSED.to_owned());
        }
        // Built before the watch opens: a filter that failed to compile after
        // events started arriving would record the noise it exists to drop.
        let exclusions =
            ExclusionFilter::new(&storage.patterns(), root).map_err(|error| error.to_string())?;
        let storage_threshold = storage.growth_alert_bytes();
        let now = SystemTime::now();
        let day_start = utc_day_start(now);
        // Only when the user asked to be told: this reads, decrypts and parses
        // the whole journal, which the cap allows to be a gigabyte. Paying that
        // on every watch open for an alert nobody switched on is the opposite
        // of a monitor that stays cheap.
        let initial_growth = match storage_threshold > 0 {
            false => 0,
            true => storage
                .positive_growth_since(root, day_start)
                .unwrap_or_else(|error| {
                    storage.note(&format!(
                        "Could not restore today's growth total for {root}: {error}"
                    ));
                    0
                }),
        };
        let records_epoch = storage.records_epoch();
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let live = Arc::new(Mutex::new(Live::default()));
        let watcher = Watcher::start(
            root.to_owned(),
            None,
            latency_ms,
            Arc::new(QueueListener {
                sender,
                dropped: dropped.clone(),
            }),
        )
        .map_err(|error| error.to_string())?;

        storage.note(&format!(
            "watch opened on {root} · flushing every {latency_ms} ms · this watcher {}",
            crate::text::guarantees(&crate::monitor::watcher_capabilities())
        ));
        let worker = Worker {
            scope: root.to_owned(),
            storage,
            options,
            exclusions,
            live: live.clone(),
            dropped,
            records_epoch,
            alerts: Mutex::new(Alerts::watching(storage_threshold, initial_growth, now)),
            announce: Box::new(announce),
            flush_interval: worker_flush_interval(latency_ms),
        };
        let worker = std::thread::Builder::new()
            .name("pathlight-session".into())
            .spawn(move || worker.run(receiver))
            .map_err(|error| error.to_string())?;

        Ok(Self {
            live,
            watcher: Some(watcher),
            worker: Some(worker),
        })
    }

    /// Reads the live state. Held for as long as one frame's drawing takes.
    pub fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().expect("session state mutex poisoned")
    }

    pub fn stop(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
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
///
/// On disk rather than resident: a whole-disk baseline is the largest thing a
/// background watch would hold, and it is read at exactly two moments — a gap,
/// and the first flush after a restart, when the folder is compared against
/// what the last run left behind.
#[derive(Default)]
struct BaselineState {
    /// Where the last known state is, when there is one.
    kept: Option<PathBuf>,
    /// What was recorded live since `kept` was written: the path's allocated
    /// size now, or `None` for a path that is gone, with when it was seen.
    /// Applied before a comparison so a change already in the journal is not
    /// recovered a second time as a guess.
    overlay: BTreeMap<String, (SystemTime, Option<u64>)>,
    /// A fresh scan, waiting to be compared. Owed to the worker rather than
    /// done on the thread that scanned, because only the worker writes rows.
    catching_up: Option<ScanSnapshot>,
    /// The thread walking the folder, while one is. A second gap during a
    /// walk gets the same walk: the comparison at its end covers both.
    scan: Option<JoinHandle<()>>,
}

type Baseline = Arc<Mutex<BaselineState>>;

/// The overlay is folded into the file on disk at this size rather than kept
/// growing until the next gap, which on a busy disk may be weeks away.
///
/// ponytail: one read and one write of the baseline per this many changes,
/// which is far cheaper than a scan. Raise it if the fold shows up.
const OVERLAY_FOLD: usize = 65_536;

fn keep(
    snapshot: ScanSnapshot,
    scope: &str,
    live: &Arc<Mutex<Live>>,
    storage: &Storage,
) -> Option<PathBuf> {
    let path = baseline::file(storage.dir(), scope);
    match baseline::write(&path, &snapshot) {
        Ok(()) => Some(path),
        Err(error) => {
            trouble(
                live,
                storage,
                format!(
                    "Could not remember what {scope} holds, so a dropped event cannot be \
                     recovered here: {error}"
                ),
            );
            None
        }
    }
}

/// Pathlight's own last-known state is not an amendment: only the entries
/// that were changed after `since` are.
fn amendments(
    overlay: &BTreeMap<String, (SystemTime, Option<u64>)>,
    since: Option<SystemTime>,
) -> BTreeMap<PathBuf, Option<u64>> {
    overlay
        .iter()
        .filter(|(_, (seen, _))| since.is_none_or(|since| *seen >= since))
        .map(|(path, (_, size))| (PathBuf::from(path), *size))
        .collect()
}

/// What the folder holds now. A folder that cannot be read has no baseline,
/// which is said out loud: silently having none would turn every later gap
/// into a gap nobody can reconcile.
fn baseline_of(scope: &str, live: &Arc<Mutex<Live>>, storage: &Storage) -> Option<ScanSnapshot> {
    match snapshot::scan(Path::new(scope)) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            trouble(
                live,
                storage,
                format!(
                    "Could not read {scope} to know what it holds, so a dropped event cannot \
                     be recovered here: {error}"
                ),
            );
            None
        }
    }
}

/// One thing that went wrong, said to whoever is looking now and written down
/// for whoever looks later.
///
/// Both, because a watch runs for months with the window closed: an error that
/// only ever reached a screen nobody was in front of is an error nobody can be
/// told about afterwards.
fn trouble(live: &Arc<Mutex<Live>>, storage: &Storage, message: String) {
    storage.note(&message);
    live.lock().unwrap_or_else(PoisonError::into_inner).error = Some(message);
}

struct Worker {
    scope: String,
    storage: Storage,
    /// What is recorded and how it is folded together, including the sizes of
    /// file this watch records at all.
    options: AggregationOptions,
    /// The noise nobody asked to be told about: `.DS_Store`, trashes,
    /// caches, half-finished downloads — the shipped list until the user
    /// edits it.
    // ponytail: one list for the install, not one per watch. macOS keeps a
    // list per target because a preset ships its own; here a second list per
    // folder is a shape nobody has asked for yet.
    exclusions: Option<ExclusionFilter>,
    live: Arc<Mutex<Live>>,
    dropped: Arc<AtomicU64>,
    records_epoch: u64,
    /// What the user has already been told, so a background watch can speak
    /// up without becoming noise.
    alerts: Mutex<Alerts>,
    announce: Announcer,
    flush_interval: Duration,
}

fn worker_flush_interval(latency_ms: u64) -> Duration {
    #[cfg(target_os = "macos")]
    {
        let _ = latency_ms;
        MACOS_WORKER_FLUSH
    }
    #[cfg(not(target_os = "macos"))]
    {
        Duration::from_millis(bounded_monitor_latency_ms(latency_ms))
    }
}

impl Worker {
    fn run(mut self, receiver: mpsc::Receiver<StreamEvent>) {
        // Loaded from disk, so a watch reopened tomorrow measures deltas
        // against yesterday's sizes instead of calling every first change a
        // whole file and every deletion nothing at all.
        let index = self.storage.size_index();
        let size_scope = self.scope.clone();
        let prior_scope = self.scope.clone();
        let known_scope = self.scope.clone();
        // The size provider records what it measured, so a later deletion of
        // the same path still has a size to report.
        let size = |path: &str| {
            let size = allocated_size(Path::new(path));
            index.record(&size_scope, path, size);
            size
        };
        let prior_size = |path: &str| index.take(&prior_scope, path);
        let known_size = |path: &str| index.peek(&known_scope, path);
        let attributor = Attributor::new(self.options, &size, &prior_size, &known_size);
        let mut pending: Vec<Change> = Vec::new();
        // What the folder looked like when the watch opened. Without it a gap
        // can only ever be counted; with it the gap becomes a list of files.
        //
        // Taken on its own thread, because a whole-disk baseline can take
        // minutes and a worker busy walking a tree is a worker not draining
        // the queue — the events that would drop while it walked are exactly
        // the ones the baseline exists to recover. A gap that arrives before
        // it lands finds nothing to compare against, and says so.
        //
        // A baseline the last run left behind is not thrown away: the folder
        // changed while Pathlight was closed, and that is a gap like any
        // other. The scan below is what it gets compared against.
        let earlier = baseline::file(self.storage.dir(), &self.scope);
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: earlier.exists().then_some(earlier),
            ..BaselineState::default()
        }));
        if baseline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .kept
            .is_some()
        {
            self.live().gaps += 1;
        }
        self.rescan(&baseline);
        let mut due = Instant::now() + self.flush_interval;
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
                    self.flush(&attributor, &mut pending, &index, &baseline);
                    self.storage
                        .note(&format!("watch closed on {}", self.scope));
                    return;
                }
            }
            let now = Instant::now();
            if now >= due {
                self.catch_up(&baseline);
                self.flush(&attributor, &mut pending, &index, &baseline);
                due = now + self.flush_interval;
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
        self.rescan(baseline);
    }

    /// Walk the folder on its own thread and leave the result for the next
    /// flush to compare. Never on the worker: a whole-disk walk takes minutes,
    /// and a worker not draining its queue for minutes overflows it, which is
    /// another gap, which is another walk — a watch that never catches up.
    fn rescan(&self, baseline: &Baseline) {
        let mut slot = baseline.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.scan.as_ref().is_some_and(|scan| !scan.is_finished()) {
            return;
        }
        let shared = baseline.clone();
        let scope = self.scope.clone();
        let live = self.live.clone();
        let storage = self.storage.clone();
        let spawned = std::thread::Builder::new()
            .name("pathlight-baseline".into())
            .spawn(move || {
                let Some(taken) = baseline_of(&scope, &live, &storage) else {
                    return;
                };
                shared
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .catching_up = Some(taken);
            });
        match spawned {
            Ok(handle) => slot.scan = Some(handle),
            Err(error) => self.trouble(format!("Could not take a baseline: {error}")),
        }
    }

    /// Start over with no last-known state: the records it described are gone.
    fn forget_baseline(&self, baseline: &Baseline) {
        {
            let mut slot = baseline.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(path) = slot.kept.take() {
                let _ = std::fs::remove_file(path);
            }
            slot.overlay.clear();
            slot.catching_up = None;
        }
        self.rescan(baseline);
    }

    /// The scan the baseline thread finished, against the last known state:
    /// what changed while nobody was looking — a dropped batch, or the whole
    /// time Pathlight was closed. Rows first, then the new baseline: a
    /// baseline nobody can compare against is worth less than a row nobody
    /// has a baseline for.
    fn catch_up(&self, baseline: &Baseline) {
        let mut slot = baseline.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(mut current) = slot.catching_up.take() else {
            self.fold_overlay(&mut slot);
            return;
        };
        let previous = slot
            .kept
            .as_deref()
            .and_then(|path| baseline::read(path, &current.root));
        let Some(mut previous) = previous else {
            current.amend(amendments(&slot.overlay, Some(current.started_at)));
            slot.overlay.clear();
            slot.kept = keep(current, &self.scope, &self.live, &self.storage);
            return;
        };
        // Everything recorded live is already in the journal, so it goes
        // into what the folder was thought to hold. What arrived during the
        // walk goes into both sides: the walk may or may not have seen it,
        // and either way it is not the gap's to report.
        previous.amend(amendments(&slot.overlay, None));
        current.amend(amendments(&slot.overlay, Some(current.started_at)));
        slot.overlay.clear();
        let comparison = snapshot::reconcile(&previous, &current, IdentityContinuity::Unknown);
        let events: Vec<ActivityEvent> = comparison
            .bindings
            .iter()
            .filter_map(|binding| self.recovered_row(binding, &previous, &current))
            .collect();
        let unread = previous.errors.len().max(current.errors.len());
        drop(previous);
        slot.kept = keep(current, &self.scope, &self.live, &self.storage);
        drop(slot);

        let recovered = events.len() as u64;
        self.publish(events, 0);
        self.storage.note(&format!(
            "caught up on {}: {recovered} row(s) recovered from a gap",
            self.scope
        ));
        self.live().recovered += recovered;
        if unread > 0 {
            self.trouble(format!(
                "{unread} folder(s) here could not be read while catching up, so changes \
                 inside them were not recovered."
            ));
        }
    }

    /// Between gaps the overlay only grows. Past a point it is cheaper to
    /// write it into the baseline than to carry it.
    fn fold_overlay(&self, slot: &mut BaselineState) {
        if slot.overlay.len() < OVERLAY_FOLD {
            return;
        }
        let Some(mut kept) = slot
            .kept
            .as_deref()
            .and_then(|path| baseline::read(path, Path::new(&self.scope)))
        else {
            return;
        };
        kept.amend(amendments(&slot.overlay, None));
        slot.overlay.clear();
        slot.kept = keep(kept, &self.scope, &self.live, &self.storage);
    }

    /// What a batch of live changes says the folder now holds, noted so the
    /// next comparison does not report it again.
    fn overlay(&self, changes: &[Change], index: &SizeIndex, baseline: &Baseline) {
        if changes.is_empty() {
            return;
        }
        let mut slot = baseline.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = |path: &str| {
            index
                .peek(&self.scope, path)
                .map(|size| u64::try_from(size).unwrap_or(0))
        };
        for change in changes {
            let at = change.timestamp;
            match &change.kind {
                ChangeKind::Created | ChangeKind::Modified => {
                    // A size that could not be read leaves the last known one
                    // in place rather than inventing zero.
                    if let Some(size) = seen(&change.path) {
                        slot.overlay.insert(change.path.clone(), (at, Some(size)));
                    }
                }
                ChangeKind::Deleted => {
                    slot.overlay.insert(change.path.clone(), (at, None));
                }
                ChangeKind::Renamed { previous_path } => {
                    if let Some(from) = previous_path {
                        slot.overlay.insert(from.clone(), (at, None));
                    }
                    slot.overlay
                        .insert(change.path.clone(), (at, seen(&change.path)));
                }
            }
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
                .get(at)
                .and_then(|entry| entry.allocated_bytes)
                .map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX))
        };
        let gained = allocated(current, &binding.path);
        let lost = allocated(
            previous,
            binding.previous_path.as_deref().unwrap_or(&binding.path),
        );
        // The same rules a live change is judged by: the size bounds are on
        // the file, the threshold on the change. A gap is not a reason to
        // record what the user asked not to have recorded.
        if !self.options.watches_file(gained.or(lost)) {
            return None;
        }
        let byte_delta = match (gained, lost) {
            (None, None) => None,
            (gained, lost) => Some(gained.unwrap_or(0) - lost.unwrap_or(0)),
        };
        if byte_delta
            .is_some_and(|delta| delta.saturating_abs() < self.options.minimum_recorded_byte_delta)
        {
            return None;
        }
        Some(ActivityEvent {
            kind: match binding.kind {
                BindingChangeKind::Created | BindingChangeKind::HardLinkAdded => EventKind::Created,
                BindingChangeKind::Removed | BindingChangeKind::HardLinkRemoved => {
                    EventKind::Deleted
                }
                BindingChangeKind::Replaced | BindingChangeKind::Modified => EventKind::Modified,
                BindingChangeKind::Renamed => EventKind::Moved,
            },
            path,
            root_path: self.scope.clone(),
            // Now, not when it happened: the whole point of a gap is that
            // nobody knows when inside it anything happened.
            timestamp: SystemTime::now(),
            byte_delta,
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
            self.trouble(format!("Could not trim the journal: {error}"));
        }
    }

    /// See [`trouble`]: a Worker always has both halves to hand.
    fn trouble(&self, message: String) {
        trouble(&self.live, &self.storage, message);
    }

    fn flush(
        &mut self,
        attributor: &Attributor<'_>,
        pending: &mut Vec<Change>,
        index: &SizeIndex,
        baseline: &Baseline,
    ) {
        let mut dropped = self.dropped.swap(0, Ordering::Relaxed);
        if pending.is_empty() && dropped == 0 {
            return;
        }
        let (records_epoch, reset_at) = self.storage.records_generation();
        if records_epoch != self.records_epoch {
            pending.retain(|change| change.timestamp >= reset_at);
            dropped = 0;
            index.reset();
            self.forget_baseline(baseline);
            self.records_epoch = records_epoch;
        }
        let events = attributor.process(pending);
        // After attribution, which is what measured the sizes it reads.
        self.overlay(pending, index, baseline);
        pending.clear();
        if !self.publish(events, dropped) {
            index.reset();
            self.forget_baseline(baseline);
            self.records_epoch = self.storage.records_epoch();
            return;
        }
        if dropped > 0 {
            self.storage.note(&format!(
                "watch queue overflow on {}: {dropped} observation(s) dropped; reconciling",
                self.scope
            ));
            self.reconcile(baseline);
        }
        // After the rows, because a baseline nobody can compare against is
        // worth less than a row nobody has a baseline for.
        match self
            .storage
            .persist_index_if_generation(index, self.records_epoch)
        {
            Ok(true) => {}
            Ok(false) => {
                index.reset();
                self.forget_baseline(baseline);
                self.records_epoch = self.storage.records_epoch();
            }
            Err(error) => self.trouble(format!("Sizes could not be remembered: {error}")),
        }
    }

    /// Journal first, then the screen. A row on screen that was never written
    /// is the difference between a record and a display nobody can get back.
    fn publish(&self, events: Vec<ActivityEvent>, dropped: u64) -> bool {
        let failure = if events.is_empty() {
            None
        } else {
            match self
                .storage
                .record_if_generation(events.clone(), self.records_epoch)
            {
                Ok(true) => None,
                Ok(false) => return false,
                Err(error) => Some(error),
            }
        };

        let mut live = self.live();
        live.dropped += dropped;
        let mut arrived = 0i64;
        for event in events {
            live.total_byte_delta += event.byte_delta.unwrap_or(0);
            arrived = arrived.saturating_add(event.byte_delta.unwrap_or(0).max(0));
            live.event_count += 1;
            live.rows.push_front(event);
            if live.rows.len() > MAX_ROWS {
                live.rows.pop_back();
            }
        }
        let journal_failure =
            failure.map(|failure| format!("Could not write to the journal: {failure}"));
        if let Some(message) = &journal_failure {
            live.error = Some(message.clone());
        }

        // While the window is closed nothing repaints, so this has to happen
        // here on the worker rather than on the paint path: a watch that only
        // notices anomalies when somebody is looking is a watch that notices
        // nothing worth notifying about.
        let news = self
            .alerts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .news(&live.rows, arrived, SystemTime::now());
        drop(live);
        // After the lock, because writing to the diary is a disk write and
        // the lock is held on the path that also draws.
        if let Some(message) = &journal_failure {
            self.storage.note(message);
        }
        for alert in news {
            self.storage
                .note(&crate::text::alert_title(&alert, &self.scope));
            (self.announce)(&alert, &self.scope);
        }
        true
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().expect("session state mutex poisoned")
    }
}

/// Remembers what has already been said, so one finding is one notification.
#[derive(Debug, Default)]
struct Alerts {
    /// When each kind was last raised. A folder being emptied stays a folder
    /// being emptied for as long as it takes; saying so every flush would
    /// teach the user to ignore it.
    said: [Option<SystemTime>; 3],
    /// How much this folder may gain in a day before the user is told, or
    /// zero for never.
    threshold: i64,
    /// The day being added up, and what has arrived inside it.
    day: Option<(SystemTime, i64)>,
}

impl Alerts {
    /// Alerts that also watch for a folder gaining `threshold` bytes in a day.
    fn watching(threshold: i64, initial_growth: i64, now: SystemTime) -> Self {
        Self {
            threshold,
            day: Some((utc_day_start(now), initial_growth.max(0))),
            ..Self::default()
        }
    }

    /// What is worth saying about these rows right now, given that `arrived`
    /// bytes of them are new.
    ///
    /// `rows` is newest first and bounded, so a long enough burst is measured
    /// from what was kept: the thresholds are floors, and a floor that is
    /// crossed is still crossed.
    fn news(
        &mut self,
        rows: &VecDeque<ActivityEvent>,
        arrived: i64,
        now: SystemTime,
    ) -> Vec<Anomaly> {
        let mut found = anomalies(rows, now);
        found.extend(self.grown(arrived, now));
        found
            .into_iter()
            .filter(|alert| self.once(alert, now))
            .collect()
    }

    /// Adds `arrived` to the day and says whether the day is now past the
    /// threshold. The total resets at the UTC day boundary, matching history
    /// aggregation and the macOS host.
    fn grown(&mut self, arrived: i64, now: SystemTime) -> Option<Anomaly> {
        if self.threshold <= 0 {
            return None;
        }
        let today = utc_day_start(now);
        let (start, gained) = self.day.get_or_insert((today, 0));
        if *start != today {
            *start = today;
            *gained = 0;
        }
        *gained = gained.saturating_add(arrived);
        growth(*gained, self.threshold)
    }

    /// Whether this finding has gone unsaid for a window.
    fn once(&mut self, alert: &Anomaly, now: SystemTime) -> bool {
        // Matched rather than indexed by discriminant, so a finding added to
        // the core's policy fails to compile here instead of silently
        // borrowing another kind's cooldown.
        let (index, cooldown) = match alert.kind {
            AnomalyKind::Removal => (0, WINDOW),
            AnomalyKind::Burst => (1, WINDOW),
            // Once a day, because that is the span the threshold is about.
            AnomalyKind::Growth => (2, GROWTH_WINDOW),
        };
        let slot = &mut self.said[index];
        if slot.is_some_and(|said| now.duration_since(said).unwrap_or_default() < cooldown) {
            return false;
        }
        *slot = Some(now);
        true
    }
}

fn utc_day_start(time: SystemTime) -> SystemTime {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds / 86_400 * 86_400)
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
        let session = Session::start(
            &crate::paths::normalize(&root.to_string_lossy()),
            Storage::at(storage),
            |_, _| {},
        )
        .unwrap();
        // The Windows notify backend arms its recursive handle asynchronously;
        // give it a scheduling turn before the test performs its first write.
        std::thread::sleep(Duration::from_millis(250));
        session
    }

    #[test]
    fn dropping_a_session_waits_for_its_worker_to_close() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let session = watch(root.path(), storage_dir.path());
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(storage_dir.path().join(crate::store::LOCK_FILE))
            .unwrap();
        lock.lock().unwrap();
        let (finished, result) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            drop(session);
            let _ = finished.send(());
        });

        assert!(
            result.recv_timeout(Duration::from_millis(100)).is_err(),
            "Session::drop returned while its worker was still closing"
        );
        fs::File::unlock(&lock).unwrap();
        result
            .recv_timeout(Duration::from_secs(5))
            .expect("Session::drop did not finish after storage unlocked");
        assert!(
            Storage::at(storage_dir.path())
                .log_tail(10)
                .contains("watch closed"),
            "the worker did not finish its close record"
        );
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

    /// The setting the window offers, end to end: a saved bound is read when
    /// the watch opens, and the file under it never reaches the journal. The
    /// large file proves the watch was alive, so a silent watcher cannot pass.
    #[test]
    fn a_watch_skips_the_files_the_saved_bounds_exclude() {
        let root = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        Storage::at(storage_dir.path())
            .set_size_bounds(Some(1_000_000), None)
            .unwrap();
        let session = watch(root.path(), storage_dir.path());

        fs::write(root.path().join("tiny.txt"), b"small").unwrap();
        fs::write(root.path().join("large.bin"), vec![7u8; 2 * 1024 * 1024]).unwrap();

        eventually(&session, "the large file to be recorded", |live| {
            live.rows.iter().any(|row| row.path.ends_with("large.bin"))
        });
        let live = session.live();
        assert!(
            !live.rows.iter().any(|row| row.path.ends_with("tiny.txt")),
            "rows: {:#?}",
            live.rows
        );
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
        fs::write(root.path().join("ordinary.txt"), vec![b'h'; 2 * 1024]).unwrap();

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
        fs::write(root.path().join("keep.txt"), vec![b'h'; 2 * 1024]).unwrap();

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
        let stale = crate::ActivityEvent {
            kind: crate::EventKind::Modified,
            path: "/elsewhere/ancient.bin".to_owned(),
            root_path: "/elsewhere".to_owned(),
            timestamp: SystemTime::now() - Duration::from_secs(400 * 86_400),
            byte_delta: Some(1),
            confidence: crate::Confidence::Confirmed,
            previous_path: None,
            affected_item_count: 1,
            process_name: None,
        };
        storage.record(vec![stale]).unwrap();

        let session = watch(root.path(), storage_dir.path());
        fs::write(root.path().join("now.txt"), vec![b'n'; 2 * 1024]).unwrap();
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

    #[test]
    fn a_watch_reports_changes_on_its_configured_interval() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let scope = paths::normalize(&root.to_string_lossy());
        let live = Arc::new(Mutex::new(Live::default()));
        let worker = Worker {
            exclusions: None,
            scope: scope.clone(),
            storage: storage.clone(),
            options: AggregationOptions {
                minimum_recorded_byte_delta: 0,
                ..AggregationOptions::SHORT_TERM
            },
            live: live.clone(),
            dropped: Arc::new(AtomicU64::new(0)),
            records_epoch: storage.records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: Duration::from_millis(800),
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || worker.run(receiver));
        let path = root.join("paced.txt");
        fs::write(&path, b"paced change").unwrap();
        sender
            .send(StreamEvent::Change {
                change: Change {
                    kind: crate::monitor::ChangeKind::Created,
                    path: paths::normalize(&path.to_string_lossy()),
                    root_path: scope,
                    timestamp: SystemTime::now(),
                    process_name: None,
                },
                event_id: 1,
            })
            .unwrap();

        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            live.lock().unwrap().event_count,
            0,
            "the worker flushed before the configured interval"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while live.lock().unwrap().event_count == 0 {
            assert!(Instant::now() < deadline, "the configured flush never ran");
            std::thread::sleep(Duration::from_millis(20));
        }

        drop(sender);
        worker.join().unwrap();
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
            exclusions: ExclusionFilter::new(crate::exclusion::DEFAULT_PATTERNS, &scope).unwrap(),
            scope,
            storage: Storage::at(storage_dir.path()),
            options: AggregationOptions::SHORT_TERM,
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(0)),
            records_epoch: Storage::at(storage_dir.path()).records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: MACOS_WORKER_FLUSH,
        };

        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: baseline_of(&worker.scope, &worker.live, &worker.storage)
                .and_then(|taken| keep(taken, &worker.scope, &worker.live, &worker.storage)),
            ..BaselineState::default()
        }));
        assert!(
            baseline.lock().unwrap().kept.is_some(),
            "no baseline: {:?}",
            worker.live().error
        );

        fs::write(root.join("arrived.bin"), vec![7u8; 16 * 1024]).unwrap();
        fs::write(root.join(".DS_Store"), b"finder").unwrap();
        fs::remove_file(root.join("gone.bin")).unwrap();
        worker.reconcile(&baseline);
        settle(&baseline);
        worker.catch_up(&baseline);

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
        settle(&baseline);
        worker.catch_up(&baseline);
        let live = worker.live();
        assert_eq!(live.recovered, 3, "rows: {:#?}", live.rows);

        let journal = fs::read_to_string(Storage::at(storage_dir.path()).journal()).unwrap();
        assert!(journal.contains("arrived.bin"), "journal was {journal:?}");
        assert!(journal.contains("later.bin"), "journal was {journal:?}");
    }

    #[test]
    fn a_reset_discards_a_batch_accepted_before_the_reset() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let scope = paths::normalize(&root.to_string_lossy());
        let mut worker = Worker {
            exclusions: None,
            scope: scope.clone(),
            storage: storage.clone(),
            options: AggregationOptions::SHORT_TERM,
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(0)),
            records_epoch: storage.records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: MACOS_WORKER_FLUSH,
        };
        let file = root.join("before-reset.txt");
        fs::write(&file, b"queued").unwrap();
        let mut pending = vec![Change {
            kind: crate::monitor::ChangeKind::Created,
            path: paths::normalize(&file.to_string_lossy()),
            root_path: scope.clone(),
            timestamp: SystemTime::now(),
            process_name: None,
        }];
        let index = SizeIndex::default();
        let size = |path: &str| allocated_size(Path::new(path));
        let prior = |_: &str| None;
        let known = |_: &str| None;
        let attributor = Attributor::new(worker.options, &size, &prior, &known);

        storage.forget_records().unwrap();
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState::default()));
        worker.flush(&attributor, &mut pending, &index, &baseline);

        let journal = fs::read_to_string(storage.journal()).unwrap_or_default();
        assert!(
            !journal.contains("before-reset.txt"),
            "a pre-reset batch recreated deleted history: {journal}"
        );
    }

    #[test]
    fn an_internal_queue_overflow_is_reconciled_and_written_to_the_diary() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let scope = paths::normalize(&root.to_string_lossy());
        let mut worker = Worker {
            exclusions: None,
            scope: scope.clone(),
            storage: storage.clone(),
            options: AggregationOptions::SHORT_TERM,
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(1)),
            records_epoch: storage.records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: MACOS_WORKER_FLUSH,
        };
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: baseline_of(&scope, &worker.live, &storage)
                .and_then(|taken| keep(taken, &scope, &worker.live, &storage)),
            ..BaselineState::default()
        }));
        fs::write(root.join("missed.txt"), b"reconcile me").unwrap();
        let index = SizeIndex::default();
        let size = |path: &str| allocated_size(Path::new(path));
        let prior = |_: &str| None;
        let known = |_: &str| None;
        let attributor = Attributor::new(worker.options, &size, &prior, &known);

        worker.flush(&attributor, &mut Vec::new(), &index, &baseline);
        // The walk is on its own thread; the worker compares at its next flush.
        settle(&baseline);
        worker.catch_up(&baseline);

        let live = worker.live();
        assert_eq!(live.dropped, 1);
        assert_eq!(
            live.gaps, 1,
            "overflow was counted but not treated as a gap"
        );
        assert!(live.rows.iter().any(|row| row.path.ends_with("missed.txt")));
        drop(live);
        assert!(storage.log_tail(20).contains("queue overflow"));
    }

    /// The gap nobody watches: the app was closed, files moved, the app came
    /// back. Without a baseline that outlives the process this is silence.
    #[test]
    fn what_changed_while_pathlight_was_closed_is_recovered_on_the_next_run() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let scope = paths::normalize(&root.to_string_lossy());
        fs::write(root.join("kept.txt"), b"here before").unwrap();
        let worker = Worker {
            exclusions: None,
            scope: scope.clone(),
            storage: storage.clone(),
            options: AggregationOptions::SHORT_TERM,
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(0)),
            records_epoch: storage.records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: MACOS_WORKER_FLUSH,
        };

        // The run that ends: a baseline is left on disk.
        assert!(baseline_of(&scope, &worker.live, &storage)
            .and_then(|taken| keep(taken, &scope, &worker.live, &storage))
            .is_some());
        // Nothing is watching now.
        fs::write(root.join("arrived.txt"), b"while it was closed").unwrap();
        fs::remove_file(root.join("kept.txt")).unwrap();
        // The run that starts: the thread scans, the worker compares.
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: Some(baseline::file(storage.dir(), &scope)),
            catching_up: baseline_of(&scope, &worker.live, &storage),
            ..BaselineState::default()
        }));

        worker.catch_up(&baseline);

        let live = worker.live();
        assert!(
            live.rows
                .iter()
                .any(|row| row.path.ends_with("arrived.txt")),
            "a file that appeared while it was closed went unrecorded: {:?}",
            live.rows
        );
        assert!(
            live.rows.iter().any(|row| row.path.ends_with("kept.txt")),
            "a file that went missing while it was closed went unrecorded: {:?}",
            live.rows
        );
        drop(live);
        // And the folder as it is now is what the next gap compares against.
        let now = baseline::read(&baseline::file(storage.dir(), &scope), &root).unwrap();
        assert!(now.get(&root.join("arrived.txt")).is_some());
        assert!(now.get(&root.join("kept.txt")).is_none());
    }

    /// Wait for the baseline thread, the way the worker's next flush would.
    fn settle(baseline: &Baseline) {
        let scan = baseline.lock().unwrap().scan.take();
        if let Some(scan) = scan {
            scan.join().unwrap();
        }
    }

    fn worker_on(root: &Path, storage: &Storage) -> Worker {
        Worker {
            exclusions: None,
            scope: paths::normalize(&root.to_string_lossy()),
            storage: storage.clone(),
            options: AggregationOptions::SHORT_TERM,
            live: Arc::new(Mutex::new(Live::default())),
            dropped: Arc::new(AtomicU64::new(0)),
            records_epoch: storage.records_epoch(),
            alerts: Mutex::new(Alerts::default()),
            announce: Box::new(|_, _| {}),
            flush_interval: MACOS_WORKER_FLUSH,
        }
    }

    /// The gap's other half: a file that only grew has the same name and the
    /// same inode before and after, and used to be recovered as nothing.
    #[test]
    fn a_file_that_grew_during_a_gap_is_recovered_as_a_modification() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        fs::write(root.join("log.txt"), b"short").unwrap();
        let worker = worker_on(&root, &storage);
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: baseline_of(&worker.scope, &worker.live, &storage)
                .and_then(|taken| keep(taken, &worker.scope, &worker.live, &storage)),
            ..BaselineState::default()
        }));

        fs::write(root.join("log.txt"), vec![b'x'; 1 << 20]).unwrap();
        worker.reconcile(&baseline);
        settle(&baseline);
        worker.catch_up(&baseline);

        let live = worker.live();
        let row = live
            .rows
            .iter()
            .find(|row| row.path.ends_with("log.txt"))
            .expect("the growth went unrecorded");
        assert_eq!(row.kind, EventKind::Modified);
        // Allocated, not logical: the megabyte less the blocks it already had.
        assert!(
            row.byte_delta.is_some_and(|delta| delta > 1_000_000),
            "{row:?}"
        );
    }

    /// What was recorded live is in the journal. Recovering it again after a
    /// gap would count the same bytes twice.
    #[test]
    fn a_change_recorded_live_is_not_recovered_again_after_a_gap() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        fs::write(root.join("doomed.txt"), vec![b'x'; 4096]).unwrap();
        let mut worker = worker_on(&root, &storage);
        let scope = worker.scope.clone();
        let baseline: Baseline = Arc::new(Mutex::new(BaselineState {
            kept: baseline_of(&scope, &worker.live, &storage)
                .and_then(|taken| keep(taken, &scope, &worker.live, &storage)),
            ..BaselineState::default()
        }));
        let index = SizeIndex::default();
        let size = |path: &str| {
            let size = allocated_size(Path::new(path));
            index.record(&scope, path, size);
            size
        };
        let prior = |path: &str| index.take(&scope, path);
        let known = |path: &str| index.peek(&scope, path);
        let attributor = Attributor::new(worker.options, &size, &prior, &known);

        // Seen live: a file arrives, another goes.
        let arrived = root.join("arrived.txt");
        fs::write(&arrived, vec![b'y'; 8192]).unwrap();
        fs::remove_file(root.join("doomed.txt")).unwrap();
        let change = |kind, path: &Path| Change {
            kind,
            path: paths::normalize(&path.to_string_lossy()),
            root_path: scope.clone(),
            timestamp: SystemTime::now(),
            process_name: None,
        };
        let mut pending = vec![
            change(ChangeKind::Created, &arrived),
            change(ChangeKind::Deleted, &root.join("doomed.txt")),
        ];
        worker.flush(&attributor, &mut pending, &index, &baseline);
        let live_rows = worker.live().rows.len();
        assert_eq!(live_rows, 2, "{:?}", worker.live().rows);

        // Then a gap in which nothing else happened.
        worker.reconcile(&baseline);
        settle(&baseline);
        worker.catch_up(&baseline);

        let live = worker.live();
        assert_eq!(
            live.rows.len(),
            live_rows,
            "the gap re-reported what was already recorded: {:?}",
            live.rows
        );
        assert_eq!(live.recovered, 0);
    }

    fn row(kind: EventKind, delta: i64, items: u32, at: SystemTime) -> ActivityEvent {
        ActivityEvent {
            kind,
            path: "/watched/file.bin".to_owned(),
            root_path: "/watched".to_owned(),
            timestamp: at,
            byte_delta: Some(delta),
            confidence: Confidence::Confirmed,
            previous_path: None,
            affected_item_count: items,
            process_name: None,
        }
    }

    /// The part a host does not decide: a folder being emptied stays a folder
    /// being emptied for as long as it takes, and saying so on every flush
    /// would teach the user to ignore it. What counts as emptied is
    /// `core/tests/anomaly.rs`.
    #[test]
    fn a_finding_is_said_once_per_window() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let rows: VecDeque<_> = (0..2)
            .map(|_| row(EventKind::Deleted, -1, 60, now))
            .collect();
        let mut alerts = Alerts::default();

        let news = alerts.news(&rows, 0, now);
        assert_eq!(news.len(), 1, "120 deleted items went unmentioned");
        assert_eq!(news[0].kind, AnomalyKind::Removal);
        assert!(
            alerts
                .news(&rows, 0, now + Duration::from_secs(60))
                .is_empty(),
            "the same finding was raised twice"
        );
        assert_eq!(
            alerts.news(&rows, 0, now + WINDOW).len(),
            1,
            "still happening a window later, and still worth saying"
        );
    }

    /// Nothing to say stays nothing to say, without consuming the window.
    #[test]
    fn a_quiet_folder_is_never_interrupted() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ordinary = VecDeque::from([row(EventKind::Modified, 4096, 1, now)]);
        assert!(Alerts::default().news(&ordinary, 0, now).is_empty());
        // And a threshold nobody set is not a threshold anything crosses.
        assert!(Alerts::watching(0, 0, now)
            .news(&ordinary, 100 * 1_000_000_000, now)
            .is_empty());
    }

    /// The threshold the user set, measured over a day of watching: enough
    /// arriving is said once, and the day it was said in has to pass before
    /// it is said again.
    #[test]
    fn a_folder_past_the_size_the_user_asked_about_says_so_once_a_day() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let quiet = VecDeque::new();
        let mut alerts = Alerts::watching(1_000_000_000, 0, now);

        assert!(alerts.news(&quiet, 600_000_000, now).is_empty());
        let news = alerts.news(&quiet, 600_000_000, now + Duration::from_secs(60));
        assert_eq!(news.len(), 1, "1.2 GB in a day went unmentioned");
        assert_eq!(news[0].kind, AnomalyKind::Growth);
        assert_eq!(news[0].bytes, 1_200_000_000);
        assert!(
            alerts
                .news(&quiet, 600_000_000, now + Duration::from_secs(120))
                .is_empty(),
            "the same day was reported twice"
        );

        // A new day starts the total over: yesterday's gigabytes are not
        // today's growth.
        assert!(alerts
            .news(&quiet, 600_000_000, now + GROWTH_WINDOW)
            .is_empty());
    }

    #[test]
    fn growth_recorded_before_a_restart_counts_toward_todays_alert() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let quiet = VecDeque::new();
        let mut alerts = Alerts::watching(1_000_000_000, 700_000_000, now);

        let news = alerts.news(&quiet, 400_000_000, now + Duration::from_secs(60));

        assert_eq!(news.len(), 1, "the pre-restart growth was forgotten");
        assert_eq!(news[0].kind, AnomalyKind::Growth);
        assert_eq!(news[0].bytes, 1_100_000_000);
    }
}
