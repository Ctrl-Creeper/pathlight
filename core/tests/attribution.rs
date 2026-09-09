use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use pathlight_core::monitor::{Change, ChangeKind};
use pathlight_core::{
    ActivityAttributor, AggregationOptions, Attributor, Confidence, EventKind, SizeIndex,
    SizeLookup,
};

const SCOPE: &str = "short-term:/Users/example/Downloads";
const ROOT: &str = "/Users/example/Downloads";

fn change(kind: ChangeKind, name: &str, seconds: u64) -> Change {
    Change {
        kind,
        path: format!("{ROOT}/{name}"),
        root_path: ROOT.into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(seconds),
        process_name: None,
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
    let known = |path: &str| path.ends_with("tiny.txt").then_some(0);
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 1024,
            ..AggregationOptions::SHORT_TERM
        },
        &size,
        &none,
        &known,
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
fn first_modified_observation_has_unknown_growth_instead_of_the_whole_file_size() {
    let size = |_: &str| Some(8_192);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &none, &none);

    let events = attributor.process(&[change(ChangeKind::Modified, "existing.bin", 1)]);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Modified);
    assert_eq!(events[0].byte_delta, None);
    assert_eq!(events[0].confidence, Confidence::Unknown);
}

#[test]
fn deletions_and_departed_renames_use_prior_size() {
    let index = SizeIndex::default();
    index.record(SCOPE, &format!("{ROOT}/gone.zip"), Some(2048));
    index.record(SCOPE, &format!("{ROOT}/moved.mov"), Some(99));
    let size = |_: &str| None;
    let prior = |path: &str| index.take(SCOPE, path);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &prior, &none);

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
        index.take(SCOPE, &format!("{ROOT}/gone.zip")),
        None,
        "prior size is consumed"
    );
}

#[test]
fn aggregates_same_parent_inside_window() {
    let size = |_: &str| Some(1024 * 1024 * 20);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::LONG_TERM, &size, &none, &none);
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
    // `b/3.bin` is alone under its parent and still collapses: the setting is
    // about the path, not about how busy the folder was.
    assert_eq!(events[1].kind, EventKind::Aggregate);
    assert_eq!(events[1].path, format!("{ROOT}/b"));
    assert_eq!(events[1].affected_item_count, 1);
}

#[test]
fn modifications_report_growth_and_moves_inside_root_net_out() {
    let size = |_: &str| Some(5_000);
    let known = |path: &str| {
        if path.ends_with("report.pdf") {
            Some(3_000)
        } else {
            None
        }
    };
    let prior = |path: &str| {
        if path.ends_with("draft.mov") {
            Some(5_000)
        } else {
            None
        }
    };
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &prior, &known);

    let events = attributor.process(&[
        change(ChangeKind::Modified, "report.pdf", 1),
        change(ChangeKind::Created, "fresh.txt", 2),
        change(
            ChangeKind::Renamed {
                previous_path: Some(format!("{ROOT}/draft.mov")),
            },
            "final.mov",
            3,
        ),
        change(
            ChangeKind::Renamed {
                previous_path: Some("/Users/example/Desktop/clip.mov".into()),
            },
            "clip.mov",
            4,
        ),
        change(
            ChangeKind::Renamed {
                previous_path: Some(format!("{ROOT}/mystery.mov")),
            },
            "solved.mov",
            5,
        ),
    ]);
    let deltas: Vec<Option<i64>> = events.iter().map(|e| e.byte_delta).collect();
    assert_eq!(
        deltas,
        [Some(2_000), Some(5_000), Some(0), Some(5_000), Some(5_000)]
    );
    assert_eq!(events[4].confidence, Confidence::Estimated);
    assert_eq!(events[2].confidence, Confidence::Confirmed);
}

#[test]
fn small_deletions_survive_the_size_threshold() {
    // A long-term watch filters small *writes* as noise, but a hundred tiny
    // files vanishing is the exact event the threshold exists to surface.
    let prior = |_: &str| Some(4_096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 10 * 1024 * 1024,
            ..AggregationOptions::SHORT_TERM
        },
        &none,
        &prior,
        &none,
    );
    let events = attributor.process(&[
        change(ChangeKind::Deleted, "a.txt", 1),
        change(ChangeKind::Deleted, "b.txt", 2),
    ]);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.byte_delta == Some(-4_096)));
}

#[test]
fn aggregated_small_writes_are_judged_as_one_change() {
    // Individually each write is under the threshold; together they are not,
    // so filtering after aggregation keeps the folder-level total.
    let size = |_: &str| Some(600);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 1_000,
            aggregation_window_secs: 300,
            records_file_names: false,
        },
        &size,
        &none,
        &none,
    );
    let events = attributor.process(&[
        change(ChangeKind::Created, "a.txt", 1),
        change(ChangeKind::Created, "b.txt", 2),
    ]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Aggregate);
    assert_eq!(events[0].byte_delta, Some(1_200));
    assert_eq!(events[0].affected_item_count, 2);
}

/// Two watches over the same file are independent observers. Sharing one
/// baseline lets whichever watch handles the event first consume it, so the
/// other measures a delta of zero and its threshold drops the change.
#[test]
fn baselines_are_per_watch_not_per_path() {
    let index = SizeIndex::default();
    let path = format!("{ROOT}/shared.bin");
    index.record("long-term:/", &path, Some(4_096));
    index.record("short-term:/Users/example/Downloads", &path, Some(2_048));

    assert_eq!(index.take("long-term:/", &path), Some(4_096));
    assert_eq!(
        index.take("short-term:/Users/example/Downloads", &path),
        Some(2_048),
        "one watch consuming its baseline must not blind the other"
    );
}

/// With names switched off, a window holding one change still has to collapse.
/// The path is the whole disclosure, so a threshold on how busy the folder was
/// is not a privacy rule.
#[test]
fn a_lone_change_loses_its_file_name_too() {
    let size = |_: &str| Some(4096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 0,
            aggregation_window_secs: 300,
            records_file_names: false,
        },
        &size,
        &none,
        &none,
    );

    let events = attributor.process(&[change(ChangeKind::Created, "secret-report.pdf", 1)]);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Aggregate);
    assert_eq!(events[0].path, ROOT, "the file name survived the setting");
    assert_eq!(events[0].byte_delta, Some(4096));
    assert_eq!(events[0].affected_item_count, 1);
}

/// The surface a host reaches over the FFI: same arithmetic, sizes supplied by
/// the host because its index is storage policy (the macOS app persists and
/// encrypts one). Three lookups is three chances to wire the wrong one.
#[test]
fn the_ffi_attributor_measures_through_the_hosts_lookups() {
    struct HostSizes;
    impl SizeLookup for HostSizes {
        fn size(&self, _path: String) -> Option<i64> {
            Some(5_000)
        }
        fn prior_size(&self, path: String) -> Option<i64> {
            path.ends_with("gone.zip").then_some(2_048)
        }
        fn known_size(&self, path: String) -> Option<i64> {
            path.ends_with("report.pdf").then_some(3_000)
        }
    }

    let attributor = ActivityAttributor::new(AggregationOptions::SHORT_TERM, Arc::new(HostSizes));
    let events = attributor.process(vec![
        change(ChangeKind::Modified, "report.pdf", 1),
        change(ChangeKind::Deleted, "gone.zip", 2),
    ]);

    let deltas: Vec<Option<i64>> = events.iter().map(|event| event.byte_delta).collect();
    assert_eq!(deltas, [Some(2_000), Some(-2_048)]);
    assert_eq!(events[0].path, format!("{ROOT}/report.pdf"));
}
