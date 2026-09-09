//! Where this shell keeps what it wrote, and the one rule about that place.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pathlight_core::{paths, uninstall};

const JOURNAL_FILE: &str = "activity-events.jsonl";
const WATCHES_FILE: &str = "watches.json";

/// Pathlight's own storage directory, plus the spelling used to keep every
/// watch out of it.
#[derive(Clone, Debug)]
pub struct Storage {
    /// Resolved and [`paths::normalize`]d. One spelling serves as the
    /// directory, the journal's parent and the exclusion guard, because two
    /// spellings of one directory is the bug `paths` exists to prevent — and
    /// here it is the bug that lets Pathlight record itself.
    // ponytail: forward slashes on Windows too. Win32 accepts them, and a
    // second spelling for display would be a second spelling.
    path: String,
}

impl Storage {
    /// The real storage location, or `None` when the OS will not say where
    /// home is. Writing to a guessed directory is how a journal ends up
    /// somewhere no uninstall looks.
    pub fn current() -> Option<Self> {
        uninstall::current_data_dir().map(Self::at)
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: paths::normalize(&resolved(&dir.into()).to_string_lossy()),
        }
    }

    pub fn dir(&self) -> &Path {
        Path::new(&self.path)
    }

    pub fn journal(&self) -> PathBuf {
        self.dir().join(JOURNAL_FILE)
    }

    /// True when `path` is this storage or something inside it.
    ///
    /// Recording a journal write appends another row, whose write is another
    /// change, which is a feedback loop no byte threshold stops: attribution
    /// measures a path before any threshold applies. So this is checked
    /// before attribution, and it is not a user setting — a watch that could
    /// be pointed here would spend the disk it is supposed to be watching.
    pub fn is_own(&self, path: &str) -> bool {
        path == self.path || paths::is_inside(&self.path, path)
    }

    /// The folders the user chose, dropping any that no longer exist so a
    /// stale entry cannot look like a live watch.
    pub fn load_watches(&self) -> Vec<String> {
        let text = match fs::read_to_string(self.dir().join(WATCHES_FILE)) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        };
        serde_json::from_str::<Watches>(&text)
            .map(|watches| watches.roots)
            .unwrap_or_default()
    }

    pub fn save_watches(&self, roots: &[String]) -> io::Result<()> {
        fs::create_dir_all(self.dir())?;
        let text = serde_json::to_string_pretty(&Watches {
            roots: roots.to_vec(),
        })
        .map_err(io::Error::other)?;
        fs::write(self.dir().join(WATCHES_FILE), text)
    }
}

/// The spelling a watcher backend will use for `dir`.
///
/// Backends report resolved paths: on macOS `/var` is a symlink to
/// `/private/var`, and a home directory can be a symlink anywhere. A guard
/// holding the unresolved spelling silently never matches the events it exists
/// to drop, which is the feedback loop with extra steps. The directory does not
/// exist yet on a first run, so the nearest existing ancestor is resolved and
/// the rest joined back on.
fn resolved(dir: &Path) -> PathBuf {
    if let Ok(real) = dir.canonicalize() {
        return real;
    }
    match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => dir.to_owned(),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Watches {
    #[serde(default)]
    roots: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sibling whose name merely starts the same way is somebody else's
    /// folder, so the separator has to be part of the match. Getting this
    /// wrong in the lenient direction excludes a user's folder from their own
    /// watch; getting it wrong the other way is the feedback loop.
    ///
    /// Built on a real directory because the guard resolves symlinks, and a
    /// made-up path resolves differently per host — `/home` on macOS is
    /// `/System/Volumes/Data/home`.
    #[test]
    fn own_storage_is_matched_on_path_boundaries() {
        let parent = tempfile::tempdir().unwrap();
        let base = paths::normalize(&parent.path().canonicalize().unwrap().to_string_lossy());
        let storage = Storage::at(parent.path().join("pathlight"));

        assert!(storage.is_own(&format!("{base}/pathlight")));
        assert!(storage.is_own(&format!("{base}/pathlight/activity-events.jsonl")));
        assert!(!storage.is_own(&format!("{base}/pathlight-backup/x")));
        assert!(!storage.is_own(&base));
        assert!(!storage.is_own(&format!("{base}/Documents/report.pdf")));
    }

    /// The journal sits in the storage directory, so a watch that excludes the
    /// directory excludes the journal. Spelled out because the two are set
    /// independently and only their agreement prevents the loop.
    #[test]
    fn the_journal_is_inside_the_directory_every_watch_excludes() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let journal = paths::normalize(&storage.journal().to_string_lossy());

        assert!(storage.is_own(&journal), "{journal} would be recorded");
    }

    /// The defect this caught: the guard held `/var/...` while the watcher
    /// reported `/private/var/...`, so the exclusion matched nothing at all.
    /// Only the resolved spelling has to match — that is the one backends
    /// report — but it has to match even when the configured path was a link.
    #[cfg(unix)]
    #[test]
    fn the_guard_holds_the_spelling_a_watcher_reports() {
        let real = tempfile::tempdir().unwrap();
        let link = real
            .path()
            .parent()
            .unwrap()
            .join(format!("pathlight-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        let storage = Storage::at(link.join("records"));
        let resolved_event = real
            .path()
            .canonicalize()
            .unwrap()
            .join("records")
            .join("activity-events.jsonl");
        let _ = std::fs::remove_file(&link);

        assert!(storage.is_own(&paths::normalize(&resolved_event.to_string_lossy())));
    }

    #[test]
    fn a_saved_folder_list_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        assert!(storage.load_watches().is_empty());
        storage
            .save_watches(&["/a".to_owned(), "/b".to_owned()])
            .unwrap();

        assert_eq!(Storage::at(dir.path()).load_watches(), ["/a", "/b"]);
    }
}
