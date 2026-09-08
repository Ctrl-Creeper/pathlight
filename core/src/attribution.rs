//! Turns raw path changes into `ActivityEvent`s with byte deltas.
//! Port of Swift's `StorageAttributionService` + `FileAllocatedSizeProvider`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;
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
}

impl AggregationOptions {
    /// Long-term default: 10 MiB threshold, 5-minute aggregation, no file names.
    pub const LONG_TERM: Self = Self {
        minimum_recorded_byte_delta: 10 * 1024 * 1024,
        aggregation_window_secs: 5 * 60,
        records_file_names: false,
    };

    /// Live monitor default: everything, individually.
    pub const SHORT_TERM: Self = Self {
        minimum_recorded_byte_delta: 0,
        aggregation_window_secs: 0,
        records_file_names: true,
    };
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
// ponytail: in-memory only; Swift persists this as a journal so deletions after a
// relaunch still get a size. Add persistence when a non-mac shell needs it.
#[derive(Debug, Default)]
pub struct SizeIndex {
    sizes: Mutex<HashMap<String, i64>>,
}

impl SizeIndex {
    /// Records `size` for `path` (or forgets it when `None`) and returns it.
    pub fn record(&self, path: &str, size: Option<i64>) -> Option<i64> {
        let mut sizes = self.sizes.lock().unwrap();
        match size {
            Some(size) => sizes.insert(path.to_owned(), size),
            None => sizes.remove(path),
        };
        size
    }

    pub fn take(&self, path: &str) -> Option<i64> {
        self.sizes.lock().unwrap().remove(path)
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
            process_name: None,
        };
        let sized = |kind, previous_path: Option<String>| {
            let known = (self.known_size)(&change.path).unwrap_or(0);
            (self.size)(&change.path).map(|size| {
                base(
                    kind,
                    Some(size - known),
                    Confidence::Confirmed,
                    previous_path,
                )
            })
        };
        let vanished = |kind, previous_path: Option<String>| match (self.prior_size)(&change.path) {
            Some(prior) => base(kind, Some(-prior), Confidence::Estimated, previous_path),
            None => base(kind, None, Confidence::Unknown, previous_path),
        };

        match &change.kind {
            ChangeKind::Created => sized(EventKind::Created, None),
            ChangeKind::Modified => sized(EventKind::Modified, None),
            ChangeKind::Deleted => Some(vanished(EventKind::Deleted, None)),
            // Consume the departed path's size first so a move inside the root nets
            // to its real growth (usually zero). A rename whose destination is gone
            // is the departure side (e.g. into the Trash); attribute it like a deletion.
            ChangeKind::Renamed { previous_path } => {
                let previous_known = previous_path.as_deref().and_then(|p| (self.prior_size)(p));
                let within_root = previous_path
                    .as_deref()
                    .is_some_and(|p| crate::paths::is_inside(&change.root_path, p));
                match (self.size)(&change.path) {
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
                    None => Some(vanished(EventKind::Moved, previous_path.clone())),
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

fn aggregate_group(mut events: Vec<ActivityEvent>) -> ActivityEvent {
    if events.len() == 1 {
        return events.remove(0);
    }
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
        process_name: None,
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
