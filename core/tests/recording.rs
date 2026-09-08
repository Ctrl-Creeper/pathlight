use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use pathlight_core::evidence::{EvidenceJournal, EvidencePayload, EvidenceRecord, NativePath};
use pathlight_core::monitor::ChangeKind;
use pathlight_core::recording::record_for;

#[test]
fn records_real_changes_and_two_interval_snapshots_to_an_external_journal() {
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let journal_path = output.path().join("evidence.jsonl");
    fs::write(root.path().join("original"), b"retained object").unwrap();
    let writer_root = root.path().to_owned();
    let writer_journal = journal_path.clone();
    let writer = thread::spawn(move || {
        wait_for_first_snapshot(&writer_journal);
        fs::rename(writer_root.join("original"), writer_root.join("renamed")).unwrap();
        fs::write(writer_root.join("added"), vec![1; 4096]).unwrap();
    });

    let summary = record_for(root.path(), &journal_path, Duration::from_secs(3)).unwrap();
    writer.join().unwrap();
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

fn wait_for_first_snapshot(journal: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(journal).is_ok_and(|text| {
            text.lines().any(|line| {
                serde_json::from_str::<EvidenceRecord>(line)
                    .is_ok_and(|record| matches!(record.payload, EvidencePayload::Snapshot { .. }))
            })
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "recorder never wrote its initial snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
