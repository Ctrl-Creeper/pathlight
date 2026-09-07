//! Append-only JSONL journal sharing one file with Swift's `JSONLActivityEventStore`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{ActivityEvent, CoreError};

/// Lines Swift wrote with "Encrypt new activity data" on. The key lives in the
/// macOS Keychain, so other platforms skip them rather than fail the whole load.
// ponytail: plaintext only; add AES-GCM + a shared key provider when a second platform needs encrypted rows.
const ENCRYPTED_PREFIX: &str = "pathlight:v1:aes-gcm:";

#[derive(Debug, uniffi::Object)]
pub struct Journal {
    path: PathBuf,
}

#[uniffi::export]
impl Journal {
    #[uniffi::constructor]
    pub fn new(path: String) -> Arc<Self> {
        Arc::new(Self {
            path: PathBuf::from(path),
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

        let mut payload = String::new();
        for event in &events {
            payload.push_str(&event.to_json_line()?);
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

    /// Newest first, then path descending, limited — the same order Swift returns.
    pub fn load(&self, root_path: String, limit: u32) -> Result<Vec<ActivityEvent>, CoreError> {
        if limit == 0 || !self.path.exists() {
            return Ok(Vec::new());
        }

        let root = root_path.trim_end_matches('/');
        let contents = fs::read_to_string(&self.path)?;
        let mut events: Vec<ActivityEvent> = contents
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with(ENCRYPTED_PREFIX))
            .filter_map(|line| ActivityEvent::from_json_line(line).ok())
            .filter(|event| event.root_path.trim_end_matches('/') == root)
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
        let events = self.load(root_path.clone(), limit)?;
        Ok(crate::history::build_history(
            &root_path,
            events,
            bucket_interval_secs,
            std::time::SystemTime::now(),
        ))
    }
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
