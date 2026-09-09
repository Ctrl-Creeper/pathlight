#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) use crate::inotify::start;

use std::path::Path;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// How often the root is re-checked, at most. Event-driven, not a timer: a
/// watch nothing is happening in asks nothing of the disk.
const ROOT_CHECK_EVERY: Duration = Duration::from_secs(1);

/// Whether the configured path still names the directory this watch opened on.
///
/// Windows has no root-loss signal. Our own handle keeps the directory alive,
/// so it cannot be deleted while watched — but it can be renamed, and the
/// handle follows it. Events keep arriving, every path in them is the old
/// spelling, and nothing anywhere says the watch has moved: a silently wrong
/// history, which is worse than a reported gap. inotify has `MOVE_SELF` and
/// `DELETE_SELF` and FSEvents has `RootChanged`; here the only thing left is
/// to ask whether the path still resolves to the same object.
pub(crate) struct RootIdentity {
    at: String,
    /// `None` when the filesystem would not say. Then only disappearance is
    /// detectable, which is still better than nothing.
    identity: Option<crate::measurement::ObjectIdentity>,
    /// Latched: once reported, the watch is already known to be wrong, and
    /// repeating it every second would bury the report it already made.
    reported: bool,
    due: Instant,
}

impl RootIdentity {
    pub(crate) fn of(root: &str) -> Self {
        Self {
            at: root.to_owned(),
            identity: crate::measurement::measure_file(Path::new(root))
                .ok()
                .and_then(|measurement| measurement.identity),
            reported: false,
            due: Instant::now(),
        }
    }

    /// True once, when the root has gone or become a different directory.
    pub(crate) fn drifted(&mut self) -> bool {
        if self.reported || Instant::now() < self.due {
            return false;
        }
        self.due = Instant::now() + ROOT_CHECK_EVERY;
        // An unreadable root counts as drifted. A transient failure costs a
        // rescan the host would have run anyway; treating it as healthy costs
        // the history.
        self.reported = match crate::measurement::measure_file(Path::new(&self.at)) {
            Ok(now) => self.identity.is_some() && now.identity != self.identity,
            Err(_) => true,
        };
        self.reported
    }
}

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
    let mut root = RootIdentity::of(&emitter.root);
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            let event_id = next_id.fetch_add(1, Ordering::Relaxed);
            // Before the event, not after: the paths in it are already the
            // old spelling if the root moved, and the host needs to know that
            // before it trusts them.
            if root.drifted() {
                handler_emitter.emit(StreamEvent::RequiresRescan { event_id });
            }
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
        // ponytail: one gap for every error kind. A watch that runs into the
        // inotify limit mid-run stays permanently partial, and reconciling
        // still finds the changes, but the stream cannot say coverage shrank.
        // Saying that needs the per-watch coverage descriptor PLATFORMS.md
        // proposes; add it when a host has somewhere to show it.
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
    if matches!(
        event.kind,
        NotifyKind::Remove(_) | NotifyKind::Modify(ModifyKind::Name(_))
    ) && event
        .paths
        .iter()
        .any(|path| crate::paths::normalize(&exact(path)) == emitter.root)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The silent failure this exists for: the watch is alive, the events
    /// keep coming, and the path they are all spelled with names something
    /// else now. Not compiled on macOS, where FSEvents reports it itself.
    #[test]
    fn a_root_that_became_a_different_directory_is_noticed() {
        let parent = tempfile::tempdir().unwrap();
        let at = parent.path().join("watched");
        std::fs::create_dir(&at).unwrap();
        let spelling = crate::paths::normalize(&at.to_string_lossy());
        let mut root = RootIdentity::of(&spelling);

        std::fs::rename(&at, parent.path().join("renamed")).unwrap();

        assert!(root.drifted(), "a renamed root went unnoticed");
        // Latched: the host has been told, and the same report every second
        // afterwards would bury it.
        assert!(!root.drifted());
    }

    #[test]
    fn a_root_that_is_still_itself_costs_nothing_and_reports_nothing() {
        let at = tempfile::tempdir().unwrap();
        let spelling = crate::paths::normalize(&at.path().to_string_lossy());
        let mut root = RootIdentity::of(&spelling);

        std::fs::write(at.path().join("busy.txt"), b"work").unwrap();

        assert!(!root.drifted());
        // A second call inside the check window does not go near the disk;
        // what it must not do is invent a drift.
        assert!(!root.drifted());
    }
}
