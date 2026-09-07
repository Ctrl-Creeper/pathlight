use std::time::{Duration, UNIX_EPOCH};

use pathlight_core::monitor::{Change, ChangeKind};
use pathlight_core::{AggregationOptions, Attributor, Confidence, EventKind, SizeIndex};

const ROOT: &str = "/Users/example/Downloads";

fn change(kind: ChangeKind, name: &str, seconds: u64) -> Change {
    Change {
        kind,
        path: format!("{ROOT}/{name}"),
        root_path: ROOT.into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(seconds),
    }
}

#[test]
fn records_created_file_and_filters_below_threshold() {
    let size = |path: &str| {
        if path.ends_with("big.dmg") {
            Some(4096)
        } else {
            Some(10)
        }
    };
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 1024,
            ..AggregationOptions::SHORT_TERM
        },
        &size,
        &none,
    );
    let events = attributor.process(&[
        change(ChangeKind::Created, "big.dmg", 1),
        change(ChangeKind::Modified, "tiny.txt", 2),
    ]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Created);
    assert_eq!(events[0].byte_delta, Some(4096));
    assert_eq!(events[0].confidence, Confidence::Confirmed);
}

#[test]
fn deletions_and_departed_renames_use_prior_size() {
    let index = SizeIndex::default();
    index.record(&format!("{ROOT}/gone.zip"), Some(2048));
    index.record(&format!("{ROOT}/moved.mov"), Some(99));
    let size = |_: &str| None;
    let prior = |path: &str| index.take(path);
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &prior);

    let events = attributor.process(&[
        change(ChangeKind::Deleted, "gone.zip", 1),
        change(
            ChangeKind::Renamed {
                previous_path: None,
            },
            "moved.mov",
            2,
        ),
        change(ChangeKind::Deleted, "unknown.bin", 3),
    ]);
    assert_eq!(events[0].byte_delta, Some(-2048));
    assert_eq!(events[0].confidence, Confidence::Estimated);
    assert_eq!(events[1].kind, EventKind::Moved);
    assert_eq!(events[1].byte_delta, Some(-99));
    assert_eq!(events[2].byte_delta, None);
    assert_eq!(events[2].confidence, Confidence::Unknown);
    assert_eq!(
        index.take(&format!("{ROOT}/gone.zip")),
        None,
        "prior size is consumed"
    );
}

#[test]
fn aggregates_same_parent_inside_window() {
    let size = |_: &str| Some(1024 * 1024 * 20);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::LONG_TERM, &size, &none);
    let events = attributor.process(&[
        change(ChangeKind::Created, "a/1.bin", 10),
        change(ChangeKind::Created, "a/2.bin", 20),
        change(ChangeKind::Created, "b/3.bin", 30),
    ]);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Aggregate);
    assert_eq!(events[0].path, format!("{ROOT}/a"));
    assert_eq!(events[0].affected_item_count, 2);
    assert_eq!(events[0].byte_delta, Some(2 * 1024 * 1024 * 20));
    assert_eq!(events[1].kind, EventKind::Created);
}
