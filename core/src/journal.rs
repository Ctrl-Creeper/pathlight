//! Append-only JSONL journal sharing one file with Swift's `JSONLActivityEventStore`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::{ActivityEvent, CoreError};

/// Rows written with encryption on, by either host. The key is per machine —
/// Swift's is in the macOS Keychain, this build's is beside the journal — so a
/// line whose key is somewhere else stays unread rather than failing the load.
use crate::crypt::PREFIX as ENCRYPTED_PREFIX;

#[derive(Debug, uniffi::Object)]
pub struct Journal {
    path: PathBuf,
    /// Whether rows appended from now on are encrypted. Reading does not need
    /// it: an encrypted line is recognised by its marker either way, which is
    /// what lets the setting be turned on and off without a migration.
    encrypt: bool,
}

#[uniffi::export]
impl Journal {
    #[uniffi::constructor]
    pub fn new(path: String) -> Arc<Self> {
        Arc::new(Self {
            path: PathBuf::from(path),
            encrypt: false,
        })
    }

    /// The same journal, appending encrypted rows.
    #[uniffi::constructor]
    pub fn encrypting(path: String) -> Arc<Self> {
        Arc::new(Self {
            path: PathBuf::from(path),
            encrypt: true,
        })
    }

    /// Appends events as one line each. Creates the directory (0700) and file (0600) as needed.
    pub fn append(&self, events: Vec<ActivityEvent>) -> Result<(), CoreError> {
        if events.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            set_permissions(parent, 0o700)?;
        }

        // The key is loaded once for the batch, and only when it is needed:
        // an unencrypted journal must never create one.
        let key = match self.encrypt {
            true => Some(crate::crypt::key_or_create(&self.path)?),
            false => None,
        };
        let mut payload = String::new();
        for event in &events {
            let line = event.to_json_line()?;
            match &key {
                Some(key) => payload.push_str(&crate::crypt::seal(key, line.as_bytes())?),
                None => payload.push_str(&line),
            }
            payload.push('\n');
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        set_permissions(&self.path, 0o600)?;
        file.write_all(payload.as_bytes())?;
        Ok(())
    }

    /// Drops rows that are older than `retention_days`, then, if the file
    /// still would not fit in `limit_bytes`, the oldest of what is left.
    /// Returns how many rows went. Zero means unlimited, for either.
    ///
    /// A journal nothing ever trims is the one way a monitor that promises to
    /// be cheap fills a disk — the thing it exists to warn about.
    ///
    /// Age is judged per row and the cap positionally, because a row this
    /// build cannot date (an encrypted line, or a line from a newer format)
    /// must not be aged out on a guess. It still counts against the cap, and
    /// the cap drops from the front, which is oldest for an append-only file.
    // ponytail: no daily rollup. Swift's `retainedEvents` summarizes older
    // detailed rows into one row per day instead of dropping them; port that
    // here when a host wants the long tail, not before.
    pub fn trim(&self, retention_days: u32, limit_bytes: u64) -> Result<u64, CoreError> {
        if (retention_days == 0 && limit_bytes == 0) || !self.path.exists() {
            return Ok(0);
        }
        // Read whole: the file is what the previous trim left, so it is
        // bounded by the cap. Stream it if a caller ever passes a cap larger
        // than it is willing to hold in memory.
        let contents = fs::read_to_string(&self.path)?;
        let mut kept: Vec<&str> = contents.lines().filter(|line| !line.is_empty()).collect();
        let total = kept.len();
        let key = self.key_for(&contents);

        if retention_days > 0 {
            let cutoff = SystemTime::now()
                .checked_sub(Duration::from_secs(u64::from(retention_days) * 86_400));
            if let Some(cutoff) = cutoff {
                kept.retain(|line| match dated(line, key.as_ref()) {
                    Some(timestamp) => timestamp >= cutoff,
                    None => true,
                });
            }
        }

        if limit_bytes > 0 {
            let mut bytes: u64 = kept.iter().map(|line| line.len() as u64 + 1).sum();
            let mut oldest = 0;
            while bytes > limit_bytes && oldest < kept.len() {
                bytes -= kept[oldest].len() as u64 + 1;
                oldest += 1;
            }
            kept.drain(..oldest);
        }

        if kept.len() == total {
            return Ok(0);
        }

        // Written beside the journal and renamed over it, so a crash halfway
        // through leaves the old journal rather than half a new one.
        let temporary = self.path.with_extension("jsonl.trimming");
        let mut file = fs::File::create(&temporary)?;
        set_permissions(&temporary, 0o600)?;
        for line in &kept {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &self.path)?;
        Ok((total - kept.len()) as u64)
    }

    /// Newest first, then path descending, limited — the same order Swift returns.
    pub fn load(&self, root_path: String, limit: u32) -> Result<Vec<ActivityEvent>, CoreError> {
        if limit == 0 || !self.path.exists() {
            return Ok(Vec::new());
        }

        let root = crate::paths::normalize(&root_path);
        let contents = fs::read_to_string(&self.path)?;
        let key = self.key_for(&contents);
        let mut events: Vec<ActivityEvent> = contents
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| readable(line, key.as_ref()))
            .filter_map(|line| ActivityEvent::from_json_line(&line).ok())
            .filter(|event| crate::paths::normalize(&event.root_path) == root)
            .collect();

        events.sort_by(|lhs, rhs| {
            rhs.timestamp
                .cmp(&lhs.timestamp)
                .then_with(|| rhs.path.cmp(&lhs.path))
        });
        events.truncate(limit as usize);
        Ok(events)
    }

    /// Dashboard history for one root: buckets, totals, and the newest rows.
    pub fn load_history(
        &self,
        root_path: String,
        limit: u32,
        bucket_interval_secs: u64,
    ) -> Result<crate::HistorySnapshot, CoreError> {
        // Every retained row, not just the page: `limit` cuts the listed rows
        // inside `build_history`, after the totals are known.
        let events = self.load(root_path.clone(), u32::MAX)?;
        Ok(crate::history::build_history(
            &root_path,
            events,
            bucket_interval_secs,
            limit,
            std::time::SystemTime::now(),
        ))
    }
}

impl Journal {
    /// The key, if this journal has encrypted rows and this machine holds it.
    /// Nothing is created here: a read must not leave a key behind.
    fn key_for(&self, contents: &str) -> Option<[u8; 32]> {
        contents
            .lines()
            .any(|line| line.starts_with(ENCRYPTED_PREFIX))
            .then(|| crate::crypt::key(&self.path))
            .flatten()
    }
}

/// The json behind a line, decrypting when the line is encrypted and this
/// machine has the key. `None` for a row this build cannot read.
fn readable(line: &str, key: Option<&[u8; 32]>) -> Option<String> {
    if !line.starts_with(ENCRYPTED_PREFIX) {
        return Some(line.to_owned());
    }
    String::from_utf8(crate::crypt::open(key?, line)?).ok()
}

/// The timestamp of a row this build can read, or `None` when it cannot.
fn dated(line: &str, key: Option<&[u8; 32]>) -> Option<SystemTime> {
    ActivityEvent::from_json_line(&readable(line, key)?)
        .ok()
        .map(|event| event.timestamp)
}

#[cfg(unix)]
pub(crate) fn set_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(crate) fn set_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
