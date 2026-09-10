//! What the journal keeps and what it lets go.

use std::fs;
use std::time::{Duration, SystemTime};

use pathlight_core::{ActivityEvent, Confidence, EventKind, Journal};

const ROOT: &str = "/Users/example/Downloads";

fn event(name: &str, days_ago: u64) -> ActivityEvent {
    ActivityEvent {
        kind: EventKind::Modified,
        path: format!("{ROOT}/{name}"),
        root_path: ROOT.into(),
        timestamp: SystemTime::now() - Duration::from_secs(days_ago * 86_400),
        byte_delta: Some(1_024),
        confidence: Confidence::Confirmed,
        previous_path: None,
        affected_item_count: 1,
        process_name: None,
    }
}

fn at(dir: &std::path::Path) -> std::sync::Arc<Journal> {
    Journal::new(
        dir.join("activity-events.jsonl")
            .to_string_lossy()
            .into_owned(),
    )
}

fn lines(dir: &std::path::Path) -> Vec<String> {
    fs::read_to_string(dir.join("activity-events.jsonl"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn rows_past_the_retention_window_are_dropped_and_the_rest_stay_readable() {
    let dir = tempfile::tempdir().unwrap();
    let journal = at(dir.path());
    journal
        .append(vec![event("old.bin", 400), event("recent.bin", 3)])
        .unwrap();

    assert_eq!(journal.trim(180, 180, 0).unwrap(), 1);

    let kept = journal.load(ROOT.to_owned(), 100).unwrap();
    assert_eq!(kept.len(), 1, "kept: {kept:#?}");
    assert!(kept[0].path.ends_with("recent.bin"));
    // Trimming rewrites the file, so the survivor has to still parse as a row
    // rather than as a line that merely looks like one.
    assert_eq!(lines(dir.path()).len(), 1);
}

#[test]
fn a_cap_drops_the_oldest_until_the_file_fits() {
    let dir = tempfile::tempdir().unwrap();
    let journal = at(dir.path());
    journal
        .append(
            (0..10)
                .map(|n| event(&format!("f{n}.bin"), 10 - n))
                .collect(),
        )
        .unwrap();
    let one_row = lines(dir.path())[0].len() as u64 + 1;

    let dropped = journal.trim(0, 0, one_row * 3).unwrap();

    assert_eq!(dropped, 7);
    let kept = journal.load(ROOT.to_owned(), 100).unwrap();
    assert_eq!(kept.len(), 3);
    // The newest three, because the oldest are the ones that go.
    assert!(kept.iter().any(|row| row.path.ends_with("f9.bin")));
    assert!(!kept.iter().any(|row| row.path.ends_with("f0.bin")));
}

/// A row this build cannot date must not be aged out on a guess — the key for
/// an encrypted line lives in the macOS Keychain, and a Linux host reading the
/// same file cannot read the timestamp inside it.
#[test]
fn a_row_this_build_cannot_read_is_not_aged_out() {
    let dir = tempfile::tempdir().unwrap();
    let journal = at(dir.path());
    journal.append(vec![event("old.bin", 400)]).unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join("activity-events.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut file, b"pathlight:v1:aes-gcm:ciphertext\n").unwrap();
    drop(file);

    assert_eq!(journal.trim(180, 180, 0).unwrap(), 1);

    assert_eq!(lines(dir.path()), ["pathlight:v1:aes-gcm:ciphertext"]);
}

#[test]
fn nothing_to_drop_leaves_the_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let journal = at(dir.path());
    journal.append(vec![event("recent.bin", 1)]).unwrap();
    let before = fs::metadata(dir.path().join("activity-events.jsonl"))
        .unwrap()
        .modified()
        .unwrap();

    assert_eq!(journal.trim(180, 180, 1 << 30).unwrap(), 0);

    assert_eq!(
        fs::metadata(dir.path().join("activity-events.jsonl"))
            .unwrap()
            .modified()
            .unwrap(),
        before,
        "an unnecessary rewrite is a write the watch on this folder would record"
    );
    // A journal with no file yet is the first-run state, not an error.
    let empty = tempfile::tempdir().unwrap();
    let unwritten = at(empty.path());
    assert_eq!(unwritten.trim(180, 180, 1 << 30).unwrap(), 0);
}

/// The whole point of the setting: what lands on disk is not readable by
/// looking at it, and Pathlight itself still reads it back whole.
#[test]
fn encrypted_rows_are_unreadable_on_disk_and_readable_through_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("activity-events.jsonl")
        .to_string_lossy()
        .into_owned();
    let journal = Journal::encrypting(path.clone());
    journal.append(vec![event("secret.bin", 1)]).unwrap();

    let written = lines(dir.path());
    assert_eq!(written.len(), 1);
    assert!(
        written[0].starts_with("pathlight:v1:aes-gcm:"),
        "{written:?}"
    );
    assert!(!written[0].contains("secret.bin"), "{written:?}");

    // Read back by a journal that was never told about encryption: the marker
    // on the line is what says how to read it, so turning the setting off
    // does not hide what was already recorded.
    let events = Journal::new(path).load(ROOT.to_owned(), 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].path, format!("{ROOT}/secret.bin"));

    // And retention still applies to a row it can read, which is what would
    // silently stop working if encrypted rows were only ever skipped.
    assert_eq!(journal.trim(180, 180, 0).unwrap(), 0);
    journal.append(vec![event("old.bin", 400)]).unwrap();
    assert_eq!(journal.trim(180, 180, 0).unwrap(), 1);
}

/// Without the key the rows stay on disk and stay unread. A journal copied
/// off this machine is the case the setting exists for.
#[test]
fn a_journal_without_its_key_reads_nothing_and_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("activity-events.jsonl")
        .to_string_lossy()
        .into_owned();
    Journal::encrypting(path.clone())
        .append(vec![event("secret.bin", 400)])
        .unwrap();
    fs::remove_file(dir.path().join("activity-events.key")).unwrap();

    let journal = Journal::new(path);
    assert!(journal.load(ROOT.to_owned(), 10).unwrap().is_empty());
    assert_eq!(
        journal.trim(180, 180, 0).unwrap(),
        0,
        "a row it cannot date was aged out on a guess"
    );
}

/// The long tail: a grouped row says how much a folder changed and outlives
/// the file-level rows it was made of, so a year-old total survives a
/// retention that the list of files does not.
#[test]
fn a_grouped_row_outlives_the_file_rows_it_was_made_of() {
    let dir = tempfile::tempdir().unwrap();
    let journal = at(dir.path());
    let mut grouped = event("", 400);
    grouped.kind = EventKind::Aggregate;
    grouped.path = format!("{ROOT}/");
    journal
        .append(vec![event("old.bin", 400), grouped, event("recent.bin", 3)])
        .unwrap();

    assert_eq!(journal.trim(180, 730, 0).unwrap(), 1);
    let kept = journal.load(ROOT.to_owned(), 100).unwrap();
    assert_eq!(kept.len(), 2, "kept: {kept:#?}");
    assert!(kept.iter().any(|row| row.kind == EventKind::Aggregate));
    assert!(kept.iter().any(|row| row.path.ends_with("recent.bin")));

    // And a grouped row past its own retention goes like anything else: this
    // is a longer window, not an exemption.
    assert_eq!(journal.trim(180, 365, 0).unwrap(), 1);
    assert_eq!(journal.load(ROOT.to_owned(), 100).unwrap().len(), 1);
}
