use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pathlight_core::{ActivityListener, ChangeKind, StreamEvent, Watcher};

#[derive(Default)]
struct Collector(Mutex<Vec<StreamEvent>>);

impl ActivityListener for Collector {
    fn on_event(&self, event: StreamEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[test]
fn reports_created_file_with_increasing_event_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let collector = Arc::new(Collector::default());
    let watcher = Watcher::start(
        root.to_string_lossy().into_owned(),
        None,
        100,
        collector.clone(),
    )
    .unwrap();

    // Give FSEvents/inotify a moment to arm before producing the change.
    std::thread::sleep(Duration::from_millis(300));
    let file = root.join("hello.txt");
    std::fs::write(&file, b"hi").unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let seen = loop {
        let events = collector.0.lock().unwrap().clone();
        let created = events.iter().find_map(|event| match event {
            StreamEvent::Change { change, event_id }
                if change.path.ends_with("hello.txt") && change.kind != ChangeKind::Deleted =>
            {
                Some((change.clone(), *event_id))
            }
            _ => None,
        });
        if let Some(found) = created {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "no change reported; events: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    watcher.stop();

    let events = collector.0.lock().unwrap();
    assert!(matches!(events[0], StreamEvent::HistoryCaughtUp { .. }));
    assert_eq!(seen.0.root_path, root.to_string_lossy());
    assert!(seen.1 >= 1);
}

#[cfg(target_os = "macos")]
#[test]
fn pairs_rename_halves_by_inode() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let source = root.join("draft.txt");
    std::fs::write(&source, b"draft").unwrap();
    let collector = Arc::new(Collector::default());
    let watcher = Watcher::start(
        root.to_string_lossy().into_owned(),
        None,
        100,
        collector.clone(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let target = root.join("final.txt");
    std::fs::rename(&source, &target).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let previous = loop {
        let events = collector.0.lock().unwrap().clone();
        let arrival = events.iter().find_map(|event| match event {
            StreamEvent::Change { change, .. } if change.path.ends_with("final.txt") => {
                match &change.kind {
                    ChangeKind::Renamed { previous_path } => Some(previous_path.clone()),
                    _ => None,
                }
            }
            _ => None,
        });
        if let Some(found) = arrival {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "no arrival reported; events: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    watcher.stop();

    assert!(
        previous
            .as_deref()
            .is_some_and(|p| p.ends_with("draft.txt")),
        "previous_path was {previous:?}"
    );
}
