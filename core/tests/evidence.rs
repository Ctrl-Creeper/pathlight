use pathlight_core::evidence::{EvidenceJournal, EvidencePayload, EvidenceRecord, SourceIdentity};
use std::time::{SystemTime, UNIX_EPOCH};

fn record(epoch: &str, sequence: u64) -> EvidenceRecord {
    EvidenceRecord {
        schema_version: 1,
        source: SourceIdentity {
            backend: "inotify".into(),
            epoch: epoch.into(),
        },
        sequence,
        source_cursor: None,
        observed_at: UNIX_EPOCH,
        payload: EvidencePayload::Gap {
            reason: "queue overflow".into(),
        },
    }
}

#[test]
fn replay_deduplicates_across_reopen_but_not_across_source_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("evidence.jsonl");
    {
        let mut journal = EvidenceJournal::open(&path).unwrap();
        assert!(journal.append(record("first", 1)).unwrap());
    }
    let mut journal = EvidenceJournal::open(&path).unwrap();
    assert!(!journal.append(record("first", 1)).unwrap());
    assert!(journal.append(record("second", 1)).unwrap());
    assert_eq!(journal.read_records().unwrap().len(), 2);
}

#[test]
fn conflicting_replay_is_an_error_not_a_silently_discarded_observation() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = EvidenceJournal::open(&dir.path().join("log")).unwrap();
    journal.append(record("epoch", 1)).unwrap();
    let mut conflicting = record("epoch", 1);
    conflicting.payload = EvidencePayload::Gap {
        reason: "permission lost".into(),
    };
    assert_eq!(
        journal.append(conflicting).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn incomplete_or_corrupted_journal_is_rejected_without_repairing_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let text = serde_json::to_string(&record("epoch", 1)).unwrap();
    std::fs::write(&path, text.as_bytes()).unwrap();
    assert!(EvidenceJournal::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
    std::fs::write(&path, b"not-json\n").unwrap();
    assert!(EvidenceJournal::open(&path).is_err());
}

#[test]
fn current_state_measurement_does_not_erase_the_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = EvidenceJournal::open(&dir.path().join("log")).unwrap();
    journal.append(record("epoch", 1)).unwrap();
    let mut snapshot = record("epoch", 2);
    snapshot.payload = EvidencePayload::Snapshot {
        started_at: UNIX_EPOCH,
        finished_at: SystemTime::now(),
        logical_bytes: Some(10),
        allocated_bytes: Some(4096),
        measured_paths: 2,
        measured_objects: Some(2),
        partial: false,
    };
    journal.append(snapshot).unwrap();
    let rows = journal.read_records().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0].payload, EvidencePayload::Gap { .. }));
    assert!(matches!(rows[1].payload, EvidencePayload::Snapshot { .. }));
}

#[test]
fn future_schema_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = EvidenceJournal::open(&dir.path().join("log")).unwrap();
    let mut future = record("epoch", 1);
    future.schema_version = 2;
    assert!(journal.append(future).is_err());
    assert!(journal.read_records().unwrap().is_empty());
}
