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
    let watcher = Watcher::start(root.to_string_lossy().into_owned(), 100, collector.clone()).unwrap();

    // Give FSEvents/inotify a moment to arm before producing the change.
    std::thread::sleep(Duration::from_millis(300));
    let file = root.join("hello.txt");
    std::fs::write(&file, b"hi").unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let seen = loop {
        let events = collector.0.lock().unwrap().clone();
        let created = events.iter().find_map(|event| match event {
            StreamEvent::Change { change, event_id }
                if change.path == file.to_string_lossy() && change.kind != ChangeKind::Deleted =>
            {
                Some((change.clone(), *event_id))
            }
            _ => None,
        });
        if let Some(found) = created {
            break found;
        }
        assert!(Instant::now() < deadline, "no change reported; events: {events:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    watcher.stop();

    let events = collector.0.lock().unwrap();
    assert!(matches!(events[0], StreamEvent::HistoryCaughtUp { .. }));
    assert_eq!(seen.0.root_path, root.to_string_lossy());
    assert!(seen.1 > 1);
}
