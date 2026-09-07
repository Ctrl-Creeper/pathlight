//! Folder watcher mirroring Swift's `DiskActivityMonitoring`: a stream of
//! changes tagged with an event ID plus history/rescan markers.
//!
//! macOS uses FSEvents directly (real event IDs, resumable via `since_event_id`);
//! other platforms use `notify` (inotify / ReadDirectoryChangesW) with a
//! process-local counter.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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

/// What a platform's watcher backend actually guarantees.
///
/// Backends are not equal, and papering over that costs accuracy: an
/// intersection of features would throw away FSEvents' resumable IDs just to
/// match inotify. So each backend declares what it can do, the host adapts to
/// the declaration, and `tests/monitor.rs` checks the declaration against
/// observed behavior. A platform is allowed to differ; it is not allowed to be
/// wrong about how it differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Capabilities {
    /// Event IDs come from the kernel, so `since_event_id` really resumes a
    /// previous session instead of silently starting over.
    pub resumable_cursor: bool,
    /// One user-visible rename arrives as one event carrying both paths. When
    /// false the host sees two halves and must pair them itself.
    pub pairs_renames: bool,
    /// Changes name the process responsible. When false the host should show
    /// nothing rather than a guess.
    pub reports_process: bool,
    /// Events can be lost under load. The backend reports it with
    /// `RequiresRescan`, so the host re-baselines rather than under-count.
    pub may_drop_events: bool,
}

/// What this platform's watcher guarantees. Safe to call before starting a
/// watch, which is the point: the host needs it to decide whether a stored
/// cursor is worth trusting.
#[uniffi::export]
pub fn watcher_capabilities() -> Capabilities {
    platform::CAPABILITIES
}

pub(crate) trait Backend: Send {
    fn stop(&mut self);
}

/// Shared by backends: knows the root and forwards to the host listener.
pub(crate) struct Emitter {
    pub root: String,
    listener: Arc<dyn ActivityListener>,
}

impl Emitter {
    pub fn emit(&self, event: StreamEvent) {
        self.listener.on_event(event);
    }

    pub fn change(&self, kind: ChangeKind, path: String, event_id: u64) {
        self.emit(StreamEvent::Change {
            change: Change {
                kind,
                path: crate::paths::normalize(&path),
                root_path: self.root.clone(),
                timestamp: SystemTime::now(),
            },
            event_id,
        });
    }
}

#[derive(uniffi::Object)]
pub struct Watcher {
    inner: Mutex<Option<Box<dyn Backend>>>,
}

#[uniffi::export]
impl Watcher {
    /// Starts watching `root_path` recursively.
    ///
    /// `since_event_id` resumes from a previous cursor where the platform supports
    /// it (FSEvents); elsewhere a `RequiresRescan` is emitted so the host
    /// re-baselines. `latency_ms` is the coalescing window handed to the kernel.
    #[uniffi::constructor]
    pub fn start(
        root_path: String,
        since_event_id: Option<u64>,
        latency_ms: u64,
        listener: Arc<dyn ActivityListener>,
    ) -> Result<Arc<Self>, CoreError> {
        let root = crate::paths::normalize(&root_path);
        let emitter = Arc::new(Emitter { root, listener });
        let latency = Duration::from_millis(latency_ms.max(1));
        let backend = platform::start(emitter, since_event_id, latency)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Some(backend)),
        }))
    }

    pub fn stop(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(mut backend) = guard.take() {
                backend.stop();
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub(crate) use crate::fsevents::start;

    /// FSEvents hands out kernel event IDs and `fsevents.rs` pairs rename halves
    /// by inode. It never names a process, and it drops events when its queue
    /// overflows (reported as `MustScanSubDirs`).
    pub(crate) const CAPABILITIES: super::Capabilities = super::Capabilities {
        resumable_cursor: true,
        pairs_renames: true,
        reports_process: false,
        may_drop_events: true,
    };
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use notify::event::{ModifyKind, RenameMode};
    use notify::{
        Config, EventKind as NotifyKind, RecommendedWatcher, RecursiveMode, Watcher as _,
    };

    use super::{Backend, ChangeKind, Emitter, StreamEvent};
    use crate::CoreError;

    /// inotify and ReadDirectoryChangesW both give a process-local counter, not a
    /// cursor that survives a restart, and neither pairs rename halves for us
    /// (the `RenameMode::Both` arm below is opportunistic). Queue overflow is
    /// reported, so `RequiresRescan` is reachable.
    pub(crate) const CAPABILITIES: super::Capabilities = super::Capabilities {
        resumable_cursor: false,
        pairs_renames: false,
        reports_process: false,
        may_drop_events: true,
    };

    struct NotifyBackend(Option<RecommendedWatcher>);

    impl Backend for NotifyBackend {
        fn stop(&mut self) {
            self.0.take();
        }
    }

    pub fn start(
        emitter: Arc<Emitter>,
        since_event_id: Option<u64>,
        latency: Duration,
    ) -> Result<Box<dyn Backend>, CoreError> {
        // ponytail: counter, not inotify cookies / USN numbers; resume-after-restart
        // needs a platform backend that exposes the kernel's own cursor.
        let next_id = Arc::new(AtomicU64::new(1));
        let handler_emitter = Arc::clone(&emitter);
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                let event_id = next_id.fetch_add(1, Ordering::Relaxed);
                forward(&handler_emitter, result, event_id);
            },
            Config::default().with_poll_interval(latency),
        )?;
        watcher.watch(Path::new(&emitter.root), RecursiveMode::Recursive)?;

        if since_event_id.is_some() {
            emitter.emit(StreamEvent::RequiresRescan { event_id: 0 });
        }
        emitter.emit(StreamEvent::HistoryCaughtUp { event_id: 0 });
        Ok(Box::new(NotifyBackend(Some(watcher))))
    }

    fn forward(emitter: &Emitter, result: notify::Result<notify::Event>, event_id: u64) {
        let event = match result {
            Ok(event) => event,
            // A backend error means events may have been lost; the host re-baselines.
            Err(_) => return emitter.emit(StreamEvent::RequiresRescan { event_id }),
        };
        if event.need_rescan() {
            return emitter.emit(StreamEvent::RequiresRescan { event_id });
        }
        let lossy = |path: &Path| path.to_string_lossy().into_owned();
        match event.kind {
            NotifyKind::Access(_) => {}
            NotifyKind::Create(_) => event
                .paths
                .iter()
                .for_each(|p| emitter.change(ChangeKind::Created, lossy(p), event_id)),
            NotifyKind::Remove(_) => event
                .paths
                .iter()
                .for_each(|p| emitter.change(ChangeKind::Deleted, lossy(p), event_id)),
            NotifyKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                emitter.change(
                    ChangeKind::Renamed {
                        previous_path: Some(lossy(&event.paths[0])),
                    },
                    lossy(&event.paths[1]),
                    event_id,
                )
            }
            NotifyKind::Modify(ModifyKind::Name(_)) => event.paths.iter().for_each(|p| {
                emitter.change(
                    ChangeKind::Renamed {
                        previous_path: None,
                    },
                    lossy(p),
                    event_id,
                )
            }),
            NotifyKind::Modify(_) | NotifyKind::Any | NotifyKind::Other => event
                .paths
                .iter()
                .for_each(|p| emitter.change(ChangeKind::Modified, lossy(p), event_id)),
        }
    }
}
