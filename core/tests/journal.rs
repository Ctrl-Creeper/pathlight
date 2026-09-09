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

    assert_eq!(journal.trim(180, 0).unwrap(), 1);

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

    let dropped = journal.trim(0, one_row * 3).unwrap();

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

    assert_eq!(journal.trim(180, 0).unwrap(), 1);

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

    assert_eq!(journal.trim(180, 1 << 30).unwrap(), 0);

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
    assert_eq!(unwritten.trim(180, 1 << 30).unwrap(), 0);
}
