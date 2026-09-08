use pathlight_core::evidence::{
    EvidenceJournal, EvidencePayload, EvidenceRecord, NativePath, SourceIdentity,
    CURRENT_SCHEMA_VERSION,
};
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
        snapshot_id: 1,
        root: NativePath::Utf8("/watched".into()),
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
    future.schema_version = 3;
    assert!(journal.append(future).is_err());
    assert!(journal.read_records().unwrap().is_empty());
}

#[test]
fn schema_two_rejects_missing_native_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = EvidenceJournal::open(&dir.path().join("log")).unwrap();
    let mut invalid = record("epoch", 1);
    invalid.schema_version = CURRENT_SCHEMA_VERSION;
    invalid.payload = EvidencePayload::Snapshot {
        snapshot_id: 1,
        root: NativePath::default(),
        started_at: UNIX_EPOCH,
        finished_at: UNIX_EPOCH,
        logical_bytes: Some(0),
        allocated_bytes: Some(0),
        measured_paths: 0,
        measured_objects: Some(0),
        partial: false,
    };

    assert!(journal.append(invalid).is_err());
}

#[test]
fn schema_one_rejects_schema_two_payload_variants() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = EvidenceJournal::open(&dir.path().join("log")).unwrap();
    let mut invalid = record("epoch", 1);
    invalid.payload = EvidencePayload::SnapshotStarted {
        snapshot_id: 1,
        root: NativePath::Utf8("/watched".into()),
    };

    assert!(journal.append(invalid).is_err());
}

#[test]
fn legacy_v1_snapshot_without_native_manifest_fields_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    std::fs::write(
        &path,
        concat!(
            r#"{"schema_version":1,"source":{"backend":"fsevents","epoch":"old"},"sequence":1,"source_cursor":null,"observed_at":{"secs_since_epoch":0,"nanos_since_epoch":0},"payload":{"type":"snapshot","started_at":{"secs_since_epoch":0,"nanos_since_epoch":0},"finished_at":{"secs_since_epoch":1,"nanos_since_epoch":0},"logical_bytes":10,"allocated_bytes":4096,"measured_paths":1,"measured_objects":1,"partial":false}}"#,
            "\n"
        ),
    )
    .unwrap();

    let records = EvidenceJournal::open(&path)
        .unwrap()
        .read_records()
        .unwrap();
    assert_eq!(records.len(), 1);
    assert!(matches!(
        &records[0].payload,
        EvidencePayload::Snapshot {
            snapshot_id: 0,
            root: NativePath::Utf8(root),
            ..
        } if root.is_empty()
    ));
}

#[test]
fn batch_validation_prevents_a_conflicting_duplicate_from_partially_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut journal = EvidenceJournal::open(&path).unwrap();
    let first = record("epoch", 1);
    let mut conflict = first.clone();
    conflict.payload = EvidencePayload::Gap {
        reason: "different evidence".into(),
    };

    assert!(journal.append_batch(vec![first, conflict]).is_err());
    assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
}

#[test]
fn a_second_writer_is_rejected_until_the_first_releases_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let first = EvidenceJournal::open(&path).unwrap();
    assert!(EvidenceJournal::open(&path).is_err());
    drop(first);
    assert!(EvidenceJournal::open(&path).is_ok());
}

#[cfg(unix)]
#[test]
fn native_paths_round_trip_non_utf8_bytes_without_display_conversion() {
    use std::os::unix::ffi::OsStrExt;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/root/file-\xff"));
    let native = NativePath::from_path(path);
    let encoded = serde_json::to_vec(&native).unwrap();

    assert_eq!(native, NativePath::UnixBytes(b"/root/file-\xff".to_vec()));
    assert_eq!(
        serde_json::from_slice::<NativePath>(&encoded).unwrap(),
        native
    );
}
