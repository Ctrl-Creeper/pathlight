//! Versioned evidence separate from the legacy activity journal. An observation
//! and a later size measurement are independent evidence, never a write-I/O count.
//! Single-writer file lock, durable records and source keys make retries explicit.

use crate::measurement::FileMeasurement;
use crate::monitor::Change;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub backend: String,
    pub epoch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub schema_version: u32,
    pub source: SourceIdentity,
    /// Journal ordering/deduplication identity within this source epoch.
    pub sequence: u64,
    /// Native cursor is source-specific; it is not a global sequence number.
    #[serde(default)]
    pub source_cursor: Option<u64>,
    pub observed_at: SystemTime,
    pub payload: EvidencePayload,
}

/// Lossless storage spelling, never a display string or an identity key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "units", rename_all = "snake_case")]
pub enum NativePath {
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
    Utf8(String),
}

impl Default for NativePath {
    fn default() -> Self {
        // Schema v1 did not persist native paths. Keep the compatibility
        // sentinel distinguishable from every path emitted by a v2 recorder.
        Self::Utf8(String::new())
    }
}

impl NativePath {
    fn is_missing(&self) -> bool {
        match self {
            Self::UnixBytes(units) => units.is_empty(),
            Self::WindowsWide(units) => units.is_empty(),
            Self::Utf8(path) => path.is_empty(),
        }
    }
}

impl NativePath {
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::UnixBytes(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::WindowsWide(path.as_os_str().encode_wide().collect())
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self::Utf8(path.to_string_lossy().into_owned())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvidencePayload {
    SessionStarted {
        root: NativePath,
    },
    /// The recorder stopped and drained its accepted queue. This does not
    /// assert an OS delivery barrier, absence of gaps, or complete history.
    SessionEnded,
    /// Begins a native binding manifest. A missing matching `Snapshot` record
    /// means the manifest was interrupted and must not be treated as complete.
    SnapshotStarted {
        #[serde(default)]
        snapshot_id: u64,
        #[serde(default)]
        root: NativePath,
    },
    SnapshotEntries {
        #[serde(default)]
        snapshot_id: u64,
        entries: Vec<SnapshotBinding>,
    },
    /// Scope that could not be measured; separate records keep lines bounded.
    SnapshotError {
        #[serde(default)]
        snapshot_id: u64,
        path: NativePath,
        kind: String,
        message: String,
    },
    Observation {
        change: Change,
        #[serde(default)]
        native_path: NativePath,
        #[serde(default)]
        native_previous_path: Option<NativePath>,
        measurement: Option<FileMeasurement>,
        measurement_error: Option<String>,
    },
    Gap {
        reason: String,
    },
    /// Completed traversal over an interval, never an atomic snapshot or proof
    /// that an earlier history gap was repaired. Totals may be unavailable.
    Snapshot {
        #[serde(default)]
        snapshot_id: u64,
        #[serde(default)]
        root: NativePath,
        started_at: SystemTime,
        finished_at: SystemTime,
        logical_bytes: Option<u64>,
        allocated_bytes: Option<u64>,
        measured_paths: u64,
        measured_objects: Option<u64>,
        partial: bool,
    },
    Ready {
        resumable: bool,
        pairs_renames: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotBinding {
    pub path: NativePath,
    pub measurement: FileMeasurement,
}

type RecordKey = (SourceIdentity, u64);
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_INDEXED_RECORDS: usize = 1_000_000;
pub const MAX_BATCH_RECORDS: usize = 256;
pub const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

pub struct EvidenceJournal {
    file: File,
    /// Offsets rather than full records: memory bounded by record count.
    index: HashMap<RecordKey, u64>,
    poisoned: bool,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

impl EvidenceJournal {
    /// Opens a single-writer journal. A malformed/torn record is reported and
    /// preserved for explicit recovery; it is never silently skipped/truncated.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        // `sync_data` makes appended bytes durable, but a newly created file can
        // still disappear after power loss unless its parent directory entry is
        // also synchronized. This runs once per open, not once per batch.
        sync_parent_directory(path)?;
        file.try_lock().map_err(|error| {
            io::Error::other(format!(
                "evidence journal already in use or cannot be locked: {error}"
            ))
        })?;
        let index = Self::read_index(&mut file)?;
        Ok(Self {
            file,
            index,
            poisoned: false,
        })
    }

    fn read_index(file: &mut File) -> io::Result<HashMap<RecordKey, u64>> {
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file);
        let mut index = HashMap::new();
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            bytes.clear();
            let count = reader
                .by_ref()
                .take((MAX_RECORD_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            let record = decode_record(&bytes)?;
            if index
                .insert((record.source, record.sequence), offset)
                .is_some()
            {
                return Err(invalid("duplicate source key in persisted evidence"));
            }
            if index.len() > MAX_INDEXED_RECORDS {
                return Err(invalid(
                    "evidence journal record limit reached; rotate the file",
                ));
            }
            offset += count as u64;
        }
        Ok(index)
    }

    fn read_at(&mut self, offset: u64) -> io::Result<Option<(EvidenceRecord, u64)>> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(&mut self.file);
        let mut bytes = Vec::new();
        // Bound malformed lines even when the file is externally damaged.
        let mut limited = reader.by_ref().take((MAX_RECORD_BYTES + 1) as u64);
        let count = limited.read_until(b'\n', &mut bytes)?;
        if count == 0 {
            return Ok(None);
        }
        let record = decode_record(&bytes)?;
        Ok(Some((record, count as u64)))
    }

    /// Synchronizes one record. Use append_batch for a burst of observations.
    pub fn append(&mut self, record: EvidenceRecord) -> io::Result<bool> {
        Ok(self.append_batch(vec![record])? == 1)
    }

    /// Validates the complete batch, writes its new records, then synchronizes
    /// once. No key is acknowledged before sync succeeds. A write/sync failure
    /// poisons this handle; reopening validates any surviving complete prefix.
    /// This is not a transaction: a process crash can leave a partial batch.
    pub fn append_batch(&mut self, records: Vec<EvidenceRecord>) -> io::Result<usize> {
        if self.poisoned {
            return Err(io::Error::other(
                "evidence journal has an unresolved write failure; reopen for recovery",
            ));
        }
        if records.len() > MAX_BATCH_RECORDS {
            return Err(invalid("evidence batch exceeds 256 records"));
        }
        let mut batch_index: HashMap<RecordKey, usize> = HashMap::new();
        let mut staged: Vec<EvidenceRecord> = Vec::new();
        let mut relative_offsets = Vec::new();
        let mut bytes = Vec::new();
        for record in records {
            validate(&record)?;
            let key = (record.source.clone(), record.sequence);
            let previous = if let Some(&offset) = self.index.get(&key) {
                Some(
                    self.read_at(offset)?
                        .ok_or_else(|| invalid("indexed evidence record disappeared"))?
                        .0,
                )
            } else {
                batch_index.get(&key).map(|&index| staged[index].clone())
            };
            if let Some(previous) = previous {
                if previous != record {
                    return Err(invalid(
                        "conflicting payload for an existing source sequence",
                    ));
                }
                continue;
            }
            if self.index.len() + staged.len() >= MAX_INDEXED_RECORDS {
                return Err(invalid(
                    "evidence journal record limit reached; rotate the file",
                ));
            }
            let mut line = serde_json::to_vec(&record).map_err(|e| invalid(e.to_string()))?;
            line.push(b'\n');
            if line.len() > MAX_RECORD_BYTES {
                return Err(invalid("evidence record too large"));
            }
            if bytes.len() + line.len() > MAX_BATCH_BYTES {
                return Err(invalid("evidence batch exceeds 8 MiB"));
            }
            relative_offsets.push(bytes.len() as u64);
            bytes.extend_from_slice(&line);
            batch_index.insert(key, staged.len());
            staged.push(record);
        }
        if staged.is_empty() {
            return Ok(0);
        }
        let offset = self.file.seek(SeekFrom::End(0))?;
        if let Err(error) = self
            .file
            .write_all(&bytes)
            .and_then(|_| self.file.sync_data())
        {
            self.poisoned = true;
            return Err(error);
        }
        let count = staged.len();
        for (record, relative_offset) in staged.into_iter().zip(relative_offsets) {
            self.index
                .insert((record.source, record.sequence), offset + relative_offset);
        }
        Ok(count)
    }

    pub fn read_records(&mut self) -> io::Result<Vec<EvidenceRecord>> {
        let mut rows = Vec::new();
        let mut offset = 0;
        while let Some((row, length)) = self.read_at(offset)? {
            rows.push(row);
            offset += length;
        }
        Ok(rows)
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn decode_record(bytes: &[u8]) -> io::Result<EvidenceRecord> {
    if bytes.len() > MAX_RECORD_BYTES || bytes.last() != Some(&b'\n') {
        return Err(invalid("oversized or incomplete evidence record"));
    }
    let record = serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    validate(&record)?;
    Ok(record)
}

fn validate(record: &EvidenceRecord) -> io::Result<()> {
    if !(1..=CURRENT_SCHEMA_VERSION).contains(&record.schema_version) {
        return Err(invalid("unsupported evidence schema version"));
    }
    if record.source.backend.is_empty() || record.source.epoch.is_empty() {
        return Err(invalid("source identity must include backend and epoch"));
    }
    match &record.payload {
        EvidencePayload::SnapshotStarted { snapshot_id, .. }
        | EvidencePayload::SnapshotEntries { snapshot_id, .. }
        | EvidencePayload::SnapshotError { snapshot_id, .. }
        | EvidencePayload::Snapshot { snapshot_id, .. }
            if record.schema_version >= 2 && *snapshot_id == 0 =>
        {
            return Err(invalid("snapshot identity must be nonzero"));
        }
        EvidencePayload::SnapshotStarted { .. } | EvidencePayload::SnapshotEntries { .. }
            if record.schema_version == 1 =>
        {
            return Err(invalid("evidence payload requires schema version 2"));
        }
        EvidencePayload::SessionStarted { root }
            if record.schema_version >= 2 && root.is_missing() =>
        {
            return Err(invalid("session root native path is required"));
        }
        EvidencePayload::SnapshotStarted { root, .. }
            if record.schema_version >= 2 && root.is_missing() =>
        {
            return Err(invalid("snapshot root native path is required"));
        }
        EvidencePayload::SnapshotEntries { entries, .. }
            if record.schema_version >= 2
                && (entries.is_empty() || entries.iter().any(|entry| entry.path.is_missing())) =>
        {
            return Err(invalid("snapshot entries require native paths"));
        }
        EvidencePayload::SnapshotError { path, .. }
            if record.schema_version >= 2 && path.is_missing() =>
        {
            return Err(invalid("snapshot error native path is required"));
        }
        EvidencePayload::Observation {
            change,
            native_path,
            native_previous_path,
            ..
        } if record.schema_version >= 2
            && (native_path.is_missing()
                || matches!(
                    &change.kind,
                    crate::monitor::ChangeKind::Renamed {
                        previous_path: Some(_)
                    }
                ) && native_previous_path
                    .as_ref()
                    .is_none_or(NativePath::is_missing)) =>
        {
            return Err(invalid("observation native paths are required"));
        }
        EvidencePayload::Snapshot { root, .. }
            if record.schema_version >= 2 && root.is_missing() =>
        {
            return Err(invalid("snapshot root native path is required"));
        }
        EvidencePayload::Snapshot {
            started_at,
            finished_at,
            ..
        } if finished_at < started_at => {
            return Err(invalid("snapshot interval is reversed"));
        }
        _ => {}
    }
    Ok(())
}
