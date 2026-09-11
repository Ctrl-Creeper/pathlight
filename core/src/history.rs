//! Folds journal rows into the dashboard's history shape.
//! Port of Swift's `ActivityHistoryService`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::attribution::unix_seconds;
use crate::{ActivityEvent, EventKind};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HistoryBucket {
    pub start: SystemTime,
    pub end: SystemTime,
    pub byte_delta: i64,
    pub event_count: u32,
    pub unknown_size_event_count: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HistorySnapshot {
    pub root_path: String,
    pub generated_at: SystemTime,
    pub total_net_byte_delta: i64,
    pub event_count: u32,
    pub unknown_size_event_count: u32,
    pub buckets: Vec<HistoryBucket>,
    /// Newest first, at most `recent_limit` rows.
    pub recent_events: Vec<ActivityEvent>,
    /// The totals above cover more rows than `recent_events` lists.
    pub is_truncated: bool,
}

/// What a person narrowed the history to.
///
/// The default is the whole record, newest first — what every host asks for
/// before anybody has typed anything, so the plain case costs no words.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct Query {
    /// Part of a path, matched without regard to case. Empty matches every row.
    pub text: String,
    /// One kind of change, or every kind.
    pub kind: Option<EventKind>,
    /// Biggest change first rather than newest first. The sign is ignored: the
    /// question is which change moved the most bytes, and a deletion moves as
    /// many as the write that made the file.
    pub largest_first: bool,
    /// Matching rows to step over before the listed page starts, so a host can
    /// page through without asking for a bigger limit each time.
    pub skip: u32,
}

impl Query {
    /// Whether this row is one of the rows being asked for.
    pub fn matches(&self, event: &ActivityEvent) -> bool {
        if self.kind.is_some_and(|kind| kind != event.kind) {
            return false;
        }
        self.text.is_empty() || contains_ignoring_case(&event.path, &self.text)
    }

    /// Newest first, or biggest first — with the newest order as the tiebreak
    /// either way, so two rows never swap places between two reads.
    fn order(&self, lhs: &ActivityEvent, rhs: &ActivityEvent) -> Ordering {
        let newest = rhs
            .timestamp
            .cmp(&lhs.timestamp)
            .then_with(|| rhs.path.cmp(&lhs.path));
        match self.largest_first {
            false => newest,
            true => moved(rhs).cmp(&moved(lhs)).then(newest),
        }
    }
}

/// How many bytes a change moved, whichever way they went. A row of unknown
/// size sorts as zero: it is the one row whose size nobody can compare.
fn moved(event: &ActivityEvent) -> i64 {
    event.byte_delta.unwrap_or(0).abs()
}

// ponytail: two short allocations per row, which is what a search box means by
// a match. Case-folding in place is the upgrade if a journal ever puts this in
// a measurement.
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// `events` are the rows already filtered to `root_path` (see `Journal::load`).
///
/// Totals and buckets cover every row `query` matched, and only
/// `recent_events` is cut to the page `query` asked for: totalling just the rows a caller can display understates
/// the answer by the ratio between the journal and the page.
pub fn build_history(
    root_path: &str,
    mut events: Vec<ActivityEvent>,
    bucket_interval_secs: u64,
    recent_limit: u32,
    query: &Query,
    generated_at: SystemTime,
) -> HistorySnapshot {
    let interval = bucket_interval_secs.max(1);
    events.retain(|event| query.matches(event));
    events.sort_by(|lhs, rhs| query.order(lhs, rhs));

    let mut groups: BTreeMap<u64, Vec<&ActivityEvent>> = BTreeMap::new();
    for event in &events {
        let start = unix_seconds(event.timestamp) / interval * interval;
        groups.entry(start).or_default().push(event);
    }
    let buckets = groups
        .into_iter()
        .map(|(start, bucket_events)| HistoryBucket {
            start: UNIX_EPOCH + Duration::from_secs(start),
            end: UNIX_EPOCH + Duration::from_secs(start + interval),
            byte_delta: bucket_events.iter().filter_map(|e| e.byte_delta).sum(),
            event_count: bucket_events.len() as u32,
            unknown_size_event_count: bucket_events
                .iter()
                .filter(|e| e.byte_delta.is_none())
                .count() as u32,
        })
        .collect();

    let skip = query.skip as usize;
    let matched = events.len();
    HistorySnapshot {
        root_path: crate::paths::normalize(root_path),
        generated_at,
        total_net_byte_delta: events.iter().filter_map(|e| e.byte_delta).sum(),
        event_count: matched as u32,
        unknown_size_event_count: events.iter().filter(|e| e.byte_delta.is_none()).count() as u32,
        buckets,
        is_truncated: matched > skip + recent_limit as usize,
        recent_events: events
            .into_iter()
            .skip(skip)
            .take(recent_limit as usize)
            .collect(),
    }
}
