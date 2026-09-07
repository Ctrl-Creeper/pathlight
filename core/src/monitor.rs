//! Folder watcher built on `notify` (FSEvents / inotify / ReadDirectoryChangesW).
//!
//! Mirrors Swift's `DiskActivityMonitoring`: a stream of changes tagged with a
//! monotonically increasing event ID, plus history/rescan markers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use notify::event::{ModifyKind, RenameMode};
use notify::{Config, EventKind as NotifyKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::CoreError;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed { previous_path: Option<String> },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: String,
    pub root_path: String,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum StreamEvent {
    Change { change: Change, event_id: u64 },
    HistoryCaughtUp { event_id: u64 },
    RequiresRescan { event_id: u64 },
}

/// Implemented by the host (Swift/Kotlin). Called from the watcher's thread.
#[uniffi::export(with_foreign)]
pub trait ActivityListener: Send + Sync {
    fn on_event(&self, event: StreamEvent);
}

#[derive(uniffi::Object)]
pub struct Watcher {
    inner: Mutex<Option<RecommendedWatcher>>,
}

#[uniffi::export]
impl Watcher {
    /// Starts watching `root_path` recursively. `latency_ms` is the coalescing
    /// window handed to the kernel backend where supported (FSEvents).
    #[uniffi::constructor]
    pub fn start(
        root_path: String,
        latency_ms: u64,
        listener: Arc<dyn ActivityListener>,
    ) -> Result<Arc<Self>, CoreError> {
        let root = PathBuf::from(root_path.trim_end_matches('/'));
        let root_string = root.to_string_lossy().into_owned();
        // ponytail: process-local counter, not FSEvents/USN IDs; resume-after-restart
        // needs a per-platform backend that exposes the kernel's own cursor.
        let next_id = Arc::new(AtomicU64::new(1));

        let handler_listener = Arc::clone(&listener);
        let handler_ids = Arc::clone(&next_id);
        let handler_root = root_string.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                let event_id = handler_ids.fetch_add(1, Ordering::Relaxed);
                for event in map_event(result, &handler_root, event_id) {
                    handler_listener.on_event(event);
                }
            },
            Config::default().with_poll_interval(Duration::from_millis(latency_ms.max(1))),
        )?;
        watcher.watch(&root, RecursiveMode::Recursive)?;

        // notify has no history replay, so the live stream starts immediately.
        listener.on_event(StreamEvent::HistoryCaughtUp {
            event_id: next_id.fetch_add(1, Ordering::Relaxed),
        });

        Ok(Arc::new(Self { inner: Mutex::new(Some(watcher)) }))
    }

    pub fn stop(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.take();
        }
    }
}

fn map_event(result: notify::Result<notify::Event>, root: &str, event_id: u64) -> Vec<StreamEvent> {
    let event = match result {
        Ok(event) => event,
        // A backend error means events may have been lost; the host re-baselines.
        Err(_) => return vec![StreamEvent::RequiresRescan { event_id }],
    };
    if event.need_rescan() {
        return vec![StreamEvent::RequiresRescan { event_id }];
    }

    let timestamp = SystemTime::now();
    let change = |kind: ChangeKind, path: &Path| StreamEvent::Change {
        change: Change {
            kind,
            path: path.to_string_lossy().into_owned(),
            root_path: root.to_owned(),
            timestamp,
        },
        event_id,
    };

    match event.kind {
        NotifyKind::Access(_) => Vec::new(),
        NotifyKind::Create(_) => event.paths.iter().map(|p| change(ChangeKind::Created, p)).collect(),
        NotifyKind::Remove(_) => event.paths.iter().map(|p| change(ChangeKind::Deleted, p)).collect(),
        NotifyKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
            let previous = event.paths[0].to_string_lossy().into_owned();
            vec![change(ChangeKind::Renamed { previous_path: Some(previous) }, &event.paths[1])]
        }
        NotifyKind::Modify(ModifyKind::Name(_)) => event
            .paths
            .iter()
            .map(|p| change(ChangeKind::Renamed { previous_path: None }, p))
            .collect(),
        NotifyKind::Modify(_) | NotifyKind::Any | NotifyKind::Other => {
            event.paths.iter().map(|p| change(ChangeKind::Modified, p)).collect()
        }
    }
}
