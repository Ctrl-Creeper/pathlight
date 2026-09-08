//! Folds journal rows into the dashboard's history shape.
//! Port of Swift's `ActivityHistoryService`.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::attribution::unix_seconds;
use crate::ActivityEvent;

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

/// `events` are the rows already filtered to `root_path` (see `Journal::load`).
///
/// Totals and buckets cover every row given, and only `recent_events` is cut
/// to `recent_limit`: totalling just the rows a caller can display understates
/// the answer by the ratio between the journal and the page.
pub fn build_history(
    root_path: &str,
    mut events: Vec<ActivityEvent>,
    bucket_interval_secs: u64,
    recent_limit: u32,
    generated_at: SystemTime,
) -> HistorySnapshot {
    let interval = bucket_interval_secs.max(1);
    events.sort_by(|lhs, rhs| {
        rhs.timestamp
            .cmp(&lhs.timestamp)
            .then_with(|| rhs.path.cmp(&lhs.path))
    });

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

    let snapshot = HistorySnapshot {
        root_path: crate::paths::normalize(root_path),
        generated_at,
        total_net_byte_delta: events.iter().filter_map(|e| e.byte_delta).sum(),
        event_count: events.len() as u32,
        unknown_size_event_count: events.iter().filter(|e| e.byte_delta.is_none()).count() as u32,
        buckets,
        is_truncated: events.len() > recent_limit as usize,
        recent_events: events,
    };
    let mut snapshot = snapshot;
    snapshot.recent_events.truncate(recent_limit as usize);
    snapshot
}
