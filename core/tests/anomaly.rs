//! What both hosts agree is worth interrupting somebody for.
//!
//! These cases came from the macOS app's `ActivityAnomalyDetectorTests` and
//! the desktop shell's own notification tests, and they live here because the
//! policy does: the numbers used to be written down twice, and a threshold
//! that drifts means one host stays quiet through what the other reports as
//! an emergency.

use std::time::{Duration, SystemTime};

use pathlight_core::anomaly::{anomalies, AnomalyKind, WINDOW};
use pathlight_core::{ActivityEvent, Confidence, EventKind};

const NOW: SystemTime = SystemTime::UNIX_EPOCH;

fn now() -> SystemTime {
    NOW + Duration::from_secs(1_700_000_000)
}

fn event(kind: EventKind, delta: i64, items: u32, seconds_ago: u64) -> ActivityEvent {
    ActivityEvent {
        kind,
        path: format!("/watched/file{seconds_ago}"),
        root_path: "/watched".to_owned(),
        timestamp: now() - Duration::from_secs(seconds_ago),
        byte_delta: Some(delta),
        confidence: Confidence::Confirmed,
        previous_path: None,
        affected_item_count: items,
        process_name: None,
    }
}

/// Either enough items or enough bytes: a folder can be emptied of a hundred
/// thousand small files or of one large one, and both are the same event to
/// the person it happened to.
#[test]
fn a_mass_deletion_is_found_by_item_count_or_by_size() {
    let many_small: Vec<_> = (0..120)
        .map(|index| event(EventKind::Deleted, -10, 1, 60 + index))
        .collect();
    let found = anomalies(&many_small, now());
    assert_eq!(found.len(), 1, "120 deleted items went unmentioned");
    assert_eq!(found[0].kind, AnomalyKind::Removal);
    assert_eq!(found[0].items, 120);

    let one_huge = [event(EventKind::Deleted, -2_000_000_000, 1, 30)];
    assert_eq!(
        anomalies(&one_huge, now())[0].kind,
        AnomalyKind::Removal,
        "2 GB gone in one row is still 2 GB gone"
    );
}

/// A row's item count is what it stands for: one aggregate row can describe a
/// directory that took thousands of files with it.
#[test]
fn one_row_can_carry_the_whole_deletion() {
    let collapsed = [event(EventKind::Deleted, -4096, 200, 5)];
    let found = anomalies(&collapsed, now());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].items, 200);
}

#[test]
fn a_write_burst_is_found_by_size_and_reported_positive() {
    let burst = [event(EventKind::Created, 6_000_000_000, 1, 10)];
    let found = anomalies(&burst, now());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, AnomalyKind::Burst);
    assert_eq!(found[0].bytes, 6_000_000_000);
}

/// Ordinary use is not an anomaly, and neither is an old one: a threshold
/// measured over the whole journal fires on every folder eventually.
#[test]
fn quiet_and_long_ago_are_both_quiet() {
    let ordinary = [event(EventKind::Modified, 4096, 1, 10)];
    assert!(anomalies(&ordinary, now()).is_empty());

    let old: Vec<_> = (0..120)
        .map(|index| event(EventKind::Deleted, -10, 1, 3_600 + index))
        .collect();
    assert!(
        anomalies(&old, now()).is_empty(),
        "an hour-old deletion was reported as happening now"
    );

    let just_outside = [event(
        EventKind::Created,
        6_000_000_000,
        1,
        WINDOW.as_secs() + 1,
    )];
    assert!(anomalies(&just_outside, now()).is_empty());
}

/// A clock that jumped is not a folder that changed.
#[test]
fn a_row_stamped_in_the_future_is_not_news() {
    let mut ahead = event(EventKind::Created, 6_000_000_000, 1, 0);
    ahead.timestamp = now() + Duration::from_secs(60);
    assert!(anomalies(&[ahead], now()).is_empty());
}

/// A deletion with no measured size still counts as items removed: an
/// unmeasurable delete is the one most worth mentioning.
#[test]
fn a_deletion_of_unknown_size_still_counts() {
    let mut unmeasured = event(EventKind::Deleted, 0, 150, 30);
    unmeasured.byte_delta = None;
    let found = anomalies(&[unmeasured], now());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].items, 150);
    assert_eq!(found[0].bytes, 0);
}
