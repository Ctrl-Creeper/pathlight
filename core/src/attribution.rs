//! Turns raw path changes into `ActivityEvent`s with byte deltas.
//! Port of Swift's `StorageAttributionService` + `FileAllocatedSizeProvider`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::monitor::{Change, ChangeKind};
use crate::{ActivityEvent, Confidence, CoreError, EventKind};

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct AggregationOptions {
    /// Events whose |delta| is below this are dropped (unknown-size events pass).
    pub minimum_recorded_byte_delta: i64,
    /// Seconds; 0 disables grouping into `Aggregate` rows.
    pub aggregation_window_secs: u64,
    /// When true, individual file rows are kept (no aggregation).
    pub records_file_names: bool,
    /// Smallest and largest file this watch records at all, in bytes; `None`
    /// is no bound. Judged on the file's own size, not on how much of it
    /// changed, which is what `minimum_recorded_byte_delta` does.
    pub min_file_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
}

impl AggregationOptions {
    /// Long-term default: 10 MiB threshold, 5-minute aggregation, no file names.
    pub const LONG_TERM: Self = Self {
        minimum_recorded_byte_delta: 10 * 1024 * 1024,
        aggregation_window_secs: 5 * 60,
        records_file_names: false,
        min_file_bytes: None,
        max_file_bytes: None,
    };

    /// Live monitor default: everything, individually.
    pub const SHORT_TERM: Self = Self {
        minimum_recorded_byte_delta: 0,
        aggregation_window_secs: 0,
        records_file_names: true,
        min_file_bytes: None,
        max_file_bytes: None,
    };

    /// Whether a file of this size is watched at all.
    pub fn watches_file(&self, size: Option<i64>) -> bool {
        size_in_bounds(size, self.min_file_bytes, self.max_file_bytes)
    }
}

/// Whether a file of this size is inside the bounds a host set; `None` for a
/// bound is no bound.
///
/// A size that could not be read is inside them. A change whose file cannot be
/// measured — a deletion, usually — must not vanish because of a bound it was
/// never tested against; under-reporting a deletion is the one failure this
/// app cannot afford. Shared with the evidence recorder, so the bounds a user
/// types mean the same thing in the CLI as in either window.
pub fn size_in_bounds(size: Option<i64>, min_bytes: Option<i64>, max_bytes: Option<i64>) -> bool {
    let Some(size) = size else {
        return true;
    };
    if let Some(min) = min_bytes {
        if size < min {
            return false;
        }
    }
    if let Some(max) = max_bytes {
        if size > max {
            return false;
        }
    }
    true
}

/// Allocated on-disk size; directories count as 0 so readable folders are not
/// mistaken for unreadable ones.
pub fn allocated_size(path: &Path) -> Option<i64> {
    crate::measurement::measure_file(path)
        .ok()?
        .allocated_bytes
        .and_then(|bytes| i64::try_from(bytes).ok())
}

/// Remembers the last known size of each path so deletions can be attributed.
///
/// Every entry is namespaced by a `scope`. Two watches over the same file — a
/// whole-disk watch and a folder watch, or a live watch and a background one —
/// are independent observers and each needs its own baseline. Sharing one entry
/// lets whichever watch handles an event first record the new size, so the
/// others measure a delta of zero and their threshold drops the change.
///
/// [`SizeIndex::at`] keeps the baselines between runs. Without that a watch
/// restarted overnight reports the first change to every file as the whole
/// file, and a deletion as no bytes at all — the answer this app exists for.
#[derive(Debug, Default)]
pub struct SizeIndex {
    sizes: Mutex<HashMap<String, i64>>,
    /// Where the baselines outlive the run that measured them, when a host
    /// asked for that. `None` is the memory-only index a test or a one-shot
    /// recording wants.
    file: Option<Persisted>,
}

/// How many lines the file may hold before it is rewritten from what is in
/// memory. An append-only list of baselines is mostly superseded rows; the
/// macOS index compacts at the same count, so one number is one behaviour.
const COMPACT_AT: usize = 10_000;

#[derive(Debug)]
struct Persisted {
    path: PathBuf,
    lock_dir: PathBuf,
    /// The journal whose key seals these lines, when the user asked for
    /// encrypted records: one key for everything Pathlight keeps, so turning
    /// encryption on does not leave every path readable over here.
    key_source: Option<PathBuf>,
    state: Mutex<Pending>,
}

#[derive(Debug, Default)]
struct Pending {
    /// Changes not written yet. Held rather than written per file: attribution
    /// runs once per changed path, and a disk write each time is exactly the
    /// cost a monitor that promises to be cheap must not have.
    lines: Vec<String>,
    /// Lines already in the file, for deciding when to rewrite it.
    written: usize,
}

impl SizeIndex {
    /// An index whose baselines are kept in `path`, loaded from it now.
    ///
    /// `key_source` is the journal whose key the lines are sealed under, or
    /// `None` to keep them as plain json — the same choice the journal itself
    /// offers, because these lines name the same files.
    pub fn at(path: PathBuf, key_source: Option<PathBuf>) -> Self {
        let (sizes, written) = load(&path, key_source.as_deref());
        let lock_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            sizes: Mutex::new(sizes),
            file: Some(Persisted {
                path,
                lock_dir,
                key_source,
                state: Mutex::new(Pending {
                    lines: Vec::new(),
                    written,
                }),
            }),
        }
    }

    /// Records `size` for `path` in `scope` (or forgets it when `None`).
    pub fn record(&self, scope: &str, path: &str, size: Option<i64>) -> Option<i64> {
        let key = key(scope, path);
        let mut sizes = self.sizes.lock().unwrap();
        match size {
            Some(size) => sizes.insert(key.clone(), size),
            None => sizes.remove(&key),
        };
        drop(sizes);
        self.note(key, size);
        size
    }

    pub fn take(&self, scope: &str, path: &str) -> Option<i64> {
        let key = key(scope, path);
        let taken = self.sizes.lock().unwrap().remove(&key);
        if taken.is_some() {
            self.note(key, None);
        }
        taken
    }

    /// The last size recorded for `path` in `scope`, left in place. `take` is
    /// for a path that is gone; a file that was merely modified still needs
    /// its baseline afterwards, or the next change reports the whole file
    /// again instead of what was added to it.
    pub fn peek(&self, scope: &str, path: &str) -> Option<i64> {
        self.sizes.lock().unwrap().get(&key(scope, path)).copied()
    }

    pub(crate) fn reset(&self) {
        self.sizes.lock().unwrap().clear();
        if let Some(file) = &self.file {
            let mut state = file.state.lock().unwrap();
            state.lines.clear();
            state.written = 0;
        }
    }

    /// Puts what changed since the last call on disk, rewriting the file once
    /// it is mostly superseded rows. A memory-only index does nothing.
    ///
    /// Called where the journal is written rather than per file, so one flush
    /// is one append. A line that fails to reach the disk costs one baseline
    /// after the next restart, which is why this is not fatal to a watch: the
    /// file is a cache of measurements, not a record of what happened.
    pub fn persist(&self) -> Result<(), CoreError> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let _storage_lock = crate::store::lock_directory(&file.lock_dir)?;
        self.persist_while_locked()
    }

    /// Persists while the caller holds the storage directory transaction.
    /// Used by `Storage` to make a records-generation check and this write
    /// indivisible across processes.
    pub(crate) fn persist_while_locked(&self) -> Result<(), CoreError> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        // The storage transaction is always outermost; attribution releases
        // its in-memory locks before asking Storage to persist.
        let (lines, compacting) = {
            let mut state = file.state.lock().unwrap();
            if state.lines.is_empty() {
                return Ok(());
            }
            let compacting = state.written + state.lines.len() >= COMPACT_AT;
            (std::mem::take(&mut state.lines), compacting)
        };
        let key = match &file.key_source {
            Some(journal) => Some(crate::crypt::key_or_create(journal)?),
            None => None,
        };
        let lines = match compacting {
            true => {
                // Another host may have appended since this instance loaded.
                // Rebase local pending operations onto the locked on-disk
                // state instead of publishing a stale in-memory snapshot.
                let (mut merged, _) = load(&file.path, file.key_source.as_deref());
                apply(&mut merged, &lines);
                let mut lines: Vec<String> = merged
                    .into_iter()
                    .filter_map(|(path, size)| line(path, Some(size)))
                    .collect();
                lines.sort();
                lines
            }
            false => lines,
        };
        write(&file.path, &lines, key.as_ref(), !compacting)?;
        let mut state = file.state.lock().unwrap();
        state.written = match compacting {
            true => lines.len(),
            false => state.written + lines.len(),
        };
        Ok(())
    }

    fn note(&self, key: String, size: Option<i64>) {
        let Some(file) = &self.file else {
            return;
        };
        if let Some(line) = line(key, size) {
            file.state.lock().unwrap().lines.push(line);
        }
    }
}

/// A newline cannot appear in a scope, so no scope can spell another one's key.
fn key(scope: &str, path: &str) -> String {
    format!("{scope}\n{path}")
}

/// One line of the file: the shape the macOS app writes
/// (`ActivitySizeIndex.JournalEntry`), so a mac running both hosts keeps one
/// index rather than two that disagree.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    kind: Kind,
    /// The namespaced key, which is what the app stores here too.
    path: String,
    size: Option<i64>,
    /// Written because the app's decoder requires it. Nothing here reads it
    /// back: for one path the last line wins either way.
    recorded_at: Option<String>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Record,
    Remove,
}

/// The json for one baseline, or `None` when it cannot be spelled — a line
/// that would not parse back is worse than a line that was never written.
fn line(path: String, size: Option<i64>) -> Option<String> {
    serde_json::to_string(&Entry {
        kind: match size {
            Some(_) => Kind::Record,
            None => Kind::Remove,
        },
        path,
        size,
        recorded_at: crate::event::swift_date::text(SystemTime::now()),
    })
    .ok()
}

/// The baselines in `path`, and how many lines they took, or nothing when
/// there is no file yet.
fn load(path: &Path, key_source: Option<&Path>) -> (HashMap<String, i64>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (HashMap::new(), 0);
    };
    // Only fetched when a line is actually encrypted: reading a plaintext
    // file must not create a key, let alone open a keychain.
    let key = key_source
        .filter(|_| {
            text.lines()
                .any(|line| line.starts_with(crate::crypt::PREFIX))
        })
        .and_then(crate::crypt::key);
    let mut sizes = HashMap::new();
    let mut written = 0;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some(entry) = crate::journal::readable(line, key.as_ref())
            .and_then(|json| serde_json::from_str::<Entry>(&json).ok())
        else {
            continue;
        };
        written += 1;
        match entry.size.filter(|_| entry.kind == Kind::Record) {
            Some(size) => sizes.insert(entry.path, size),
            None => sizes.remove(&entry.path),
        };
    }
    (sizes, written)
}

fn apply(sizes: &mut HashMap<String, i64>, lines: &[String]) {
    for line in lines {
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        match entry.size.filter(|_| entry.kind == Kind::Record) {
            Some(size) => {
                sizes.insert(entry.path, size);
            }
            None => {
                sizes.remove(&entry.path);
            }
        }
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn compaction_rebases_onto_entries_written_by_another_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity-size-index.jsonl");
        let first = SizeIndex::at(path.clone(), None);
        let second = SizeIndex::at(path.clone(), None);

        first.record("first", "/first.bin", Some(1));
        first.persist().unwrap();
        second.record("second", "/second.bin", Some(2));
        second.file.as_ref().unwrap().state.lock().unwrap().written = COMPACT_AT;
        second.persist().unwrap();

        let reopened = SizeIndex::at(path, None);
        assert_eq!(reopened.peek("first", "/first.bin"), Some(1));
        assert_eq!(reopened.peek("second", "/second.bin"), Some(2));
    }
}

fn write(
    path: &Path,
    lines: &[String],
    key: Option<&[u8; 32]>,
    append: bool,
) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        crate::journal::set_permissions(parent, 0o700)?;
    }
    let mut payload = String::new();
    for line in lines {
        match key {
            Some(key) => payload.push_str(&crate::crypt::seal(key, line.as_bytes())?),
            None => payload.push_str(line),
        }
        payload.push('\n');
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    match append {
        true => options.append(true),
        false => options.write(true).truncate(true),
    };
    let mut file = options.open(path)?;
    crate::journal::set_permissions(path, 0o600)?;
    std::io::Write::write_all(&mut file, payload.as_bytes())?;
    Ok(())
}

/// The three size lookups attribution needs, implemented by the host.
///
/// A shell owns its own size index because that is storage policy, not
/// attribution: the macOS app persists and encrypts one, so a long-term watch
/// still has baselines after a relaunch. What the host must not own is the
/// arithmetic below, which is the same on every platform.
#[uniffi::export(with_foreign)]
pub trait SizeLookup: Send + Sync {
    /// Current allocated size. Implementations record it for later lookups.
    fn size(&self, path: String) -> Option<i64>;
    /// Last known size of a path that just vanished; consumed on use.
    fn prior_size(&self, path: String) -> Option<i64>;
    /// Last known size of a path that still exists, read before `size` so a
    /// modification reports growth instead of the whole file again.
    fn known_size(&self, path: String) -> Option<i64>;
}

/// [`Attributor`] for a host: the same logic, reached across the FFI.
#[derive(uniffi::Object)]
pub struct ActivityAttributor {
    options: AggregationOptions,
    sizes: Arc<dyn SizeLookup>,
}

#[uniffi::export]
impl ActivityAttributor {
    #[uniffi::constructor]
    pub fn new(options: AggregationOptions, sizes: Arc<dyn SizeLookup>) -> Arc<Self> {
        Arc::new(Self { options, sizes })
    }

    pub fn process(&self, changes: Vec<Change>) -> Vec<ActivityEvent> {
        let size = |path: &str| self.sizes.size(path.to_owned());
        let prior = |path: &str| self.sizes.prior_size(path.to_owned());
        let known = |path: &str| self.sizes.known_size(path.to_owned());
        Attributor::new(self.options, &size, &prior, &known).process(&changes)
    }
}

pub type SizeProvider<'a> = dyn Fn(&str) -> Option<i64> + Send + Sync + 'a;

pub struct Attributor<'a> {
    options: AggregationOptions,
    /// Current allocated size; the live provider also records it for later lookups.
    size: &'a SizeProvider<'a>,
    /// Last known size of a path that just vanished; consumed on use.
    prior_size: &'a SizeProvider<'a>,
    /// Last known size of a path that still exists, read before `size` so a
    /// modification reports growth instead of the whole file again.
    known_size: &'a SizeProvider<'a>,
}

impl<'a> Attributor<'a> {
    pub fn new(
        options: AggregationOptions,
        size: &'a SizeProvider<'a>,
        prior_size: &'a SizeProvider<'a>,
        known_size: &'a SizeProvider<'a>,
    ) -> Self {
        Self {
            options,
            size,
            prior_size,
            known_size,
        }
    }

    pub fn process(&self, changes: &[Change]) -> Vec<ActivityEvent> {
        let events: Vec<ActivityEvent> = changes
            .iter()
            .filter_map(|change| self.event_for(change))
            .collect();
        let mut events = self.aggregate(events);
        events.retain(|event| self.is_recordable(event));
        events
    }

    /// The size filter runs *after* aggregation so a folder's worth of small
    /// changes is judged as one change, and removals skip it entirely: wiping a
    /// thousand tiny files is the thing a threshold should surface, not hide.
    fn is_recordable(&self, event: &ActivityEvent) -> bool {
        match event.byte_delta {
            Some(delta) => {
                delta < 0
                    || matches!(event.kind, EventKind::Deleted)
                    || delta >= self.options.minimum_recorded_byte_delta
            }
            None => true,
        }
    }

    fn event_for(&self, change: &Change) -> Option<ActivityEvent> {
        let base = |kind, byte_delta, confidence, previous_path| ActivityEvent {
            kind,
            path: change.path.clone(),
            root_path: change.root_path.clone(),
            timestamp: change.timestamp,
            byte_delta,
            confidence,
            previous_path,
            affected_item_count: 1,
            process_name: change.process_name.clone(),
        };
        let sized = |kind, previous_path: Option<String>| {
            let known = (self.known_size)(&change.path);
            // The bound is on the file, so it is asked before the event is
            // built and before aggregation: a file this watch does not record
            // contributes nothing, not even to a directory row.
            (self.size)(&change.path)
                .filter(|size| self.options.watches_file(Some(*size)))
                .map(|size| {
                    let (byte_delta, confidence) = match (kind, known) {
                        (EventKind::Modified, None) => (None, Confidence::Unknown),
                        (_, Some(previous)) => (Some(size - previous), Confidence::Confirmed),
                        (EventKind::Created, None) => (Some(size), Confidence::Confirmed),
                        (_, None) => (None, Confidence::Unknown),
                    };
                    base(kind, byte_delta, confidence, previous_path)
                })
        };
        let vanished = |kind, previous_path: Option<String>| match (self.prior_size)(&change.path) {
            Some(prior) => self
                .options
                .watches_file(Some(prior))
                .then(|| base(kind, Some(-prior), Confidence::Estimated, previous_path)),
            None => Some(base(kind, None, Confidence::Unknown, previous_path)),
        };

        match &change.kind {
            ChangeKind::Created => sized(EventKind::Created, None),
            ChangeKind::Modified => sized(EventKind::Modified, None),
            ChangeKind::Deleted => vanished(EventKind::Deleted, None),
            // Consume the departed path's size first so a move inside the root nets
            // to its real growth (usually zero). A rename whose destination is gone
            // is the departure side (e.g. into the Trash); attribute it like a deletion.
            ChangeKind::Renamed { previous_path } => {
                let previous_known = previous_path.as_deref().and_then(|p| (self.prior_size)(p));
                let within_root = previous_path
                    .as_deref()
                    .is_some_and(|p| crate::paths::is_inside(&change.root_path, p));
                match (self.size)(&change.path)
                    .filter(|size| self.options.watches_file(Some(*size)))
                {
                    Some(size) => {
                        let delta = if within_root {
                            size - previous_known.unwrap_or(0)
                        } else {
                            size
                        };
                        let confidence = if within_root && previous_known.is_none() {
                            Confidence::Estimated
                        } else {
                            Confidence::Confirmed
                        };
                        Some(base(
                            EventKind::Moved,
                            Some(delta),
                            confidence,
                            previous_path.clone(),
                        ))
                    }
                    None => vanished(EventKind::Moved, previous_path.clone()),
                }
            }
        }
    }

    fn aggregate(&self, mut events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
        let window = self.options.aggregation_window_secs;
        if self.options.records_file_names || window == 0 {
            return events;
        }

        let mut groups: BTreeMap<(String, String, u64), Vec<ActivityEvent>> = BTreeMap::new();
        for event in events.drain(..) {
            let bucket = unix_seconds(event.timestamp) / window;
            let key = (event.root_path.clone(), parent_of(&event.path), bucket);
            groups.entry(key).or_default().push(event);
        }

        let mut aggregated: Vec<ActivityEvent> =
            groups.into_values().map(aggregate_group).collect();
        aggregated.sort_by(|lhs, rhs| {
            lhs.timestamp
                .cmp(&rhs.timestamp)
                .then_with(|| lhs.path.cmp(&rhs.path))
        });
        aggregated
    }
}

/// Collapses a window's changes to their parent directory, dropping the file
/// names along with them — including a change that is alone in its window.
/// Keeping the full path in that case would make the setting hold only for
/// folders busy enough to have something to merge with, which is not what
/// "do not record file names" means.
fn aggregate_group(mut events: Vec<ActivityEvent>) -> ActivityEvent {
    events.sort_by(|lhs, rhs| {
        lhs.timestamp
            .cmp(&rhs.timestamp)
            .then_with(|| lhs.path.cmp(&rhs.path))
    });

    let has_unknown = events.iter().any(|event| event.byte_delta.is_none());
    let byte_delta = if has_unknown {
        None
    } else {
        Some(events.iter().filter_map(|e| e.byte_delta).sum())
    };
    let confidence = if byte_delta.is_none() {
        Confidence::Unknown
    } else if events
        .iter()
        .all(|event| event.confidence == Confidence::Confirmed)
    {
        Confidence::Confirmed
    } else {
        Confidence::Estimated
    };

    ActivityEvent {
        kind: EventKind::Aggregate,
        path: parent_of(&events[0].path),
        root_path: events[0].root_path.clone(),
        timestamp: events[0].timestamp,
        byte_delta,
        confidence,
        previous_path: None,
        affected_item_count: events.iter().map(|event| event.affected_item_count).sum(),
        // One writer only when every change in the group came from the same
        // program. A mixed group has no single program to name.
        process_name: events[0].process_name.clone().filter(|name| {
            events
                .iter()
                .all(|event| event.process_name.as_deref() == Some(name.as_str()))
        }),
    }
}

pub(crate) fn parent_of(path: &str) -> String {
    match Path::new(path).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_string_lossy().into_owned(),
        _ => "/".to_owned(),
    }
}

pub(crate) fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
