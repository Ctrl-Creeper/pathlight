//! The fixture lines were produced by Swift's `JSONEncoder` with
//! `.iso8601` dates (see `core/fixtures/swift-journal.jsonl`). Rust must read
//! them and write structurally identical JSON.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pathlight_core::{ActivityEvent, Confidence, EventKind, Journal};
use serde_json::Value;

const FIXTURE: &str = include_str!("../fixtures/swift-journal.jsonl");

fn fixture_lines() -> Vec<&'static str> {
    FIXTURE.lines().filter(|line| !line.is_empty()).collect()
}

fn unix(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn parses_swift_written_rows() {
    let lines = fixture_lines();
    let moved = ActivityEvent::from_json_line(lines[0]).unwrap();
    assert_eq!(moved.kind, EventKind::Moved);
    assert_eq!(moved.path, "/Users/example/Downloads/b.dmg");
    assert_eq!(moved.root_path, "/Users/example/Downloads");
    assert_eq!(moved.previous_path.as_deref(), Some("/Users/example/Downloads/a.dmg"));
    assert_eq!(moved.byte_delta, Some(-4096));
    assert_eq!(moved.confidence, Confidence::Estimated);
    assert_eq!(moved.timestamp, unix(1_757_226_000));

    let aggregate = ActivityEvent::from_json_line(lines[1]).unwrap();
    assert_eq!(aggregate.kind, EventKind::Aggregate);
    assert_eq!(aggregate.path, "/Users/example/Downloads");
    assert_eq!(aggregate.byte_delta, None);
    assert_eq!(aggregate.previous_path, None);
    assert_eq!(aggregate.affected_item_count, 12);
}

#[test]
fn writes_rows_structurally_identical_to_swift() {
    for line in fixture_lines() {
        let event = ActivityEvent::from_json_line(line).unwrap();
        let ours: Value = serde_json::from_str(&event.to_json_line().unwrap()).unwrap();
        let theirs: Value = serde_json::from_str(line).unwrap();
        assert_eq!(ours, theirs, "line: {line}");
    }
}

#[test]
fn encodes_spaces_like_swift_urls() {
    let event = ActivityEvent {
        kind: EventKind::Created,
        path: "/Users/me/My Files/report final.pdf".into(),
        root_path: "/Users/me/My Files".into(),
        timestamp: unix(1_757_226_000),
        byte_delta: Some(10),
        confidence: Confidence::Confirmed,
        previous_path: None,
        affected_item_count: 1,
    };
    let json: Value = serde_json::from_str(&event.to_json_line().unwrap()).unwrap();
    assert_eq!(json["path"], "file:///Users/me/My%20Files/report%20final.pdf");
    assert_eq!(json["rootPath"], "file:///Users/me/My%20Files/");
    assert_eq!(json["kind"], serde_json::json!({"created": {}}));
    assert!(json.get("byteDelta").is_some() && json.get("previousPath").is_none());
    assert_eq!(ActivityEvent::from_json_line(&event.to_json_line().unwrap()).unwrap(), event);
}

#[test]
fn journal_round_trips_and_orders_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("Pathlight").join("activity-events.jsonl");
    let journal = Journal::new(journal_path.to_string_lossy().into_owned());

    let root = "/Users/example/Downloads";
    let make = |path: &str, seconds: u64| ActivityEvent {
        kind: EventKind::Modified,
        path: format!("{root}/{path}"),
        root_path: root.into(),
        timestamp: unix(seconds),
        byte_delta: Some(1),
        confidence: Confidence::Confirmed,
        previous_path: None,
        affected_item_count: 1,
    };
    journal.append(vec![make("old.txt", 100), make("new.txt", 200)]).unwrap();
    journal.append(vec![make("other-root.txt", 300).with_root("/elsewhere")]).unwrap();

    // Swift-written and encrypted rows coexist in the same file.
    let mut contents = std::fs::read_to_string(&journal_path).unwrap();
    contents.push_str(fixture_lines()[0]);
    contents.push_str("\npathlight:v1:aes-gcm:AAAA\n");
    std::fs::write(&journal_path, contents).unwrap();

    let loaded = journal.load(format!("{root}/"), 10).unwrap();
    let paths: Vec<&str> = loaded.iter().map(|event| event.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/Users/example/Downloads/b.dmg",
            "/Users/example/Downloads/new.txt",
            "/Users/example/Downloads/old.txt",
        ]
    );
    assert_eq!(journal.load(root.into(), 1).unwrap().len(), 1);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&journal_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

trait WithRoot {
    fn with_root(self, root: &str) -> Self;
}

impl WithRoot for ActivityEvent {
    fn with_root(mut self, root: &str) -> Self {
        self.root_path = root.into();
        self
    }
}
