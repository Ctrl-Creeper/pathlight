//! Folder watcher mirroring Swift's `DiskActivityMonitoring`: a stream of
//! changes tagged with an event ID plus history/rescan markers.
//!
//! macOS uses FSEvents directly (real event IDs, resumable via `since_event_id`);
//! other platforms use `notify` (inotify / ReadDirectoryChangesW) with a
//! process-local counter.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::CoreError;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Enum)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed { previous_path: Option<String> },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, uniffi::Record)]
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
    /// Pairs observed rename halves within the backend's matching window.
    /// Moves across the root boundary, expired halves, and gaps can still be
    /// unpaired. When false the host must pair even ordinary in-root renames.
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
    /// re-baselines. `latency_ms` is the FSEvents coalescing window; inotify
    /// delivers immediately and waits up to 250 ms only for unmatched renames.
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
        let backend = self.inner.lock().ok().and_then(|mut guard| guard.take());
        // Stopping may join a worker that is calling the listener. Release the
        // mutex first so a concurrent listener stop cannot deadlock that join.
        if let Some(mut backend) = backend {
            backend.stop();
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
    pub(crate) use crate::notify_backend::{start, CAPABILITIES};
}
