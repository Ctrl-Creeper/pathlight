#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) use crate::inotify::start;

use std::path::Path;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::sync::Arc;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::time::Duration;

use notify::event::{ModifyKind, RenameMode};
use notify::EventKind as NotifyKind;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher as _};

#[cfg(not(any(target_os = "linux", target_os = "android")))]
use crate::monitor::Backend;
use crate::monitor::{ChangeKind, Emitter, StreamEvent};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use crate::CoreError;

/// inotify pairs observed rename halves by cookie; the Windows adapter still
/// forwards separate halves. Neither source provides a persistent cursor.
/// Overflow and pairing-buffer exhaustion require a new baseline.
pub(crate) const CAPABILITIES: super::Capabilities = super::Capabilities {
    resumable_cursor: false,
    pairs_renames: cfg!(any(target_os = "linux", target_os = "android")),
    reports_process: false,
    may_drop_events: true,
};

#[cfg(not(any(target_os = "linux", target_os = "android")))]
struct NotifyBackend(Option<RecommendedWatcher>);

#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl Backend for NotifyBackend {
    fn stop(&mut self) {
        self.0.take();
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
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

pub(crate) fn forward(emitter: &Emitter, result: notify::Result<notify::Event>, event_id: u64) {
    let event = match result {
        Ok(event) => event,
        // A backend error means events may have been lost; the host re-baselines.
        Err(_) => return emitter.emit(StreamEvent::RequiresRescan { event_id }),
    };
    if event.need_rescan() {
        return emitter.emit(StreamEvent::RequiresRescan { event_id });
    }
    // Legacy UniFFI changes use String. Never turn an unrepresentable native
    // name into another file's replacement-character spelling. Native scans
    // and evidence keep the exact path; this stream reports its coverage gap.
    if event.paths.iter().any(|path| path.to_str().is_none()) {
        return emitter.emit(StreamEvent::RequiresRescan { event_id });
    }
    let exact = |path: &Path| path.to_str().expect("validated above").to_owned();
    // inotify's DELETE_SELF and MOVE_SELF name the watch root itself, and the
    // kernel drops the watch right after: the stream stays open and silent
    // while the host still believes it is watching. That is what FSEvents'
    // RootChanged reports on macOS, so report it the same way here instead of
    // filing it as one ordinary change inside a healthy watch.
    let names_root = |path: &Path| {
        path.to_str()
            .is_some_and(|path| crate::paths::normalize(path) == emitter.root)
    };
    if matches!(
        event.kind,
        NotifyKind::Remove(_) | NotifyKind::Modify(ModifyKind::Name(_))
    ) && event.paths.iter().any(names_root)
    {
        return emitter.emit(StreamEvent::RequiresRescan { event_id });
    }
    match event.kind {
        NotifyKind::Access(_) => {}
        NotifyKind::Create(_) => event
            .paths
            .iter()
            .for_each(|p| emitter.change(ChangeKind::Created, exact(p), event_id)),
        NotifyKind::Remove(_) => event
            .paths
            .iter()
            .for_each(|p| emitter.change(ChangeKind::Deleted, exact(p), event_id)),
        NotifyKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => emitter
            .change(
                ChangeKind::Renamed {
                    previous_path: Some(crate::paths::normalize(&exact(&event.paths[0]))),
                },
                exact(&event.paths[1]),
                event_id,
            ),
        NotifyKind::Modify(ModifyKind::Name(_)) => event.paths.iter().for_each(|p| {
            emitter.change(
                ChangeKind::Renamed {
                    previous_path: None,
                },
                exact(p),
                event_id,
            )
        }),
        NotifyKind::Modify(_) | NotifyKind::Any | NotifyKind::Other => event
            .paths
            .iter()
            .for_each(|p| emitter.change(ChangeKind::Modified, exact(p), event_id)),
    }
}
