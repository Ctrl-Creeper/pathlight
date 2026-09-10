//! Bounded, explicit monitoring sessions producing an evidence journal.
//!
//! Callbacks only enqueue observations. Metadata reads, snapshots and journal
//! I/O happen on the caller thread. Scans describe observation intervals, not
//! atomic state or reconstructed operation history. The requested duration is
//! the live interval between scans; registration and scans add wall-clock time.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::evidence::{
    EvidenceJournal, EvidencePayload, EvidenceRecord, NativePath, SnapshotBinding, SourceIdentity,
    CURRENT_SCHEMA_VERSION, MAX_BATCH_BYTES, MAX_BATCH_RECORDS,
};
use crate::measurement::measure_file;
use crate::monitor::{watcher_capabilities, ActivityListener, StreamEvent, Watcher};

const QUEUE_CAPACITY: usize = 4096;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
const SNAPSHOT_CHUNK_TARGET_BYTES: usize = 512 * 1024;
const SNAPSHOT_CHUNK_MAX_ENTRIES: usize = 256;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
pub struct RecordingSummary {
    pub records_written: u64,
    pub observations: u64,
    pub gaps: u64,
}

struct ReceivedEvent {
    event: StreamEvent,
    received_at: SystemTime,
}

struct QueueListener {
    sender: SyncSender<ReceivedEvent>,
    dropped: Arc<AtomicU64>,
}

impl ActivityListener for QueueListener {
    fn on_event(&self, event: StreamEvent) {
        if let Err(TrySendError::Full(_)) = self.sender.try_send(ReceivedEvent {
            event,
            received_at: SystemTime::now(),
        }) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Recorder {
    journal: EvidenceJournal,
    source: SourceIdentity,
    /// The sizes of file this session records at all; see [`SizeBounds`].
    bounds: SizeBounds,
    sequence: u64,
    next_snapshot_id: u64,
    summary: RecordingSummary,
    pending: Vec<EvidenceRecord>,
    pending_bytes: usize,
    flush_due: Option<Instant>,
}

impl Recorder {
    fn append(
        &mut self,
        payload: EvidencePayload,
        observed_at: SystemTime,
        source_cursor: Option<u64>,
    ) -> io::Result<()> {
        self.sequence += 1;
        let record = EvidenceRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: self.source.clone(),
            sequence: self.sequence,
            source_cursor,
            observed_at,
            payload,
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?.len() + 1;
        if self.pending_bytes + bytes > MAX_BATCH_BYTES {
            self.flush()?;
        }
        self.pending.push(record);
        self.pending_bytes += bytes;
        self.flush_due
            .get_or_insert_with(|| Instant::now() + FLUSH_INTERVAL);
        if self.pending.len() >= MAX_BATCH_RECORDS || self.pending_bytes >= MAX_BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.pending);
        let observations = batch
            .iter()
            .filter(|record| matches!(record.payload, EvidencePayload::Observation { .. }))
            .count();
        let gaps = batch
            .iter()
            .filter(|record| matches!(record.payload, EvidencePayload::Gap { .. }))
            .count();
        // This recorder owns a fresh epoch and strictly increasing sequences,
        // so none of these source keys were previously appended.
        let written = self.journal.append_batch(batch)?;
        self.summary.records_written += written as u64;
        self.summary.observations += observations as u64;
        self.summary.gaps += gaps as u64;
        self.pending_bytes = 0;
        self.flush_due = None;
        Ok(())
    }

    fn flush_if_due(&mut self) -> io::Result<()> {
        if self
            .flush_due
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.flush()?;
        }
        Ok(())
    }

    fn receive_timeout(&self, deadline: Instant) -> Duration {
        self.flush_due
            .unwrap_or(deadline)
            .min(deadline)
            .saturating_duration_since(Instant::now())
    }

    fn event(&mut self, received: ReceivedEvent) -> io::Result<bool> {
        let capabilities = watcher_capabilities();
        let (payload, cursor, ready) = match received.event {
            StreamEvent::Change { change, event_id } => {
                let native_path = NativePath::from_path(Path::new(&change.path));
                let native_previous_path = match &change.kind {
                    crate::monitor::ChangeKind::Renamed {
                        previous_path: Some(previous_path),
                    } => Some(NativePath::from_path(Path::new(previous_path))),
                    _ => None,
                };
                let (measurement, measurement_error) = match measure_file(Path::new(&change.path)) {
                    Ok(measurement) => (Some(measurement), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                if !self.bounds.records(measurement.as_ref()) {
                    return Ok(false);
                }
                (
                    EvidencePayload::Observation {
                        change,
                        native_path,
                        native_previous_path,
                        measurement,
                        measurement_error,
                    },
                    event_id,
                    false,
                )
            }
            StreamEvent::RequiresRescan { event_id } => (
                EvidencePayload::Gap {
                    reason: "Backend reported incomplete observation history; state reconciliation does not restore missing operations".into(),
                },
                event_id,
                false,
            ),
            StreamEvent::HistoryCaughtUp { event_id } => (
                EvidencePayload::Ready {
                    resumable: capabilities.resumable_cursor,
                    pairs_renames: capabilities.pairs_renames,
                },
                event_id,
                true,
            ),
        };
        // Zero is used by fresh-start readiness markers, not a replay cursor.
        self.append(
            payload,
            received.received_at,
            (cursor != 0).then_some(cursor),
        )?;
        Ok(ready)
    }

    fn snapshot(&mut self, root: &Path) -> io::Result<()> {
        self.next_snapshot_id += 1;
        let snapshot_id = self.next_snapshot_id;
        self.append(
            EvidencePayload::SnapshotStarted {
                snapshot_id,
                root: NativePath::from_path(root),
            },
            SystemTime::now(),
            None,
        )?;
        self.flush()?;
        let snapshot = match crate::snapshot::scan(root) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.append(
                    EvidencePayload::SnapshotError {
                        snapshot_id,
                        path: NativePath::from_path(root),
                        kind: format!("{:?}", error.kind()),
                        message: error.to_string(),
                    },
                    SystemTime::now(),
                    None,
                )?;
                self.flush()?;
                return Err(error);
            }
        };
        let totals = snapshot.totals();
        let mut chunk = Vec::new();
        let mut chunk_bytes = 0;
        for (path, measurement) in snapshot.entries {
            let binding = SnapshotBinding {
                path: NativePath::from_path(&path),
                measurement,
            };
            let binding_bytes = serde_json::to_vec(&binding)
                .map_err(io::Error::other)?
                .len()
                + 1;
            if !chunk.is_empty()
                && (chunk.len() >= SNAPSHOT_CHUNK_MAX_ENTRIES
                    || chunk_bytes + binding_bytes > SNAPSHOT_CHUNK_TARGET_BYTES)
            {
                self.append(
                    EvidencePayload::SnapshotEntries {
                        snapshot_id,
                        entries: std::mem::take(&mut chunk),
                    },
                    snapshot.finished_at,
                    None,
                )?;
                chunk_bytes = 0;
            }
            chunk.push(binding);
            chunk_bytes += binding_bytes;
        }
        if !chunk.is_empty() {
            self.append(
                EvidencePayload::SnapshotEntries {
                    snapshot_id,
                    entries: chunk,
                },
                snapshot.finished_at,
                None,
            )?;
        }
        for error in snapshot.errors {
            self.append(
                EvidencePayload::SnapshotError {
                    snapshot_id,
                    path: NativePath::from_path(&error.path),
                    kind: format!("{:?}", error.kind),
                    message: error.message,
                },
                snapshot.finished_at,
                None,
            )?;
        }
        self.append(
            EvidencePayload::Snapshot {
                snapshot_id,
                root: NativePath::from_path(&snapshot.root),
                started_at: snapshot.started_at,
                finished_at: snapshot.finished_at,
                logical_bytes: totals.logical_bytes,
                allocated_bytes: totals.allocated_bytes,
                measured_paths: totals.measured_paths,
                measured_objects: totals.measured_objects,
                partial: totals.partial,
            },
            snapshot.finished_at,
            None,
        )?;
        self.flush()
    }

    fn overflow(&mut self, dropped: &AtomicU64) -> io::Result<()> {
        let count = dropped.swap(0, Ordering::Relaxed);
        if count > 0 {
            self.append(
                EvidencePayload::Gap {
                    reason: format!(
                        "Recorder queue overflow: {count} received observations were dropped"
                    ),
                },
                SystemTime::now(),
                None,
            )?;
        }
        Ok(())
    }
}

/// The sizes of file a recording keeps observations for, in bytes; `None` is
/// no bound. The same rule the windows apply to a watch
/// ([`crate::attribution::size_in_bounds`]), so one number means one thing on
/// every host.
///
/// Only regular files are judged: a directory or a symlink has no size worth
/// comparing, and dropping those observations would take the renames and
/// deletions a reader reconstructs the history from. A file that could not be
/// measured is kept for the same reason. The snapshots at each end still
/// describe the whole folder — bounds decide what the live interval records,
/// not what the folder is said to hold.
///
/// Judged on the allocated size where the filesystem reports one, which is the
/// measure attribution uses, so a bound smaller than one allocation block
/// cannot separate small files on any of them.
#[derive(Clone, Copy, Debug, Default)]
pub struct SizeBounds {
    pub min_bytes: Option<i64>,
    pub max_bytes: Option<i64>,
}

impl SizeBounds {
    fn records(&self, measurement: Option<&crate::measurement::FileMeasurement>) -> bool {
        let size = measurement
            .filter(|measurement| measurement.kind == crate::measurement::FileKind::File)
            .map(|measurement| {
                measurement
                    .allocated_bytes
                    .unwrap_or(measurement.logical_bytes) as i64
            });
        crate::attribution::size_in_bounds(size, self.min_bytes, self.max_bytes)
    }
}

/// Record one live interval with a snapshot at each end, while the watcher is
/// active. The journal must be outside the monitored tree to prevent feedback.
/// This starts a new source epoch; it does not resume earlier recording sessions.
pub fn record_for(root: &Path, journal: &Path, duration: Duration) -> io::Result<RecordingSummary> {
    record_bounded(root, journal, duration, SizeBounds::default(), || {})
}

/// As `record_for`, but runs `on_live` once the watcher is armed and the
/// opening snapshot is written, before the live interval starts counting.
/// The journal holds an exclusive lock for the whole session, so watching it
/// from outside is not a portable way to learn that a session went live.
/// `on_live` runs on the caller thread while the bounded queue fills, so it
/// belongs to setup work, not to anything long-running.
pub fn record_session(
    root: &Path,
    journal: &Path,
    duration: Duration,
    on_live: impl FnOnce(),
) -> io::Result<RecordingSummary> {
    record_bounded(root, journal, duration, SizeBounds::default(), on_live)
}

/// As `record_session`, keeping observations only for files inside `bounds`.
pub fn record_bounded(
    root: &Path,
    journal: &Path,
    duration: Duration,
    bounds: SizeBounds,
    on_live: impl FnOnce(),
) -> io::Result<RecordingSummary> {
    if duration.is_zero() || Instant::now().checked_add(duration).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "duration must be positive and representable",
        ));
    }
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "watch root must be a directory",
        ));
    }
    let root_text = root.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "watcher root must be representable as UTF-8",
        )
    })?;
    let journal_path = resolve_output(journal)?;
    if journal_path.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal must be outside the monitored tree",
        ));
    }
    match measure_file(&journal_path) {
        Ok(measurement) if measurement.link_count.is_some_and(|count| count > 1) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal must not have hard-link aliases",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut recorder = Recorder {
        journal: EvidenceJournal::open(&journal_path)?,
        bounds,
        source: SourceIdentity {
            backend: backend_name().into(),
            epoch: format!(
                "{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            ),
        },
        sequence: 0,
        next_snapshot_id: 0,
        summary: RecordingSummary::default(),
        pending: Vec::new(),
        pending_bytes: 0,
        flush_due: None,
    };
    recorder.append(
        EvidencePayload::SessionStarted {
            root: NativePath::from_path(&root),
        },
        SystemTime::now(),
        None,
    )?;
    recorder.flush()?;
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let dropped = Arc::new(AtomicU64::new(0));
    let watcher = Watcher::start(
        root_text.into(),
        None,
        100,
        Arc::new(QueueListener {
            sender,
            dropped: dropped.clone(),
        }),
    )
    .map_err(io::Error::other)?;
    await_ready(
        &receiver,
        &mut recorder,
        &dropped,
        Instant::now() + Duration::from_secs(10),
    )?;
    recorder.snapshot(&root)?;
    on_live();
    let deadline = Instant::now().checked_add(duration).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "duration is not representable")
    })?;
    loop {
        recorder.overflow(&dropped)?;
        recorder.flush_if_due()?;
        if Instant::now() >= deadline {
            break;
        }
        match receiver.recv_timeout(recorder.receive_timeout(deadline)) {
            Ok(event) => {
                recorder.event(event)?;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected",
                ));
            }
        }
    }
    recorder.snapshot(&root)?;
    watcher.stop();
    for event in receiver.try_iter() {
        recorder.event(event)?;
    }
    recorder.overflow(&dropped)?;
    recorder.append(EvidencePayload::SessionEnded, SystemTime::now(), None)?;
    recorder.flush()?;
    Ok(recorder.summary)
}

fn await_ready(
    receiver: &Receiver<ReceivedEvent>,
    recorder: &mut Recorder,
    dropped: &AtomicU64,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "watcher readiness deadline expired",
            ));
        }
        recorder.overflow(dropped)?;
        recorder.flush_if_due()?;
        let event = match receiver.recv_timeout(recorder.receive_timeout(deadline)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected before readiness",
                ))
            }
        };
        if recorder.event(event)? {
            return Ok(());
        }
    }
}

fn resolve_output(path: &Path) -> io::Result<PathBuf> {
    if path.symlink_metadata().is_ok() {
        return path.canonicalize();
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "journal needs a file name"))?;
    Ok(parent.canonicalize()?.join(name))
}

fn backend_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "fsevents"
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        "inotify"
    } else if cfg!(windows) {
        "read_directory_changes"
    } else {
        "notify"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_callback_queue_persists_loss_even_when_it_cannot_enqueue_a_gap() {
        let output = tempfile::tempdir().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let listener = QueueListener {
            sender,
            dropped: dropped.clone(),
        };
        let mut recorder = Recorder {
            journal: EvidenceJournal::open(&output.path().join("evidence.jsonl")).unwrap(),
            source: SourceIdentity {
                backend: "test".into(),
                epoch: "overflow".into(),
            },
            bounds: SizeBounds::default(),
            sequence: 0,
            next_snapshot_id: 0,
            summary: RecordingSummary::default(),
            pending: Vec::new(),
            pending_bytes: 0,
            flush_due: None,
        };

        listener.on_event(StreamEvent::HistoryCaughtUp { event_id: 1 });
        listener.on_event(StreamEvent::RequiresRescan { event_id: 2 });
        listener.on_event(StreamEvent::RequiresRescan { event_id: 3 });
        recorder.event(receiver.try_recv().unwrap()).unwrap();
        recorder.overflow(&dropped).unwrap();
        recorder.overflow(&dropped).unwrap();
        recorder.flush().unwrap();

        let records = recorder.journal.read_records().unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            matches!(&records[1].payload, EvidencePayload::Gap { reason }
            if reason == "Recorder queue overflow: 2 received observations were dropped")
        );
        assert_eq!(recorder.summary.gaps, 1);
    }

    #[test]
    fn queued_events_cannot_extend_an_expired_readiness_deadline() {
        let output = tempfile::tempdir().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let listener = QueueListener {
            sender,
            dropped: dropped.clone(),
        };
        let mut recorder = Recorder {
            journal: EvidenceJournal::open(&output.path().join("evidence.jsonl")).unwrap(),
            source: SourceIdentity {
                backend: "test".into(),
                epoch: "deadline".into(),
            },
            bounds: SizeBounds::default(),
            sequence: 0,
            next_snapshot_id: 0,
            summary: RecordingSummary::default(),
            pending: Vec::new(),
            pending_bytes: 0,
            flush_due: None,
        };
        listener.on_event(StreamEvent::HistoryCaughtUp { event_id: 1 });

        let error = await_ready(&receiver, &mut recorder, &dropped, Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(recorder.journal.read_records().unwrap().is_empty());
    }
}
