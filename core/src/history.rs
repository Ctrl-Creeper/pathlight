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

/// Accumulates a snapshot one row at a time, holding the page being asked for
/// rather than every row that matched.
///
/// Mirrors Swift's `ActivityEventPageBuilder`: matched rows go into a window
/// twice the size of the page and are cut back to the page whenever it fills,
/// so a journal of any length costs the page plus one bucket per interval.
pub struct Builder<'a> {
    root_path: &'a str,
    query: &'a Query,
    generated_at: SystemTime,
    interval: u64,
    /// Rows to hold: the page plus everything a caller is paging past.
    wanted: usize,
    buckets: BTreeMap<u64, HistoryBucket>,
    page: Vec<ActivityEvent>,
    matched: usize,
    total_net_byte_delta: i64,
    unknown: usize,
}

impl<'a> Builder<'a> {
    pub fn new(
        root_path: &'a str,
        bucket_interval_secs: u64,
        recent_limit: u32,
        query: &'a Query,
        generated_at: SystemTime,
    ) -> Self {
        let wanted = query.skip as usize + recent_limit as usize;
        Self {
            root_path,
            query,
            generated_at,
            interval: bucket_interval_secs.max(1),
            wanted,
            buckets: BTreeMap::new(),
            page: Vec::with_capacity(wanted.saturating_mul(2).min(4_096)),
            matched: 0,
            total_net_byte_delta: 0,
            unknown: 0,
        }
    }

    /// One row of the journal, already filtered to this root.
    pub fn push(&mut self, event: ActivityEvent) {
        if !self.query.matches(&event) {
            return;
        }
        self.matched += 1;
        let start = unix_seconds(event.timestamp) / self.interval * self.interval;
        let bucket = self.buckets.entry(start).or_insert_with(|| HistoryBucket {
            start: UNIX_EPOCH + Duration::from_secs(start),
            end: UNIX_EPOCH + Duration::from_secs(start + self.interval),
            byte_delta: 0,
            event_count: 0,
            unknown_size_event_count: 0,
        });
        bucket.event_count += 1;
        match event.byte_delta {
            Some(delta) => {
                self.total_net_byte_delta += delta;
                bucket.byte_delta += delta;
            }
            None => {
                self.unknown += 1;
                bucket.unknown_size_event_count += 1;
            }
        }

        if self.wanted == 0 {
            return;
        }
        self.page.push(event);
        if self.page.len() >= self.wanted * 2 {
            self.cut();
        }
    }

    /// Back down to the rows being asked for. The sort is stable and the rows
    /// arrive in journal order, so cutting early picks the same rows a sort of
    /// the whole journal would have.
    fn cut(&mut self) {
        let query = self.query;
        self.page.sort_by(|lhs, rhs| query.order(lhs, rhs));
        self.page.truncate(self.wanted);
    }

    /// Totals and buckets cover every row `query` matched, and only
    /// `recent_events` is cut to the page: totalling just the rows a caller can
    /// display understates the answer by the ratio between the journal and the
    /// page.
    pub fn finish(mut self) -> HistorySnapshot {
        self.cut();
        HistorySnapshot {
            root_path: crate::paths::normalize(self.root_path),
            generated_at: self.generated_at,
            total_net_byte_delta: self.total_net_byte_delta,
            event_count: self.matched as u32,
            unknown_size_event_count: self.unknown as u32,
            buckets: self.buckets.into_values().collect(),
            is_truncated: self.matched > self.wanted,
            recent_events: self
                .page
                .into_iter()
                .skip(self.query.skip as usize)
                .collect(),
        }
    }
}

/// `events` are the rows already filtered to `root_path` (see `Journal::load`).
pub fn build_history(
    root_path: &str,
    events: Vec<ActivityEvent>,
    bucket_interval_secs: u64,
    recent_limit: u32,
    query: &Query,
    generated_at: SystemTime,
) -> HistorySnapshot {
    let mut builder = Builder::new(
        root_path,
        bucket_interval_secs,
        recent_limit,
        query,
        generated_at,
    );
    for event in events {
        builder.push(event);
    }
    builder.finish()
}
