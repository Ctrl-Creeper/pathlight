//! Turns raw path changes into `ActivityEvent`s with byte deltas.
//! Port of Swift's `StorageAttributionService` + `FileAllocatedSizeProvider`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// Long-term default: 1 KiB threshold, 5-minute aggregation, no file names.
    pub const LONG_TERM: Self = Self {
        minimum_recorded_byte_delta: 1024,
        aggregation_window_secs: 5 * 60,
        records_file_names: false,
        min_file_bytes: None,
        max_file_bytes: None,
    };

    /// Live monitor default: 1 KiB changes, individually.
    pub const SHORT_TERM: Self = Self {
        minimum_recorded_byte_delta: 1024,
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
    /// How long a baseline nobody has touched is kept. Zero is forever.
    keep_days: u32,
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
    ///
    /// `keep_days` is how long a baseline nobody has touched is worth keeping,
    /// zero being forever as everywhere else. Without it this is the one thing
    /// here that only grows: one entry per file ever changed, reloaded at every
    /// start, for the life of the machine. A dropped baseline costs one
    /// over-reported delta the next time that file changes, which is what a
    /// file first seen after a restart already costs.
    pub fn at(path: PathBuf, key_source: Option<PathBuf>, keep_days: u32) -> Self {
        let (loaded, written) = load(&path, key_source.as_deref(), cutoff(keep_days));
        let sizes = loaded
            .into_iter()
            .map(|(path, baseline)| (path, baseline.size))
            .collect();
        let lock_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            sizes: Mutex::new(sizes),
            file: Some(Persisted {
                path,
                lock_dir,
                keep_days,
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
                let (mut merged, _) = load(
                    &file.path,
                    file.key_source.as_deref(),
                    cutoff(file.keep_days),
                );
                apply(&mut merged, &lines);
                // The rewrite is where what aged out actually goes, from the
                // file and from memory alike. Compaction happens on writes, so
                // an index that is growing is an index that gets pruned.
                // ponytail: a baseline recorded between the take above and this
                // line is forgotten in memory while its line survives on disk —
                // one delta, the same cost as a line that never reached the
                // file. Hold both locks at once if that ever shows up.
                self.sizes
                    .lock()
                    .unwrap()
                    .retain(|key, _| merged.contains_key(key));
                let mut lines: Vec<String> = merged
                    .into_iter()
                    .filter_map(|(path, held)| line(path, Some(held.size), held.recorded_at))
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
        if let Some(line) = line(key, size, crate::event::swift_date::text(SystemTime::now())) {
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
    /// Written because the app's decoder requires it, and read back to decide
    /// what has gone stale. For one path the last line wins either way.
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
///
/// The stamp is passed in rather than taken here because compaction rewrites
/// lines it did not measure: stamping those with the time of the rewrite would
/// make every surviving baseline look new and nothing would ever age out.
fn line(path: String, size: Option<i64>, recorded_at: Option<String>) -> Option<String> {
    serde_json::to_string(&Entry {
        kind: match size {
            Some(_) => Kind::Record,
            None => Kind::Remove,
        },
        path,
        size,
        recorded_at,
    })
    .ok()
}

/// A baseline as the file holds it. Only the size is kept in memory between
/// compactions; the stamp is what the rewrite has to carry forward.
struct Baseline {
    size: i64,
    recorded_at: Option<String>,
}

/// When a baseline stops being worth its memory. `None` is forever.
fn cutoff(keep_days: u32) -> Option<SystemTime> {
    (keep_days > 0)
        .then(|| SystemTime::now().checked_sub(Duration::from_secs(u64::from(keep_days) * 86_400)))
        .flatten()
}

/// The baselines in `path`, and how many lines they took, or nothing when
/// there is no file yet.
fn load(
    path: &Path,
    key_source: Option<&Path>,
    cutoff: Option<SystemTime>,
) -> (HashMap<String, Baseline>, usize) {
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
        let Some(size) = entry.size.filter(|_| entry.kind == Kind::Record) else {
            sizes.remove(&entry.path);
            continue;
        };
        // A baseline whose stamp this build cannot read is kept rather than
        // aged out on a guess, the same rule the journal trims by.
        let stale = match (cutoff, entry.recorded_at.as_deref()) {
            (Some(cutoff), Some(stamp)) => {
                crate::event::swift_date::time(stamp).is_some_and(|at| at < cutoff)
            }
            _ => false,
        };
        match stale {
            true => sizes.remove(&entry.path),
            false => sizes.insert(
                entry.path,
                Baseline {
                    size,
                    recorded_at: entry.recorded_at,
                },
            ),
        };
    }
    (sizes, written)
}

fn apply(sizes: &mut HashMap<String, Baseline>, lines: &[String]) {
    for line in lines {
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        match entry.size.filter(|_| entry.kind == Kind::Record) {
            Some(size) => {
                let recorded_at = entry.recorded_at;
                sizes.insert(entry.path, Baseline { size, recorded_at });
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
        let first = SizeIndex::at(path.clone(), None, 0);
        let second = SizeIndex::at(path.clone(), None, 0);

        first.record("first", "/first.bin", Some(1));
        first.persist().unwrap();
        second.record("second", "/second.bin", Some(2));
        second.file.as_ref().unwrap().state.lock().unwrap().written = COMPACT_AT;
        second.persist().unwrap();

        let reopened = SizeIndex::at(path, None, 0);
        assert_eq!(reopened.peek("first", "/first.bin"), Some(1));
        assert_eq!(reopened.peek("second", "/second.bin"), Some(2));
    }

    /// A baseline nobody has touched inside the window goes, and a compaction
    /// carries the stamp it found rather than restamping what it rewrites.
    /// Restamping would make every survivor look new, and the index would grow
    /// for the life of the machine — one entry per file ever changed.
    #[test]
    fn baselines_age_out_and_a_compaction_does_not_reset_their_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity-size-index.jsonl");
        let days_ago = |days: u64| {
            crate::event::swift_date::text(SystemTime::now() - Duration::from_secs(days * 86_400))
        };
        let lines = vec![
            line(key("watch", "/ancient.bin"), Some(1), days_ago(400)).unwrap(),
            line(key("watch", "/older.bin"), Some(2), days_ago(100)).unwrap(),
        ];
        write(&path, &lines, None, false).unwrap();

        let index = SizeIndex::at(path.clone(), None, 180);
        assert_eq!(index.peek("watch", "/ancient.bin"), None);
        assert_eq!(index.peek("watch", "/older.bin"), Some(2));

        // Force the rewrite, then reopen with a window the survivor is outside
        // of: it can only still be there if the rewrite restamped it.
        index.record("watch", "/fresh.bin", Some(3));
        index.file.as_ref().unwrap().state.lock().unwrap().written = COMPACT_AT;
        index.persist().unwrap();

        let reopened = SizeIndex::at(path, None, 50);
        assert_eq!(
            reopened.peek("watch", "/older.bin"),
            None,
            "the compaction stamped a rewrite it did not measure"
        );
        assert_eq!(reopened.peek("watch", "/fresh.bin"), Some(3));
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
        // An arrival measures the whole file. A "modified" for the same path
        // in the same batch is the write that made it, and nobody has a size
        // from before to report growth against, so it would only add an
        // unknown row beside the sized one. A screenshot is the usual case:
        // written under a dot name, renamed into place, touched once more.
        let arrivals: HashSet<&str> = changes
            .iter()
            .filter(|change| {
                matches!(
                    change.kind,
                    ChangeKind::Created | ChangeKind::Renamed { .. }
                )
            })
            .map(|change| change.path.as_str())
            .collect();
        let events: Vec<AttributedEvent> = changes
            .iter()
            .filter(|change| {
                !(matches!(change.kind, ChangeKind::Modified)
                    && arrivals.contains(change.path.as_str()))
            })
            .filter_map(|change| self.event_for(change))
            .map(|event| AttributedEvent { event })
            .collect();
        let mut events = self.aggregate(events);
        events.retain(|event| self.is_recordable(&event.event));
        events.into_iter().map(|event| event.event).collect()
    }

    /// The size filter runs *after* aggregation so a folder's worth of small
    /// changes is judged as one change. A known deletion or move follows the
    /// same byte-delta rule; unknown changes remain visible because there is
    /// no reliable value to compare with the threshold.
    fn is_recordable(&self, event: &ActivityEvent) -> bool {
        match event.byte_delta {
            Some(delta) => delta.saturating_abs() >= self.options.minimum_recorded_byte_delta,
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
            let Some(size) = (self.size)(&change.path) else {
                // Nothing to test a bound against, so the two reasons a
                // measurement fails have to be told apart. A path that is
                // simply gone is the deletion's to report. One that is still
                // there and could not be read — no permission, an unsupported
                // filesystem — is a change that happened, and dropping it
                // would leave a folder looking untouched because of a stat
                // that failed. See `size_in_bounds`.
                return std::fs::symlink_metadata(&change.path)
                    .is_ok()
                    .then(|| base(kind, None, Confidence::Unknown, previous_path));
            };
            // The bound is on the file, so it is asked before the event is
            // built and before aggregation: a file this watch does not record
            // contributes nothing, not even to a directory row.
            if !self.options.watches_file(Some(size)) {
                return None;
            }
            let (kind, byte_delta, confidence) = match (kind, known) {
                (EventKind::Modified, None) => (kind, None, Confidence::Unknown),
                // A "created" for a file already measured is the rest of a
                // write still in progress when the first batch was flushed:
                // the same file growing, not a second one.
                (EventKind::Created, Some(previous)) => (
                    EventKind::Modified,
                    Some(size - previous),
                    Confidence::Confirmed,
                ),
                (_, Some(previous)) => (kind, Some(size - previous), Confidence::Confirmed),
                (EventKind::Created, None) => (kind, Some(size), Confidence::Confirmed),
                (_, None) => (kind, None, Confidence::Unknown),
            };
            Some(base(kind, byte_delta, confidence, previous_path))
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

    fn aggregate(&self, mut events: Vec<AttributedEvent>) -> Vec<AttributedEvent> {
        let window = self.options.aggregation_window_secs;
        if self.options.records_file_names || window == 0 {
            return events;
        }

        let mut groups: BTreeMap<(String, String, u64), Vec<AttributedEvent>> = BTreeMap::new();
        for event in events.drain(..) {
            let bucket = unix_seconds(event.event.timestamp) / window;
            let key = (
                event.event.root_path.clone(),
                parent_of(&event.event.path),
                bucket,
            );
            groups.entry(key).or_default().push(event);
        }

        let mut aggregated: Vec<AttributedEvent> =
            groups.into_values().map(aggregate_group).collect();
        aggregated.sort_by(|lhs, rhs| {
            lhs.event
                .timestamp
                .cmp(&rhs.event.timestamp)
                .then_with(|| lhs.event.path.cmp(&rhs.event.path))
        });
        aggregated
    }
}

/// Collapses a window's changes to their parent directory, dropping the file
/// names along with them — including a change that is alone in its window.
/// Keeping the full path in that case would make the setting hold only for
/// folders busy enough to have something to merge with, which is not what
/// "do not record file names" means.
struct AttributedEvent {
    event: ActivityEvent,
}

fn aggregate_group(mut events: Vec<AttributedEvent>) -> AttributedEvent {
    events.sort_by(|lhs, rhs| {
        lhs.event
            .timestamp
            .cmp(&rhs.event.timestamp)
            .then_with(|| lhs.event.path.cmp(&rhs.event.path))
    });

    let has_unknown = events.iter().any(|event| event.event.byte_delta.is_none());
    let byte_delta = if has_unknown {
        None
    } else {
        Some(events.iter().filter_map(|e| e.event.byte_delta).sum())
    };
    let confidence = if byte_delta.is_none() {
        Confidence::Unknown
    } else if events
        .iter()
        .all(|event| event.event.confidence == Confidence::Confirmed)
    {
        Confidence::Confirmed
    } else {
        Confidence::Estimated
    };

    let event = ActivityEvent {
        kind: EventKind::Aggregate,
        path: parent_of(&events[0].event.path),
        root_path: events[0].event.root_path.clone(),
        timestamp: events[0].event.timestamp,
        byte_delta,
        confidence,
        previous_path: None,
        affected_item_count: events
            .iter()
            .map(|event| event.event.affected_item_count)
            .sum(),
        // One writer only when every change in the group came from the same
        // program. A mixed group has no single program to name.
        process_name: events[0].event.process_name.clone().filter(|name| {
            events
                .iter()
                .all(|event| event.event.process_name.as_deref() == Some(name.as_str()))
        }),
    };
    AttributedEvent { event }
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
