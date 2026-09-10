use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use pathlight_core::evidence::{EvidenceJournal, EvidencePayload, NativePath};
use pathlight_core::monitor::ChangeKind;
use pathlight_core::recording::{record_bounded, record_for, record_session, SizeBounds};

#[test]
fn records_real_changes_and_two_interval_snapshots_to_an_external_journal() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let journal_path = output.path().join("evidence.jsonl");
    fs::write(root.path().join("original"), b"retained object").unwrap();

    // Mutating from `on_live` puts the changes inside the interval the two
    // snapshots bracket, without racing the recorder from another thread.
    let summary = record_session(root.path(), &journal_path, Duration::from_secs(3), || {
        fs::rename(root.path().join("original"), root.path().join("renamed")).unwrap();
        fs::write(root.path().join("added"), vec![1; 4096]).unwrap();
    })
    .unwrap();
    let records = EvidenceJournal::open(&journal_path)
        .unwrap()
        .read_records()
        .unwrap();

    assert!(summary.observations > 0);
    assert_eq!(summary.gaps, 0);
    assert_eq!(records.len() as u64, summary.records_written);
    let snapshots: Vec<_> = records
        .iter()
        .filter_map(|record| match record.payload {
            EvidencePayload::Snapshot { logical_bytes, .. } => Some(logical_bytes),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots, vec![Some(15), Some(4111)]);
    let renames = records.iter().filter(|record| match &record.payload {
        EvidencePayload::Observation { change, .. } => {
            matches!(change.kind, ChangeKind::Renamed { .. })
                && Path::new(&change.path).file_name().unwrap() == "renamed"
        }
        _ => false,
    });
    assert_eq!(renames.count(), 1);
    assert!(records.iter().any(|record| match &record.payload {
        EvidencePayload::Observation {
            change,
            measurement: Some(measurement),
            ..
        } => {
            Path::new(&change.path).file_name().unwrap() == "added"
                && measurement.logical_bytes == 4096
        }
        _ => false,
    }));
}

#[test]
fn rejects_a_journal_inside_the_watched_tree_without_creating_it() {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("self-observation.jsonl");
    let error = record_for(root.path(), &journal, Duration::from_millis(1)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(!journal.exists());
}

#[test]
fn rejects_a_journal_with_a_hard_link_inside_the_watched_tree() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let watched_alias = root.path().join("journal-alias");
    let journal = output.path().join("evidence.jsonl");
    fs::write(&watched_alias, b"").unwrap();
    fs::hard_link(&watched_alias, &journal).unwrap();

    let error = record_for(root.path(), &journal, Duration::from_millis(1)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(fs::read(&watched_alias).unwrap(), b"");
}

#[test]
fn an_idle_session_records_its_scope_and_a_terminal_state_without_claiming_complete_history() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let journal_path = output.path().join("evidence.jsonl");
    let summary = record_for(root.path(), &journal_path, Duration::from_millis(1)).unwrap();
    let records = EvidenceJournal::open(&journal_path)
        .unwrap()
        .read_records()
        .unwrap();

    assert_eq!(summary.observations, 0);
    assert_eq!(summary.records_written, records.len() as u64);
    assert!(
        matches!(&records.first().unwrap().payload, EvidencePayload::SessionStarted { root: scope }
        if *scope == NativePath::from_path(&root.path().canonicalize().unwrap()))
    );
    assert!(matches!(
        &records.last().unwrap().payload,
        EvidencePayload::SessionEnded
    ));
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record.payload, EvidencePayload::Snapshot { .. }))
            .count(),
        2
    );
}

#[test]
fn snapshots_persist_native_bindings_instead_of_only_aggregate_totals() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let existing = root.path().join("existing-file");
    fs::write(&existing, b"measured contents").unwrap();
    let observed_existing = existing.canonicalize().unwrap();
    let journal_path = output.path().join("evidence.jsonl");

    record_for(root.path(), &journal_path, Duration::from_millis(1)).unwrap();
    let records = EvidenceJournal::open(&journal_path)
        .unwrap()
        .read_records()
        .unwrap();

    assert!(records.iter().any(|record| match &record.payload {
        EvidencePayload::SnapshotEntries { entries, .. } => entries.iter().any(|entry| {
            entry.path == NativePath::from_path(&observed_existing)
                && entry.measurement.logical_bytes == 17
        }),
        _ => false,
    }));
}

#[test]
fn large_snapshot_manifests_are_chunked_instead_of_consuming_one_record_per_path() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    for index in 0..300 {
        fs::write(root.path().join(format!("file-{index}")), b"x").unwrap();
    }
    let journal_path = output.path().join("evidence.jsonl");

    record_for(root.path(), &journal_path, Duration::from_millis(1)).unwrap();
    let records = EvidenceJournal::open(&journal_path)
        .unwrap()
        .read_records()
        .unwrap();
    let manifests: Vec<_> = records
        .iter()
        .filter_map(|record| match &record.payload {
            EvidencePayload::SnapshotEntries {
                snapshot_id,
                entries,
            } => Some((*snapshot_id, entries.len())),
            _ => None,
        })
        .collect();

    assert_eq!(manifests.iter().map(|(_, count)| count).sum::<usize>(), 602);
    assert_eq!(manifests.len(), 4);
    assert!(
        records.len() < 20,
        "chunking should keep the journal index small"
    );
}

/// The bounds the windows offer, on the command line: a file outside them is
/// not observed at all, and the things that have no size worth comparing — a
/// new directory, a file already gone by the time it was measured — stay in
/// the journal, because they are what a reader rebuilds the history from.
#[test]
fn a_bounded_recording_observes_only_files_of_the_size_it_was_asked_for() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let journal_path = output.path().join("evidence.jsonl");
    let bounds = SizeBounds {
        min_bytes: Some(1_000_000),
        max_bytes: None,
    };

    record_bounded(
        root.path(),
        &journal_path,
        Duration::from_secs(3),
        bounds,
        || {
            fs::write(root.path().join("tiny.txt"), b"small").unwrap();
            fs::write(root.path().join("large.bin"), vec![7; 2 * 1024 * 1024]).unwrap();
            fs::create_dir(root.path().join("folder")).unwrap();
        },
    )
    .unwrap();
    let records = EvidenceJournal::open(&journal_path)
        .unwrap()
        .read_records()
        .unwrap();
    let observed: Vec<String> = records
        .iter()
        .filter_map(|record| match &record.payload {
            EvidencePayload::Observation { change, .. } => Some(
                Path::new(&change.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        })
        .collect();

    assert!(
        observed.iter().any(|name| name == "large.bin"),
        "{observed:?}"
    );
    assert!(observed.iter().any(|name| name == "folder"), "{observed:?}");
    assert!(
        !observed.iter().any(|name| name == "tiny.txt"),
        "{observed:?}"
    );
}
