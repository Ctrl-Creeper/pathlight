use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use pathlight_core::monitor::{Change, ChangeKind};
use pathlight_core::store::Storage;
use pathlight_core::{
    ActivityAttributor, AggregationOptions, Attributor, Confidence, EventKind, SizeIndex,
    SizeLookup,
};

const SCOPE: &str = "short-term:/Users/example/Downloads";
const ROOT: &str = "/Users/example/Downloads";

#[test]
fn new_watches_default_to_one_kibibyte_changes() {
    assert_eq!(
        AggregationOptions::LONG_TERM.minimum_recorded_byte_delta,
        1_024
    );
    assert_eq!(
        AggregationOptions::SHORT_TERM.minimum_recorded_byte_delta,
        1_024
    );
}

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

/// A screenshot is written under a dot name, renamed into place and touched
/// once more, all inside one batch. The rename sizes the file; the touch
/// must not add an unknown row beside it.
#[test]
fn a_modification_in_the_batch_that_brought_the_file_adds_no_unknown_row() {
    let size = |_: &str| Some(2_383_872);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &none, &none);

    let events = attributor.process(&[
        change(ChangeKind::Modified, "Screenshot.png", 1),
        change(
            ChangeKind::Renamed {
                previous_path: Some(format!("{ROOT}/.Screenshot.png")),
            },
            "Screenshot.png",
            1,
        ),
    ]);

    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, EventKind::Moved);
    assert_eq!(events[0].byte_delta, Some(2_383_872));
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
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].byte_delta, Some(-2048));
    assert_eq!(events[0].confidence, Confidence::Estimated);
    assert_eq!(events[1].kind, EventKind::Deleted);
    assert_eq!(events[1].byte_delta, None);
    assert_eq!(events[1].confidence, Confidence::Unknown);
    assert_eq!(
        index.take(SCOPE, &format!("{ROOT}/gone.zip")),
        None,
        "prior size is consumed"
    );
}

#[test]
fn a_rename_over_a_file_counts_only_its_growth() {
    // Chrome saves Preferences by writing a dot-named temporary and renaming
    // it into place. The temporary is gone before the batch is read, so it is
    // never measured, and the old file's inode reports a second rename at the
    // same path. The folder grew by the difference, once.
    let known = std::sync::atomic::AtomicI64::new(516_096);
    let size = |_: &str| {
        known.store(520_000, std::sync::atomic::Ordering::Relaxed);
        Some(520_000)
    };
    let known_size = |_: &str| Some(known.load(std::sync::atomic::Ordering::Relaxed));
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &none, &known_size);
    let events = attributor.process(&[
        change(
            ChangeKind::Renamed {
                previous_path: Some(format!("{ROOT}/.Preferences.7dlZ2a")),
            },
            "Preferences",
            1,
        ),
        change(
            ChangeKind::Renamed {
                previous_path: None,
            },
            "Preferences",
            1,
        ),
    ]);
    let deltas: Vec<_> = events.iter().map(|event| event.byte_delta).collect();
    assert_eq!(deltas, vec![Some(3_904)]);
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
    assert_eq!(deltas, [Some(2_000), Some(5_000), Some(5_000), Some(5_000)]);
    assert_eq!(events[3].confidence, Confidence::Estimated);
}

/// A large file is written across two batches. FSEvents keeps saying
/// "created" for it; the second row is the growth since the first, and says so.
#[test]
fn a_second_creation_of_a_measured_file_is_growth() {
    let size = |_: &str| Some(3_000);
    let known = |_: &str| Some(1_000);
    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &size, &none, &known);

    let events = attributor.process(&[change(ChangeKind::Created, "big.dmg", 1)]);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Modified);
    assert_eq!(events[0].byte_delta, Some(2_000));
    assert_eq!(events[0].confidence, Confidence::Confirmed);
}

#[test]
fn small_deletions_respect_the_size_threshold() {
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
    assert!(events.is_empty());
}

#[test]
fn zero_growth_renames_respect_the_size_threshold() {
    let current = |_: &str| Some(4_096);
    let prior = |_: &str| Some(4_096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 10 * 1024 * 1024,
            ..AggregationOptions::SHORT_TERM
        },
        &current,
        &prior,
        &none,
    );

    let events = attributor.process(&[change(
        ChangeKind::Renamed {
            previous_path: Some(format!("{ROOT}/before.txt")),
        },
        "after.txt",
        1,
    )]);

    assert!(events.is_empty());
}

#[test]
fn aggregated_zero_growth_renames_respect_the_size_threshold() {
    let current = |_: &str| Some(4_096);
    let prior = |_: &str| Some(4_096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 10 * 1024 * 1024,
            aggregation_window_secs: 300,
            records_file_names: false,
            min_file_bytes: None,
            max_file_bytes: None,
        },
        &current,
        &prior,
        &none,
    );

    let events = attributor.process(&[change(
        ChangeKind::Renamed {
            previous_path: Some(format!("{ROOT}/before.txt")),
        },
        "after.txt",
        1,
    )]);

    assert!(events.is_empty());
}

#[test]
fn an_aggregated_removal_is_filtered_when_the_net_change_is_small() {
    let current = |path: &str| path.ends_with("arrived.txt").then_some(4_000);
    let prior = |path: &str| path.ends_with("gone.txt").then_some(4_096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 10 * 1024 * 1024,
            aggregation_window_secs: 300,
            records_file_names: false,
            min_file_bytes: None,
            max_file_bytes: None,
        },
        &current,
        &prior,
        &none,
    );

    let events = attributor.process(&[
        change(ChangeKind::Deleted, "gone.txt", 1),
        change(ChangeKind::Created, "arrived.txt", 2),
    ]);

    assert!(events.is_empty());
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
            min_file_bytes: None,
            max_file_bytes: None,
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

#[test]
fn small_modification_shrinkage_respects_the_threshold() {
    let current = |_: &str| Some(4_095);
    let known = |_: &str| Some(4_096);
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 1_024,
            ..AggregationOptions::SHORT_TERM
        },
        &current,
        &none,
        &known,
    );

    let events = attributor.process(&[change(ChangeKind::Modified, "file.txt", 1)]);

    assert!(
        events.is_empty(),
        "a one-byte truncation bypassed the threshold"
    );
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
            min_file_bytes: None,
            max_file_bytes: None,
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

/// The bound is on the file, not on how much of it changed: a 4 KiB scratch
/// file is not recorded by a watch that was told to look at big files, however
/// much of it was rewritten.
#[test]
fn a_watch_records_only_files_inside_its_size_bounds() {
    let sizes = |path: &str| match path {
        _ if path.ends_with("tiny.txt") => Some(4_096),
        _ if path.ends_with("big.iso") => Some(8_000_000_000),
        _ => Some(50_000_000),
    };
    let none = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 0,
            aggregation_window_secs: 0,
            records_file_names: true,
            min_file_bytes: Some(1_000_000),
            max_file_bytes: Some(1_000_000_000),
        },
        &sizes,
        &none,
        &none,
    );

    let events = attributor.process(&[
        change(ChangeKind::Modified, "tiny.txt", 1),
        change(ChangeKind::Modified, "big.iso", 1),
        change(ChangeKind::Modified, "movie.mp4", 1),
    ]);

    assert_eq!(
        events
            .iter()
            .map(|event| event.path.as_str())
            .collect::<Vec<_>>(),
        [format!("{ROOT}/movie.mp4")]
    );
}

/// A file whose size cannot be read was never tested against the bounds, so it
/// stays recorded. Deletions are the common case, and a deletion this app drops
/// silently is worse than a row the user did not ask for.
#[test]
fn a_file_of_unknown_size_is_recorded_whatever_the_bounds_say() {
    let unknown = |_: &str| None;
    let attributor = Attributor::new(
        AggregationOptions {
            minimum_recorded_byte_delta: 0,
            aggregation_window_secs: 0,
            records_file_names: true,
            min_file_bytes: Some(1_000_000_000),
            max_file_bytes: Some(2_000_000_000),
        },
        &unknown,
        &unknown,
        &unknown,
    );

    let events = attributor.process(&[change(ChangeKind::Deleted, "gone.bin", 1)]);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].byte_delta, None);
}

/// The bounds are asked about the file that was deleted, using the size the
/// watch last knew, so a small file's removal is filtered by a big-file watch
/// while a big one's removal still lands.
#[test]
fn a_removal_is_judged_by_the_size_the_watch_last_knew() {
    let gone = |_: &str| None;
    let small = |_: &str| Some(4_096);
    let large = |_: &str| Some(4_000_000_000);
    let options = AggregationOptions {
        minimum_recorded_byte_delta: 0,
        aggregation_window_secs: 0,
        records_file_names: true,
        min_file_bytes: Some(1_000_000),
        max_file_bytes: None,
    };

    let filtered = Attributor::new(options, &gone, &small, &gone).process(&[change(
        ChangeKind::Deleted,
        "scratch.tmp",
        1,
    )]);
    let kept = Attributor::new(options, &gone, &large, &gone).process(&[change(
        ChangeKind::Deleted,
        "backup.dmg",
        1,
    )]);

    assert!(filtered.is_empty());
    assert_eq!(kept.len(), 1);
}

/// Why the baselines are on disk at all: a watch reopened tomorrow has to
/// measure what changed rather than call the first change to every file a
/// whole new file — and a deletion after a restart still needs a size.
#[test]
fn a_size_measured_before_a_restart_is_still_the_baseline_after_one() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::at(dir.path());
    let index = storage.size_index();
    index.record(SCOPE, "/a/big.bin", Some(4096));
    index.persist().unwrap();
    drop(index);

    let reopened = storage.size_index();
    assert_eq!(reopened.peek(SCOPE, "/a/big.bin"), Some(4096));
    // A second watch over the same file is its own observer, restart or not.
    assert_eq!(reopened.peek("another watch", "/a/big.bin"), None);

    // A path taken as gone stays gone, or the next restart would resurrect a
    // baseline for a file that is not there and report its deletion twice.
    assert_eq!(reopened.take(SCOPE, "/a/big.bin"), Some(4096));
    reopened.persist().unwrap();
    assert_eq!(storage.size_index().peek(SCOPE, "/a/big.bin"), None);
}

/// The baselines name every file a watch has measured, so the setting that
/// seals the rows has to seal these too.
#[test]
fn baselines_are_sealed_when_the_records_are() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::at(dir.path());
    storage.set_encrypting(true).unwrap();
    let index = storage.size_index();
    index.record(SCOPE, "/a/private.bin", Some(9));
    index.persist().unwrap();

    let text = std::fs::read_to_string(dir.path().join("activity-size-index.jsonl")).unwrap();
    assert!(!text.contains("private.bin"), "{text}");
    // And this machine still reads its own back.
    assert_eq!(storage.size_index().peek(SCOPE, "/a/private.bin"), Some(9));
}

/// A file that is still there but cannot be measured — no permission, a
/// filesystem that will not answer — is a change that happened. Dropping it
/// leaves the folder looking untouched because one stat failed; a path that is
/// simply gone is the deletion's to report, not this one's.
#[test]
fn a_file_that_cannot_be_measured_is_still_recorded_but_one_that_is_gone_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().into_owned();
    let present = dir.path().join("unreadable.bin");
    std::fs::write(&present, b"x").unwrap();

    let none = |_: &str| None;
    let attributor = Attributor::new(AggregationOptions::SHORT_TERM, &none, &none, &none);
    let modified = |path: std::path::PathBuf| Change {
        kind: ChangeKind::Modified,
        path: path.to_string_lossy().into_owned(),
        root_path: root.clone(),
        timestamp: UNIX_EPOCH + Duration::from_secs(1),
        process_name: None,
    };
    let events = attributor.process(&[modified(present), modified(dir.path().join("gone.bin"))]);

    assert_eq!(events.len(), 1, "events: {events:#?}");
    assert!(events[0].path.ends_with("unreadable.bin"));
    assert_eq!(events[0].byte_delta, None);
    assert_eq!(events[0].confidence, Confidence::Unknown);
}
