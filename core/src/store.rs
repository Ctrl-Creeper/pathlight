//! Where a host keeps what it recorded, the settings it recorded under, and
//! the one rule about that place.
//!
//! Every host that is not the macOS app reads this: the egui window, the
//! command line, and any test of either. One settings file and one journal per
//! install, so a folder watched from a terminal is the same watch the window
//! lists — and the guard that keeps a watch out of Pathlight's own storage is
//! written once.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::attribution::AggregationOptions;
use crate::exclusion::DEFAULT_PATTERNS;
use crate::{paths, uninstall, ActivityEvent, CoreError, HistorySnapshot, Journal};

const JOURNAL_FILE: &str = "activity-events.jsonl";
const WATCHES_FILE: &str = "watches.json";
/// How long a row is kept, and how large the journal may get, until the user
/// says otherwise. The same numbers the macOS app ships
/// (`ActivityStoragePreferences.defaults`), so one journal read on either host
/// means the same thing.
pub const DEFAULT_RETENTION_DAYS: u32 = 180;
/// Grouped rows outlive the file-level rows they were made of: "this folder
/// grew by 4 GB in March" is worth keeping long after the list of files is.
pub const DEFAULT_AGGREGATE_RETENTION_DAYS: u32 = 730;
pub const DEFAULT_JOURNAL_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;
/// How wide a group is when rows are grouped rather than named, matching the
/// macOS app's long-term default.
pub const DEFAULT_AGGREGATION_WINDOW_SECS: u64 = 5 * 60;
/// The interactive coalescing window: wide enough that a save is one event
/// rather than five, short enough to feel immediate.
pub const DEFAULT_LATENCY_MS: u64 = 250;
/// What the macOS app uses for a watch nobody is looking at. A wide window is
/// how monitoring stays cheap: the kernel wakes the process once for a
/// half-minute of churn instead of once per file.
pub const BACKGROUND_LATENCY_MS: u64 = 30_000;
/// One bucket per hour, and the rows one screen can plausibly be scrolled
/// through. Totals cover every retained row either way.
const HISTORY_BUCKET_SECS: u64 = 3600;
const HISTORY_ROWS: u32 = 200;

/// Every append and every trim in this process takes this.
///
/// Two watches share one journal file, and a trim rewrites that file: an
/// append landing between the trim's read and its rename is a row the trim
/// silently deletes. Poisoning is ignored on purpose — the guarded value is
/// `()`, so a thread that panicked holding it left nothing inconsistent, and
/// refusing every later append would lose far more than it protects.
fn journal_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

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

    /// Appends rows to the shared journal.
    pub fn record(&self, events: Vec<ActivityEvent>) -> Result<(), CoreError> {
        let _guard = journal_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.journal_handle().append(events)
    }

    /// Drops what is too old or over the cap. Returns how many rows went.
    pub fn trim_journal(&self) -> Result<u64, CoreError> {
        let _guard = journal_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (days, limit) = self.retention();
        self.journal_handle()
            .trim(days, self.aggregate_retention_days(), limit)
    }

    /// What was recorded for one folder, folded into buckets and totals.
    ///
    /// Deliberately without the append lock. `Journal::load` already skips a
    /// line it cannot parse, and holding the lock across a whole-file read
    /// would stall every live watch's flush for as long as the read takes: a
    /// torn line costs this one read a row, the lock would cost the recording.
    pub fn history(&self, root: &str) -> Result<HistorySnapshot, CoreError> {
        self.journal_handle()
            .load_history(root.to_owned(), HISTORY_ROWS, HISTORY_BUCKET_SECS)
    }

    /// Every retained row for one folder, newest first — the whole record
    /// rather than the page the pane lists, because an export of the newest
    /// 200 changes is not an export of what was recorded.
    pub fn rows(&self, root: &str) -> Result<Vec<ActivityEvent>, CoreError> {
        self.journal_handle().load(root.to_owned(), u32::MAX)
    }

    /// Whether new rows are written encrypted.
    ///
    /// Read from the file each time rather than cached: the setting is a
    /// checkbox, and a watch already running is exactly the writer that has to
    /// notice it was ticked.
    // ponytail: one small read per flush. Cache it if a flush ever shows up in
    // a measurement, not before.
    pub fn encrypting(&self) -> bool {
        self.settings().encrypt
    }

    pub fn set_encrypting(&self, encrypt: bool) -> io::Result<()> {
        let mut settings = self.settings();
        settings.encrypt = encrypt;
        self.save(&settings)
    }

    fn journal_handle(&self) -> std::sync::Arc<Journal> {
        let path = self.journal().to_string_lossy().into_owned();
        match self.encrypting() {
            true => Journal::encrypting(path),
            false => Journal::new(path),
        }
    }

    /// The sizes of file every watch here records at all, in bytes, smallest
    /// first; `None` is no bound. Judged on the file's own size, not on how
    /// much of it changed.
    ///
    /// One pair for the install rather than one per watch, which is the shape
    /// this settings file already has for retention and encryption.
    // ponytail: per-watch bounds when somebody watches two folders that want
    // different ones. Until then a second list in `watches.json` is a shape
    // nobody asked for.
    pub fn size_bounds(&self) -> (Option<i64>, Option<i64>) {
        let settings = self.settings();
        (settings.min_file_bytes, settings.max_file_bytes)
    }

    pub fn set_size_bounds(
        &self,
        min_bytes: Option<i64>,
        max_bytes: Option<i64>,
    ) -> io::Result<()> {
        let mut settings = self.settings();
        settings.min_file_bytes = min_bytes;
        settings.max_file_bytes = max_bytes;
        self.save(&settings)
    }

    /// The folders the user chose, and whether each one is being watched.
    pub fn watches(&self) -> Vec<WatchTarget> {
        self.settings()
            .roots
            .into_iter()
            .map(WatchTarget::from)
            .collect()
    }

    pub fn set_watches(&self, watches: &[WatchTarget]) -> io::Result<()> {
        let mut settings = self.settings();
        settings.roots = watches.iter().cloned().map(StoredWatch::from).collect();
        self.save(&settings)
    }

    /// Switches one folder on or off, leaving the rest of the list alone.
    /// Unknown paths are added, because a host that can name a folder is a
    /// host the user just asked to watch it.
    pub fn set_watch_enabled(&self, path: &str, enabled: bool) -> io::Result<()> {
        let mut watches = self.watches();
        match watches.iter_mut().find(|watch| watch.path == path) {
            Some(watch) => watch.enabled = enabled,
            None => watches.push(WatchTarget {
                path: path.to_owned(),
                enabled,
            }),
        }
        self.set_watches(&watches)
    }

    /// How long a row is kept and how large the journal may get.
    pub fn retention(&self) -> (u32, u64) {
        let settings = self.settings();
        (
            settings.retention_days.unwrap_or(DEFAULT_RETENTION_DAYS),
            settings
                .journal_limit_bytes
                .unwrap_or(DEFAULT_JOURNAL_LIMIT_BYTES),
        )
    }

    /// How long a grouped row is kept, which is longer than a file-level one:
    /// the point of grouping is the history that outlives the detail.
    pub fn aggregate_retention_days(&self) -> u32 {
        self.settings()
            .aggregate_retention_days
            .unwrap_or(DEFAULT_AGGREGATE_RETENTION_DAYS)
    }

    pub fn set_aggregate_retention_days(&self, days: u32) -> io::Result<()> {
        if days == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a retention of nothing would delete every grouped row as soon as it was written",
            ));
        }
        let mut settings = self.settings();
        settings.aggregate_retention_days = Some(days);
        self.save(&settings)
    }

    /// Neither is allowed to be zero: a retention of nothing is a monitor that
    /// records and then immediately forgets, which reads as a broken journal
    /// rather than as a setting.
    pub fn set_retention(&self, days: u32, limit_bytes: u64) -> io::Result<()> {
        if days == 0 || limit_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a retention of nothing would delete every row as soon as it was written",
            ));
        }
        let mut settings = self.settings();
        settings.retention_days = Some(days);
        settings.journal_limit_bytes = Some(limit_bytes);
        self.save(&settings)
    }

    /// How long the watcher coalesces before it hands events over.
    ///
    /// Two values are worth having and the file holds a number, so a host can
    /// offer the pair ([`DEFAULT_LATENCY_MS`], [`BACKGROUND_LATENCY_MS`]) as a
    /// choice and anybody editing the file by hand can still say 5 seconds.
    pub fn latency_ms(&self) -> u64 {
        self.settings()
            .latency_ms
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_LATENCY_MS)
    }

    pub fn set_latency_ms(&self, latency_ms: u64) -> io::Result<()> {
        let mut settings = self.settings();
        settings.latency_ms = Some(latency_ms.max(1));
        self.save(&settings)
    }

    /// What every watch here leaves out, on top of the storage guard: the
    /// noise nobody asked to be told about. Never edited means the shipped
    /// list, and an empty list means "record everything", which is a different
    /// answer and has to survive a restart as one.
    pub fn patterns(&self) -> Vec<String> {
        self.settings().exclusion_patterns.unwrap_or_else(|| {
            DEFAULT_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect()
        })
    }

    pub fn set_patterns(&self, patterns: &[String]) -> io::Result<()> {
        let mut settings = self.settings();
        settings.exclusion_patterns = Some(patterns.to_vec());
        self.save(&settings)
    }

    /// What is recorded and how it is folded together: the sizes of file this
    /// install watches at all, and how much has to have changed before a row
    /// is written.
    ///
    /// One owner, so a watch opened from a terminal aggregates exactly the way
    /// the window's would.
    pub fn options(&self) -> AggregationOptions {
        let settings = self.settings();
        let records_file_names = settings.records_file_names.unwrap_or(true);
        AggregationOptions {
            minimum_recorded_byte_delta: settings.minimum_recorded_byte_delta.unwrap_or(0).max(0),
            min_file_bytes: settings.min_file_bytes,
            max_file_bytes: settings.max_file_bytes,
            records_file_names,
            // Named rows are never grouped, so a window left over from a spell
            // of grouping cannot quietly fold them.
            aggregation_window_secs: match records_file_names {
                true => 0,
                false => settings
                    .aggregation_window_secs
                    .unwrap_or(DEFAULT_AGGREGATION_WINDOW_SECS)
                    .max(1),
            },
        }
    }

    /// Whether rows name the files that changed, or say how much changed in
    /// the folder over a window.
    ///
    /// Grouping is what makes watching a whole disk for a year affordable: one
    /// row per folder per window instead of one per file. Naming files is what
    /// makes a watch answer "what happened to my document", so this is the
    /// user's choice, per install, exactly as it is per watch on macOS.
    pub fn set_recording(
        &self,
        records_file_names: bool,
        aggregation_window_secs: u64,
    ) -> io::Result<()> {
        let mut settings = self.settings();
        settings.records_file_names = Some(records_file_names);
        settings.aggregation_window_secs = Some(aggregation_window_secs);
        self.save(&settings)
    }

    pub fn set_minimum_byte_delta(&self, bytes: i64) -> io::Result<()> {
        let mut settings = self.settings();
        settings.minimum_recorded_byte_delta = Some(bytes.max(0));
        self.save(&settings)
    }

    /// What was saved, or the defaults. A file this build cannot parse reads
    /// as the defaults rather than as an error the user cannot act on.
    fn settings(&self) -> Settings {
        fs::read_to_string(self.dir().join(WATCHES_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, settings: &Settings) -> io::Result<()> {
        fs::create_dir_all(self.dir())?;
        let text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
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

/// One watched folder, and whether a host that resumes watches at startup
/// opens this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchTarget {
    pub path: String,
    pub enabled: bool,
}

/// A folder in the settings file, in either shape it has been written in.
///
/// Untagged, so a list written before a watch could be left switched off — a
/// plain array of strings — still reads as the folders it named. Losing
/// somebody's watch list to a format change would be the worst kind of
/// upgrade, and the alternative is a migration nobody can test twice.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum StoredWatch {
    Target {
        path: String,
        #[serde(default)]
        enabled: bool,
    },
    Path(String),
}

impl From<StoredWatch> for WatchTarget {
    fn from(stored: StoredWatch) -> Self {
        match stored {
            StoredWatch::Target { path, enabled } => Self { path, enabled },
            // A list from before the flag existed was the list of folders
            // being watched, so that is what it still means.
            StoredWatch::Path(path) => Self {
                path,
                enabled: true,
            },
        }
    }
}

impl From<WatchTarget> for StoredWatch {
    fn from(watch: WatchTarget) -> Self {
        Self::Target {
            path: watch.path,
            enabled: watch.enabled,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Settings {
    #[serde(default)]
    roots: Vec<StoredWatch>,
    /// Off by default. Encryption whose key is lost is history that is lost,
    /// so this is the user's choice to make and not a default to surprise
    /// them with.
    #[serde(default)]
    encrypt: bool,
    /// Absent in a file written before size bounds existed, which is also how
    /// "no bound" is stored.
    #[serde(default)]
    min_file_bytes: Option<i64>,
    #[serde(default)]
    max_file_bytes: Option<i64>,
    /// Absent means the shipped default, which is how every setting added
    /// after a file was first written has to read.
    #[serde(default)]
    retention_days: Option<u32>,
    #[serde(default)]
    aggregate_retention_days: Option<u32>,
    #[serde(default)]
    journal_limit_bytes: Option<u64>,
    #[serde(default)]
    minimum_recorded_byte_delta: Option<i64>,
    #[serde(default)]
    latency_ms: Option<u64>,
    /// Absent means rows name the files that changed, which is what a person
    /// opening a monitor for the first time is looking for.
    #[serde(default)]
    records_file_names: Option<bool>,
    #[serde(default)]
    aggregation_window_secs: Option<u64>,
    /// Absent means the shipped patterns; an empty list means the user asked
    /// for everything to be recorded.
    #[serde(default)]
    exclusion_patterns: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grouping is the setting that makes a whole-disk watch affordable, and
    /// the pair has to stay coherent: named rows are never grouped, and a
    /// grouped one has a window to group over.
    #[test]
    fn choosing_grouped_rows_gives_them_a_window_and_naming_files_takes_it_away() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        // Shipped: files by name, no grouping.
        let options = storage.options();
        assert!(options.records_file_names);
        assert_eq!(options.aggregation_window_secs, 0);

        storage.set_recording(false, 60).unwrap();
        let options = storage.options();
        assert!(!options.records_file_names);
        assert_eq!(options.aggregation_window_secs, 60);

        // Back to names, and the window it was grouping over cannot quietly
        // fold the named rows.
        storage.set_recording(true, 60).unwrap();
        assert_eq!(storage.options().aggregation_window_secs, 0);

        // Grouped rows are kept longer than file-level ones, and neither
        // retention may be nothing.
        assert_eq!(
            storage.aggregate_retention_days(),
            DEFAULT_AGGREGATE_RETENTION_DAYS
        );
        storage.set_aggregate_retention_days(365).unwrap();
        assert_eq!(storage.aggregate_retention_days(), 365);
        assert!(storage.set_aggregate_retention_days(0).is_err());
        assert_eq!(storage.aggregate_retention_days(), 365);
    }

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
    fn saved_size_bounds_survive_a_restart_and_default_to_no_bound() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        assert_eq!(storage.size_bounds(), (None, None));
        storage.set_size_bounds(Some(1_000_000), None).unwrap();

        assert_eq!(
            Storage::at(dir.path()).size_bounds(),
            (Some(1_000_000), None)
        );
        // And a folder list saved afterwards keeps them: both live in one
        // file, so a write of either must not drop the other.
        storage.set_watch_enabled("/a", true).unwrap();
        assert_eq!(
            Storage::at(dir.path()).size_bounds(),
            (Some(1_000_000), None)
        );
    }

    #[test]
    fn a_saved_folder_list_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        assert!(storage.watches().is_empty());
        storage
            .set_watches(&[watch("/a", true), watch("/b", false)])
            .unwrap();

        assert_eq!(
            Storage::at(dir.path()).watches(),
            [watch("/a", true), watch("/b", false)]
        );
        // Switching one folder leaves the other's state alone, which is the
        // whole reason the flag is per folder.
        storage.set_watch_enabled("/b", true).unwrap();
        storage.set_watch_enabled("/c", false).unwrap();
        assert_eq!(
            Storage::at(dir.path()).watches(),
            [watch("/a", true), watch("/b", true), watch("/c", false)]
        );
    }

    fn watch(path: &str, enabled: bool) -> WatchTarget {
        WatchTarget {
            path: path.to_owned(),
            enabled,
        }
    }

    /// The upgrade: a file written before a watch could be switched off holds
    /// a plain array of paths. Reading it as an empty list would silently
    /// forget every folder somebody was watching.
    #[test]
    fn a_folder_list_from_an_older_build_still_names_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(
            dir.path().join(WATCHES_FILE),
            r#"{"roots":["/a","/b"],"encrypt":true}"#,
        )
        .unwrap();
        let storage = Storage::at(dir.path());

        assert_eq!(storage.watches(), [watch("/a", true), watch("/b", true)]);
        assert!(storage.encrypting());
    }

    /// Every knob a host offers, round-tripped through the file: a setting
    /// that is not read back is a setting the next watch ignores.
    #[test]
    fn saved_settings_survive_a_restart_and_fall_back_to_the_shipped_ones() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        assert_eq!(
            storage.retention(),
            (DEFAULT_RETENTION_DAYS, DEFAULT_JOURNAL_LIMIT_BYTES)
        );
        assert_eq!(storage.latency_ms(), DEFAULT_LATENCY_MS);
        assert_eq!(storage.patterns(), DEFAULT_PATTERNS.to_vec());
        assert_eq!(storage.options().minimum_recorded_byte_delta, 0);

        storage.set_retention(30, 2_000_000).unwrap();
        storage.set_latency_ms(BACKGROUND_LATENCY_MS).unwrap();
        storage.set_patterns(&["*.log".to_owned()]).unwrap();
        storage.set_minimum_byte_delta(1024).unwrap();

        let reopened = Storage::at(dir.path());
        assert_eq!(reopened.retention(), (30, 2_000_000));
        assert_eq!(reopened.latency_ms(), BACKGROUND_LATENCY_MS);
        assert_eq!(reopened.patterns(), ["*.log"]);
        assert_eq!(reopened.options().minimum_recorded_byte_delta, 1024);
        // Recording everything is a choice, and one that has to survive a
        // restart rather than reading as "never edited".
        reopened.set_patterns(&[]).unwrap();
        assert!(Storage::at(dir.path()).patterns().is_empty());
        // A retention of nothing is refused rather than saved.
        assert!(reopened.set_retention(0, 1).is_err());
        assert_eq!(Storage::at(dir.path()).retention(), (30, 2_000_000));
    }
}
