//! Turns raw path changes into `ActivityEvent`s with byte deltas.
//! Port of Swift's `StorageAttributionService` + `FileAllocatedSizeProvider`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::monitor::{Change, ChangeKind};
use crate::{ActivityEvent, Confidence, EventKind};

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
// ponytail: in-memory only; Swift persists this as a journal so deletions after a
// relaunch still get a size. Add persistence when a non-mac shell needs it.
#[derive(Debug, Default)]
pub struct SizeIndex {
    sizes: Mutex<HashMap<String, i64>>,
}

impl SizeIndex {
    /// Records `size` for `path` in `scope` (or forgets it when `None`).
    pub fn record(&self, scope: &str, path: &str, size: Option<i64>) -> Option<i64> {
        let mut sizes = self.sizes.lock().unwrap();
        match size {
            Some(size) => sizes.insert(key(scope, path), size),
            None => sizes.remove(&key(scope, path)),
        };
        size
    }

    pub fn take(&self, scope: &str, path: &str) -> Option<i64> {
        self.sizes.lock().unwrap().remove(&key(scope, path))
    }

    /// The last size recorded for `path` in `scope`, left in place. `take` is
    /// for a path that is gone; a file that was merely modified still needs
    /// its baseline afterwards, or the next change reports the whole file
    /// again instead of what was added to it.
    pub fn peek(&self, scope: &str, path: &str) -> Option<i64> {
        self.sizes.lock().unwrap().get(&key(scope, path)).copied()
    }
}

/// A newline cannot appear in a scope, so no scope can spell another one's key.
fn key(scope: &str, path: &str) -> String {
    format!("{scope}\n{path}")
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
