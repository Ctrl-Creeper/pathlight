use std::time::{Duration, UNIX_EPOCH};

use pathlight_core::history::{build_history, Query};
use pathlight_core::{ActivityEvent, Confidence, EventKind};

const ROOT: &str = "/Users/example/Downloads";

fn event(name: &str, seconds: u64, delta: Option<i64>) -> ActivityEvent {
    ActivityEvent {
        kind: EventKind::Modified,
        path: format!("{ROOT}/{name}"),
        root_path: ROOT.into(),
        timestamp: UNIX_EPOCH + Duration::from_secs(seconds),
        byte_delta: delta,
        confidence: if delta.is_some() {
            Confidence::Confirmed
        } else {
            Confidence::Unknown
        },
        previous_path: None,
        affected_item_count: 1,
        process_name: None,
    }
}

#[test]
fn buckets_totals_and_newest_first_ordering() {
    let events = vec![
        event("a", 10, Some(100)),
        event("b", 3_650, Some(-40)),
        event("c", 3_700, None),
    ];
    let history = build_history(
        &format!("{ROOT}/"),
        events,
        3_600,
        100,
        &Query::default(),
        UNIX_EPOCH,
    );

    assert_eq!(history.root_path, ROOT);
    assert_eq!(history.total_net_byte_delta, 60);
    assert_eq!(history.event_count, 3);
    assert_eq!(history.unknown_size_event_count, 1);
    assert_eq!(history.buckets.len(), 2);
    assert_eq!(history.buckets[0].start, UNIX_EPOCH);
    assert_eq!(history.buckets[0].byte_delta, 100);
    assert_eq!(
        history.buckets[1].start,
        UNIX_EPOCH + Duration::from_secs(3_600)
    );
    assert_eq!(history.buckets[1].event_count, 2);
    assert_eq!(history.buckets[1].unknown_size_event_count, 1);
    let names: Vec<&str> = history
        .recent_events
        .iter()
        .map(|e| e.path.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(names, ["c", "b", "a"]);
}

/// The page limit cuts the rows a caller lists, never the numbers: totalling
/// only the newest page understates the answer by the ratio between the
/// journal and the page, which is how a full disk reads as a quiet one.
#[test]
fn totals_cover_every_event_while_listing_one_page() {
    let events: Vec<ActivityEvent> = (0..10)
        .map(|i| event(&format!("f{i}"), i, Some(10)))
        .collect();

    let history = build_history(ROOT, events, 3_600, 3, &Query::default(), UNIX_EPOCH);

    assert_eq!(history.event_count, 10);
    assert_eq!(history.total_net_byte_delta, 100);
    assert_eq!(history.buckets[0].event_count, 10);
    assert_eq!(history.recent_events.len(), 3);
    assert!(history.is_truncated);
}

/// The one place the two aggregations meet.
///
/// macOS folds its own journal in Swift (`ActivityHistoryService`) because the
/// journal is Swift's and does not cross the FFI, so the same rule is written
/// twice. The fixture and its expectations are shared with
/// `PathlightCoreTests/ActivityHistoryServiceTests`: edit one implementation's
/// bucket boundary, ordering or unknown-size rule and the other host's test
/// fails, which is the only thing standing between two dashboards and two
/// different answers for one folder.
#[test]
fn both_hosts_fold_the_same_rows_into_the_same_history() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/history-expectations.json")).unwrap();
    let events: Vec<ActivityEvent> = include_str!("../fixtures/swift-journal.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| ActivityEvent::from_json_line(line).unwrap())
        .collect();

    let history = build_history(
        ROOT,
        events,
        expected["bucketIntervalSecs"].as_u64().unwrap(),
        expected["recentLimit"].as_u64().unwrap() as u32,
        &Query::default(),
        UNIX_EPOCH,
    );

    assert_eq!(
        history.total_net_byte_delta,
        expected["totalNetByteDelta"].as_i64().unwrap()
    );
    assert_eq!(
        u64::from(history.event_count),
        expected["eventCount"].as_u64().unwrap()
    );
    assert_eq!(
        u64::from(history.unknown_size_event_count),
        expected["unknownSizeEventCount"].as_u64().unwrap()
    );
    assert_eq!(
        history.is_truncated,
        expected["isTruncated"].as_bool().unwrap()
    );
    let paths: Vec<&str> = history
        .recent_events
        .iter()
        .map(|event| event.path.as_str())
        .collect();
    let wanted: Vec<&str> = expected["recentPaths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect();
    assert_eq!(paths, wanted);

    let buckets = expected["buckets"].as_array().unwrap();
    assert_eq!(history.buckets.len(), buckets.len());
    for (bucket, wanted) in history.buckets.iter().zip(buckets) {
        assert_eq!(
            bucket.start,
            UNIX_EPOCH + Duration::from_secs(wanted["startSecs"].as_u64().unwrap()),
        );
        assert_eq!(bucket.byte_delta, wanted["byteDelta"].as_i64().unwrap());
        assert_eq!(
            u64::from(bucket.event_count),
            wanted["eventCount"].as_u64().unwrap()
        );
        assert_eq!(
            u64::from(bucket.unknown_size_event_count),
            wanted["unknownSizeEventCount"].as_u64().unwrap()
        );
    }
}

/// Searching is the other half of that contract: a filtered view's totals have
/// to cover what matched, not what the page lists, or the number under a search
/// box means nothing.
#[test]
fn both_hosts_narrow_the_same_rows_the_same_way() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/history-expectations.json")).unwrap();
    let search = &expected["search"];
    let events: Vec<ActivityEvent> = include_str!("../fixtures/swift-journal.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| ActivityEvent::from_json_line(line).unwrap())
        .collect();

    let history = build_history(
        ROOT,
        events,
        expected["bucketIntervalSecs"].as_u64().unwrap(),
        search["limit"].as_u64().unwrap() as u32,
        &Query {
            text: search["text"].as_str().unwrap().to_owned(),
            kind: None,
            largest_first: search["largestFirst"].as_bool().unwrap(),
            skip: search["skip"].as_u64().unwrap() as u32,
        },
        UNIX_EPOCH,
    );

    assert_eq!(
        u64::from(history.event_count),
        search["eventCount"].as_u64().unwrap()
    );
    assert_eq!(
        history.total_net_byte_delta,
        search["totalNetByteDelta"].as_i64().unwrap()
    );
    assert_eq!(
        u64::from(history.unknown_size_event_count),
        search["unknownSizeEventCount"].as_u64().unwrap()
    );
    assert_eq!(
        history.is_truncated,
        search["isTruncated"].as_bool().unwrap()
    );
    let paths: Vec<&str> = history
        .recent_events
        .iter()
        .map(|event| event.path.as_str())
        .collect();
    let wanted: Vec<&str> = search["recentPaths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect();
    assert_eq!(paths, wanted);
}

/// One kind of change, which is how "what did it delete" gets answered.
#[test]
fn one_kind_of_change_can_be_asked_for_on_its_own() {
    let mut deleted = event("gone.psd", 20, Some(-4_000));
    deleted.kind = EventKind::Deleted;
    let events = vec![event("kept.psd", 10, Some(1_000)), deleted];

    let history = build_history(
        ROOT,
        events,
        3_600,
        10,
        &Query {
            kind: Some(EventKind::Deleted),
            ..Query::default()
        },
        UNIX_EPOCH,
    );

    assert_eq!(history.event_count, 1);
    assert_eq!(history.total_net_byte_delta, -4_000);
    assert_eq!(history.recent_events[0].path, format!("{ROOT}/gone.psd"));
}
