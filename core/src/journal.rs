//! Append-only JSONL journal sharing one file with Swift's `JSONLActivityEventStore`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{ActivityEvent, Confidence, CoreError, EventKind};

/// Rows written with encryption on, by any host. The protected key file sits
/// beside the journal so the app, CLI and egui shell can read the same rows.
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
    /// Grouped rows are kept for `aggregate_days` instead, which is how the
    /// long tail of "this folder grew by 4 GB in March" outlives the file-level
    /// rows it was made of without keeping the whole journal that long. Older
    /// detailed rows are rolled into one row per root and UTC day before their
    /// detail expires, matching the macOS host.
    pub fn trim(
        &self,
        retention_days: u32,
        aggregate_days: u32,
        limit_bytes: u64,
    ) -> Result<u64, CoreError> {
        if (retention_days == 0 && aggregate_days == 0 && limit_bytes == 0) || !self.path.exists() {
            return Ok(0);
        }
        // Read whole: the file is what the previous trim left, so it is
        // bounded by the cap. Stream it if a caller ever passes a cap larger
        // than it is willing to hold in memory.
        let contents = fs::read_to_string(&self.path)?;
        let original: Vec<String> = contents
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let total = original.len();
        let key = self.key_for(&contents);
        let mut kept: Vec<(usize, String)> = Vec::with_capacity(total);

        if retention_days > 0 || aggregate_days > 0 {
            let now = SystemTime::now();
            let cutoff = |days: u32| {
                (days > 0)
                    .then(|| now.checked_sub(Duration::from_secs(u64::from(days) * 86_400)))
                    .flatten()
            };
            let detailed = cutoff(retention_days);
            let aggregate = cutoff(aggregate_days.max(retention_days));
            let mut rollups: HashMap<(String, u64), (usize, ActivityEvent)> = HashMap::new();
            for (position, line) in original.iter().enumerate() {
                let Some(event) = readable(line, key.as_ref())
                    .and_then(|json| ActivityEvent::from_json_line(&json).ok())
                else {
                    // A row this build cannot read is not aged out on a guess.
                    kept.push((position, line.clone()));
                    continue;
                };
                if event.kind == EventKind::Aggregate {
                    if aggregate.is_none_or(|cutoff| event.timestamp >= cutoff) {
                        kept.push((position, line.clone()));
                    }
                    continue;
                }
                if detailed.is_none_or(|cutoff| event.timestamp >= cutoff) {
                    kept.push((position, line.clone()));
                    continue;
                }
                if !aggregate.is_none_or(|cutoff| event.timestamp >= cutoff) {
                    continue;
                }

                let day_seconds = event
                    .timestamp
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    / 86_400
                    * 86_400;
                let group = rollups
                    .entry((event.root_path.clone(), day_seconds))
                    .or_insert_with(|| {
                        (
                            position,
                            ActivityEvent {
                                kind: EventKind::Aggregate,
                                path: event.root_path.clone(),
                                root_path: event.root_path.clone(),
                                timestamp: UNIX_EPOCH + Duration::from_secs(day_seconds),
                                byte_delta: Some(0),
                                confidence: Confidence::Confirmed,
                                previous_path: None,
                                affected_item_count: 0,
                                process_name: None,
                            },
                        )
                    });
                group.0 = group.0.min(position);
                group.1.affected_item_count = group
                    .1
                    .affected_item_count
                    .saturating_add(event.affected_item_count);
                group.1.byte_delta = match (group.1.byte_delta, event.byte_delta) {
                    (Some(total), Some(delta)) => Some(total.saturating_add(delta)),
                    _ => None,
                };
                group.1.confidence = if group.1.byte_delta.is_none() {
                    Confidence::Unknown
                } else if group.1.confidence == Confidence::Confirmed
                    && event.confidence == Confidence::Confirmed
                {
                    Confidence::Confirmed
                } else {
                    Confidence::Estimated
                };
            }
            for (_, (position, event)) in rollups {
                kept.push((position, self.encoded_line(&event)?));
            }
            kept.sort_by_key(|(position, _)| *position);
        } else {
            kept = original.iter().cloned().enumerate().collect();
        }

        if limit_bytes > 0 {
            let mut bytes: u64 = kept.iter().map(|(_, line)| line.len() as u64 + 1).sum();
            let mut oldest = 0;
            while bytes > limit_bytes && oldest < kept.len() {
                bytes -= kept[oldest].1.len() as u64 + 1;
                oldest += 1;
            }
            kept.drain(..oldest);
        }

        let kept_lines: Vec<String> = kept.into_iter().map(|(_, line)| line).collect();
        if kept_lines == original {
            return Ok(0);
        }

        // Written beside the journal and renamed over it, so a crash halfway
        // through leaves the old journal rather than half a new one.
        let temporary = self.path.with_extension("jsonl.trimming");
        let mut file = fs::File::create(&temporary)?;
        set_permissions(&temporary, 0o600)?;
        for line in &kept_lines {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &self.path)?;
        Ok(total.saturating_sub(kept_lines.len()) as u64)
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

    /// Dashboard history for one root: buckets, totals, and the page of rows
    /// `query` asked for.
    pub fn load_history(
        &self,
        root_path: String,
        limit: u32,
        bucket_interval_secs: u64,
        query: crate::history::Query,
    ) -> Result<crate::HistorySnapshot, CoreError> {
        // Every retained row, not just the page: `limit` cuts the listed rows
        // inside `build_history`, after the totals are known.
        let events = self.load(root_path.clone(), u32::MAX)?;
        Ok(crate::history::build_history(
            &root_path,
            events,
            bucket_interval_secs,
            limit,
            &query,
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

    fn encoded_line(&self, event: &ActivityEvent) -> Result<String, CoreError> {
        let line = event.to_json_line()?;
        if !self.encrypt {
            return Ok(line);
        }
        let key = crate::crypt::key_or_create(&self.path)?;
        crate::crypt::seal(&key, line.as_bytes())
    }
}

/// The json behind a line, decrypting when the line is encrypted and this
/// machine has the key. `None` for a row this build cannot read.
pub(crate) fn readable(line: &str, key: Option<&[u8; 32]>) -> Option<String> {
    if !line.starts_with(ENCRYPTED_PREFIX) {
        return Some(line.to_owned());
    }
    String::from_utf8(crate::crypt::open(key?, line)?).ok()
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
