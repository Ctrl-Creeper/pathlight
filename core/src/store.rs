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
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use crate::attribution::AggregationOptions;
use crate::exclusion::DEFAULT_PATTERNS;
use crate::{paths, uninstall, ActivityEvent, CoreError, HistorySnapshot, Journal};

const JOURNAL_FILE: &str = "activity-events.jsonl";
const LOG_FILE: &str = "pathlight.log";
/// How large the log may get before its older half goes.
///
/// A diary of watch starts, gaps and failures is what turns "it missed
/// something" into a report somebody can read; a diary that grows without a
/// bound is the disk problem it was written to explain.
const LOG_MAX_BYTES: u64 = 256 * 1024;
const WATCHES_FILE: &str = "watches.json";
const SIZE_INDEX_FILE: &str = "activity-size-index.jsonl";
pub(crate) const LOCK_FILE: &str = "pathlight.lock";
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
/// The shipped coalescing window balances responsiveness with wake-ups.
pub const DEFAULT_LATENCY_MS: u64 = 5_000;
/// What the macOS app uses for a watch nobody is looking at. A wide window is
/// how monitoring stays cheap: the kernel wakes the process once for a
/// half-minute of churn instead of once per file.
pub const BACKGROUND_LATENCY_MS: u64 = 30_000;
/// One bucket per hour, and the rows one screen can plausibly be scrolled
/// through. Totals cover every retained row either way.
const HISTORY_BUCKET_SECS: u64 = 3600;
/// The page a host lists when nobody asked for a different one.
pub const HISTORY_ROWS: u32 = 200;

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

pub(crate) struct StorageLock {
    _process: MutexGuard<'static, ()>,
    _file: fs::File,
}

pub(crate) fn lock_directory(dir: &Path) -> io::Result<StorageLock> {
    let process = journal_lock()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    fs::create_dir_all(dir)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    file.lock()?;
    Ok(StorageLock {
        _process: process,
        _file: file,
    })
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

    fn lock(&self) -> io::Result<StorageLock> {
        lock_directory(self.dir())
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

    /// The baselines a watch measures its deltas against, kept beside the
    /// journal and sealed with the same key.
    ///
    /// Storage, not attribution, because where a measurement is kept and
    /// whether it is encrypted is this file's business — the same reason the
    /// macOS app owns its own index. The file name is the app's, so a mac
    /// running the terminal host keeps one index rather than two.
    pub fn size_index(&self) -> crate::attribution::SizeIndex {
        crate::attribution::SizeIndex::at(
            self.dir().join(SIZE_INDEX_FILE),
            self.encrypting().then(|| self.journal()),
        )
    }

    /// Appends rows to the shared journal.
    pub fn record(&self, events: Vec<ActivityEvent>) -> Result<(), CoreError> {
        let journal = self.journal_handle();
        let _guard = self.lock()?;
        journal.append(events)
    }

    /// Appends only while the caller still belongs to the current records
    /// generation. The check and append share one storage transaction so a
    /// concurrent reset cannot delete the journal between them and then have
    /// an older worker recreate it.
    pub(crate) fn record_if_generation(
        &self,
        events: Vec<ActivityEvent>,
        expected_epoch: u64,
    ) -> Result<bool, CoreError> {
        let _guard = self.lock()?;
        let settings = self.settings_unlocked();
        if settings.records_epoch != expected_epoch {
            return Ok(false);
        }
        let path = self.journal().to_string_lossy().into_owned();
        let journal = match settings.encrypt {
            true => Journal::encrypting(path),
            false => Journal::new(path),
        };
        journal.append(events)?;
        Ok(true)
    }

    /// Publishes attribution baselines only if no records reset happened
    /// after the worker accepted its batch.
    pub(crate) fn persist_index_if_generation(
        &self,
        index: &crate::attribution::SizeIndex,
        expected_epoch: u64,
    ) -> Result<bool, CoreError> {
        let _guard = self.lock()?;
        if self.settings_unlocked().records_epoch != expected_epoch {
            return Ok(false);
        }
        index.persist_while_locked()?;
        Ok(true)
    }

    /// Drops what is too old or over the cap. Returns how many rows went.
    pub fn trim_journal(&self) -> Result<u64, CoreError> {
        let (days, limit) = self.retention();
        let aggregate_days = self.aggregate_retention_days();
        let journal = self.journal_handle();
        let _guard = self.lock()?;
        journal.trim(days, aggregate_days, limit)
    }

    /// How much has been recorded, for a pane or a terminal that shows it:
    /// the size of the journal on disk and how many rows are in it.
    ///
    /// The count is a streamed read of a file the cap bounds, done when
    /// somebody asks rather than kept up to date — a counter maintained on
    /// every flush would be one more thing to get wrong about a file two
    /// hosts append to.
    pub fn recorded(&self) -> (u64, u64) {
        let Ok(file) = std::fs::File::open(self.journal()) else {
            return (0, 0);
        };
        let bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let rows = std::io::BufRead::split(std::io::BufReader::new(file), b'\n')
            .filter(|line| line.as_ref().is_ok_and(|line| !line.is_empty()))
            .count() as u64;
        (bytes, rows)
    }

    /// The file every host writes its diary of watch starts, gaps and
    /// failures to. It lives beside the journal, so the macOS app's own
    /// writer lands in the same file on a mac that also runs the terminal.
    pub fn log_file(&self) -> PathBuf {
        self.dir().join(LOG_FILE)
    }

    /// Writes one line about the watch itself.
    ///
    /// Best effort on purpose: a monitor that stops watching because it could
    /// not write its own diary is worse than a monitor with a gap in the diary.
    pub fn note(&self, line: &str) {
        use std::io::Write as _;

        let Ok(_guard) = self.lock() else { return };
        let path = self.log_file();
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_MAX_BYTES) {
            // Cut on a line boundary so the oldest surviving line is whole,
            // and cut in the middle so this happens once per doubling rather
            // than on every write.
            if let Ok(text) = fs::read_to_string(&path) {
                let kept = text
                    .char_indices()
                    .nth(text.chars().count() / 2)
                    .and_then(|(middle, _)| text[middle..].find('\n').map(|end| middle + end + 1))
                    .unwrap_or(text.len());
                let _ = fs::write(&path, &text[kept..]);
            }
        }
        let stamped = format!(
            "{} {line}\n",
            crate::event::swift_date::text(std::time::SystemTime::now())
                .unwrap_or_else(|| "unknown time".to_owned())
        );
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(stamped.as_bytes());
        }
    }

    /// The last `lines` lines of that diary, newest last, as a person reads
    /// it. Empty when nothing has been written yet, which is itself an answer.
    pub fn log_tail(&self, lines: usize) -> String {
        let Ok(_guard) = self.lock() else {
            return String::new();
        };
        let text = fs::read_to_string(self.log_file()).unwrap_or_default();
        let kept: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        kept[kept.len().saturating_sub(lines)..].join("\n")
    }

    /// Deletes everything recorded, leaving the settings alone.
    ///
    /// Records are the one thing here nobody can get back, so this exists as
    /// its own answer rather than only inside "remove Pathlight entirely": a
    /// person who wants to start the history over should not have to
    /// uninstall to do it.
    pub fn forget_records(&self) -> io::Result<()> {
        let _guard = self.lock()?;
        let mut settings = self.settings_unlocked();
        settings.records_epoch = settings.records_epoch.wrapping_add(1);
        settings.records_reset_at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        self.save_unlocked(&settings)?;
        // The baselines name the same files the rows do, and the diary names
        // the folders that were watched, so "delete every record" that left
        // either behind would be a lie about what is kept.
        let _ = std::fs::remove_file(self.dir().join(SIZE_INDEX_FILE));
        let _ = std::fs::remove_file(self.log_file());
        match std::fs::remove_file(self.journal()) {
            // Nothing recorded yet is already the state this asks for.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Every setting back to what shipped, leaving the folder list and the
    /// records alone: this is the way out of an edit whose effect the user
    /// cannot find.
    pub fn restore_default_settings(&self) -> io::Result<()> {
        self.update_settings(|settings| {
            *settings = Settings {
                roots: std::mem::take(&mut settings.roots),
                ..Settings::default()
            };
        })
    }

    /// What was recorded for one folder, folded into buckets and totals.
    ///
    /// Deliberately without the append lock. `Journal::load` already skips a
    /// line it cannot parse, and holding the lock across a whole-file read
    /// would stall every live watch's flush for as long as the read takes: a
    /// torn line costs this one read a row, the lock would cost the recording.
    pub fn history(&self, root: &str) -> Result<HistorySnapshot, CoreError> {
        self.search(root, HISTORY_ROWS, &Default::default())
    }

    /// The same history, narrowed to what somebody typed in a search box or
    /// passed on a command line.
    ///
    /// One owner for what a match is and how a page is cut, because "find
    /// every .psd that was deleted, biggest first" has to mean the same thing
    /// in a window and in a terminal.
    pub fn search(
        &self,
        root: &str,
        limit: u32,
        query: &crate::history::Query,
    ) -> Result<HistorySnapshot, CoreError> {
        self.journal_handle().load_history(
            root.to_owned(),
            limit,
            HISTORY_BUCKET_SECS,
            query.clone(),
        )
    }

    /// Every retained row for one folder, newest first — the whole record
    /// rather than the page the pane lists, because an export of the newest
    /// 200 changes is not an export of what was recorded.
    pub fn rows(&self, root: &str) -> Result<Vec<ActivityEvent>, CoreError> {
        self.journal_handle().load(root.to_owned(), u32::MAX)
    }

    /// Positive bytes already recorded since `since`. A watch reads this once
    /// when it opens so a process restart cannot reset a daily growth alert;
    /// live flushes continue to update the in-memory total incrementally.
    pub(crate) fn positive_growth_since(
        &self,
        root: &str,
        since: std::time::SystemTime,
    ) -> Result<i64, CoreError> {
        Ok(self
            .rows(root)?
            .into_iter()
            .filter(|event| event.timestamp >= since)
            .filter_map(|event| event.byte_delta)
            .map(|bytes| bytes.max(0))
            .fold(0i64, i64::saturating_add))
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
        self.update_settings(|settings| settings.encrypt = encrypt)
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
        self.update_settings(|settings| {
            settings.min_file_bytes = min_bytes;
            settings.max_file_bytes = max_bytes;
        })
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
        self.update_settings(|settings| {
            settings.roots = watches.iter().cloned().map(StoredWatch::from).collect();
        })
    }

    pub fn add_watch(&self, path: &str) -> io::Result<bool> {
        self.update_settings(|settings| {
            if settings
                .roots
                .iter()
                .any(|watch| WatchTarget::from(watch.clone()).path == path)
            {
                return false;
            }
            settings.roots.push(StoredWatch::Target {
                path: path.to_owned(),
                enabled: false,
            });
            true
        })
    }

    pub fn remove_watch(&self, path: &str) -> io::Result<bool> {
        self.update_settings(|settings| {
            let before = settings.roots.len();
            settings
                .roots
                .retain(|watch| WatchTarget::from(watch.clone()).path != path);
            settings.roots.len() != before
        })
    }

    /// Switches one folder on or off, leaving the rest of the list alone.
    /// Unknown paths are added, because a host that can name a folder is a
    /// host the user just asked to watch it.
    pub fn set_watch_enabled(&self, path: &str, enabled: bool) -> io::Result<()> {
        self.update_settings(|settings| {
            let mut watches: Vec<WatchTarget> = std::mem::take(&mut settings.roots)
                .into_iter()
                .map(WatchTarget::from)
                .collect();
            match watches.iter_mut().find(|watch| watch.path == path) {
                Some(watch) => watch.enabled = enabled,
                None => watches.push(WatchTarget {
                    path: path.to_owned(),
                    enabled,
                }),
            }
            settings.roots = watches.into_iter().map(StoredWatch::from).collect();
        })
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
        self.update_settings(|settings| settings.aggregate_retention_days = Some(days))
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
        self.update_settings(|settings| {
            settings.retention_days = Some(days);
            settings.journal_limit_bytes = Some(limit_bytes);
        })
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

    /// How much a folder may grow in a day before the user is told, or zero
    /// for never — which is the default, because an unasked-for alert about
    /// an ordinary download is how notifications get switched off wholesale.
    // ponytail: one threshold for the install, where macOS keeps one per
    // folder. Same setting, presented where these hosts keep their settings.
    pub fn growth_alert_bytes(&self) -> i64 {
        self.settings().growth_alert_bytes.unwrap_or(0).max(0)
    }

    pub fn set_growth_alert_bytes(&self, bytes: i64) -> io::Result<()> {
        self.update_settings(|settings| settings.growth_alert_bytes = Some(bytes.max(0)))
    }

    /// Whether every watch in this install is held off.
    ///
    /// One switch, so somebody about to do something noisy — a build, a
    /// restore, a big copy — can stop recording without turning watches off
    /// one at a time and having to remember afterwards which ones were on.
    pub fn paused(&self) -> bool {
        self.settings().paused.unwrap_or(false)
    }

    pub fn records_epoch(&self) -> u64 {
        self.settings().records_epoch
    }

    pub fn records_generation(&self) -> (u64, std::time::SystemTime) {
        let settings = self.settings();
        (
            settings.records_epoch,
            std::time::UNIX_EPOCH + std::time::Duration::from_nanos(settings.records_reset_at_ns),
        )
    }

    pub fn set_paused(&self, paused: bool) -> io::Result<()> {
        self.update_settings(|settings| settings.paused = Some(paused))?;
        self.note(match paused {
            true => "monitoring paused",
            false => "monitoring resumed",
        });
        Ok(())
    }

    pub fn set_latency_ms(&self, latency_ms: u64) -> io::Result<()> {
        self.update_settings(|settings| settings.latency_ms = Some(latency_ms.max(1)))
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
        self.update_settings(|settings| settings.exclusion_patterns = Some(patterns.to_vec()))
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
            minimum_recorded_byte_delta: settings
                .minimum_recorded_byte_delta
                .unwrap_or(1_024)
                .max(0),
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
        self.update_settings(|settings| {
            settings.records_file_names = Some(records_file_names);
            settings.aggregation_window_secs = Some(aggregation_window_secs);
        })
    }

    pub fn set_minimum_byte_delta(&self, bytes: i64) -> io::Result<()> {
        self.update_settings(|settings| {
            settings.minimum_recorded_byte_delta = Some(bytes.max(0));
        })
    }

    /// What was saved, or the defaults. A file this build cannot parse reads
    /// as the defaults rather than as an error the user cannot act on.
    fn settings(&self) -> Settings {
        let Ok(_guard) = self.lock() else {
            return Settings::default();
        };
        self.settings_unlocked()
    }

    fn settings_unlocked(&self) -> Settings {
        fs::read_to_string(self.dir().join(WATCHES_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn update_settings<T>(&self, update: impl FnOnce(&mut Settings) -> T) -> io::Result<T> {
        let _guard = self.lock()?;
        let mut settings = self.settings_unlocked();
        let result = update(&mut settings);
        self.save_unlocked(&settings)?;
        Ok(result)
    }

    fn save_unlocked(&self, settings: &Settings) -> io::Result<()> {
        let text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
        let target = self.dir().join(WATCHES_FILE);
        let temporary = self.dir().join("watches.json.writing");
        let mut file = fs::File::create(&temporary)?;
        use std::io::Write as _;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        if target.exists() {
            fs::remove_file(&target)?;
        }
        fs::rename(temporary, target)
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
    /// Absent means watching, which is what an install that has never been
    /// paused should do after an update.
    #[serde(default)]
    paused: Option<bool>,
    /// Absent, or zero, means the user has not asked to be told about a
    /// folder growing.
    #[serde(default)]
    growth_alert_bytes: Option<i64>,
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
    /// Incremented whenever all records are deleted. A running worker that
    /// accepted a batch under an older value must not recreate that history.
    #[serde(default)]
    records_epoch: u64,
    #[serde(default)]
    records_reset_at_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_storage_writer_waits_for_another_process_transaction() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.path().join("pathlight.lock"))
            .unwrap();
        lock.lock().unwrap();

        let storage = Storage::at(dir.path());
        let (finished, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let saved = storage.set_paused(true);
            let _ = finished.send(saved);
        });

        assert!(
            result
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "the settings write ignored the storage transaction"
        );
        std::fs::File::unlock(&lock).unwrap();
        result
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the settings write stayed blocked after unlock")
            .unwrap();
    }

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

    /// The three answers a person needs about their own records: how much is
    /// there, start it over, and put the settings back.
    #[test]
    fn records_can_be_counted_deleted_and_the_settings_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        assert_eq!(storage.recorded(), (0, 0), "nothing recorded yet");

        storage
            .record(vec![ActivityEvent {
                kind: crate::EventKind::Modified,
                path: "/watched/report.bin".to_owned(),
                root_path: "/watched".to_owned(),
                timestamp: std::time::SystemTime::now(),
                byte_delta: Some(4096),
                confidence: crate::Confidence::Confirmed,
                previous_path: None,
                affected_item_count: 1,
                process_name: None,
            }])
            .unwrap();
        let (bytes, rows) = storage.recorded();
        assert_eq!(rows, 1);
        assert!(bytes > 0);

        // Settings go back without taking the folder list with them, and the
        // records survive a settings reset.
        storage.set_watch_enabled("/watched", true).unwrap();
        storage.set_retention(30, 2_000_000).unwrap();
        storage.restore_default_settings().unwrap();
        assert_eq!(
            storage.retention(),
            (DEFAULT_RETENTION_DAYS, DEFAULT_JOURNAL_LIMIT_BYTES)
        );
        assert_eq!(storage.watches().len(), 1, "the folder list was reset too");
        assert_eq!(storage.recorded().1, 1, "the records were reset too");

        // And deleting the records leaves the settings alone — while taking
        // the baselines, which name the same files.
        let index = storage.size_index();
        index.record("watch", "/watched/report.bin", Some(4096));
        index.persist().unwrap();
        storage.forget_records().unwrap();
        assert_eq!(storage.recorded(), (0, 0));
        assert_eq!(
            storage.size_index().peek("watch", "/watched/report.bin"),
            None
        );
        assert_eq!(storage.watches().len(), 1);
        // Twice is not an error: nothing recorded is the state it asks for.
        storage.forget_records().unwrap();
    }

    #[test]
    fn an_old_size_batch_cannot_recreate_the_index_after_records_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let generation = storage.records_epoch();
        let index = storage.size_index();
        index.record("watch", "/watched/old.bin", Some(4_096));

        storage.forget_records().unwrap();

        assert!(!storage
            .persist_index_if_generation(&index, generation)
            .unwrap());
        assert!(!dir.path().join(SIZE_INDEX_FILE).exists());
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
        assert_eq!(storage.latency_ms(), 5_000);
        assert_eq!(storage.patterns(), DEFAULT_PATTERNS.to_vec());
        assert_eq!(storage.options().minimum_recorded_byte_delta, 1_024);
        // Nobody is told about a folder growing until they ask to be.
        assert_eq!(storage.growth_alert_bytes(), 0);

        storage.set_retention(30, 2_000_000).unwrap();
        storage.set_latency_ms(BACKGROUND_LATENCY_MS).unwrap();
        storage.set_patterns(&["*.log".to_owned()]).unwrap();
        storage.set_minimum_byte_delta(1024).unwrap();
        storage.set_growth_alert_bytes(5_000_000_000).unwrap();

        let reopened = Storage::at(dir.path());
        assert_eq!(reopened.retention(), (30, 2_000_000));
        assert_eq!(reopened.latency_ms(), BACKGROUND_LATENCY_MS);
        assert_eq!(reopened.patterns(), ["*.log"]);
        assert_eq!(reopened.options().minimum_recorded_byte_delta, 1024);
        assert_eq!(reopened.growth_alert_bytes(), 5_000_000_000);
        // Recording everything is a choice, and one that has to survive a
        // restart rather than reading as "never edited".
        reopened.set_patterns(&[]).unwrap();
        assert!(Storage::at(dir.path()).patterns().is_empty());
        // A retention of nothing is refused rather than saved.
        assert!(reopened.set_retention(0, 1).is_err());
        assert_eq!(Storage::at(dir.path()).retention(), (30, 2_000_000));
    }

    /// The diary answers "what happened while nobody was looking", so what it
    /// must never do is grow without a bound or lose the newest line to the
    /// trim that bounds it.
    #[test]
    fn the_diary_keeps_the_newest_lines_and_stays_under_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());

        assert!(storage.log_tail(50).is_empty());
        storage.note("watch opened on /Users/example/Downloads");
        storage.note("caught up on /Users/example/Downloads: 2 row(s) recovered from a gap");
        let tail = storage.log_tail(50);
        assert_eq!(tail.lines().count(), 2);
        assert!(
            tail.lines().next().unwrap().contains("watch opened"),
            "{tail}"
        );
        assert!(tail.lines().last().unwrap().contains("caught up"), "{tail}");
        // Only what was asked for, newest last.
        assert!(storage.log_tail(1).contains("caught up"));

        let long = "x".repeat(1024);
        for _ in 0..(LOG_MAX_BYTES / 1024 + 8) {
            storage.note(&long);
        }
        storage.note("the newest line");
        let bytes = fs::metadata(storage.log_file()).unwrap().len();
        assert!(bytes <= LOG_MAX_BYTES + 8 * 1024, "{bytes} bytes");
        assert!(storage.log_tail(1).ends_with("the newest line"));
        // Every surviving line is whole: a trim that cut mid-line would leave
        // a first row nobody can read.
        for line in storage.log_tail(usize::MAX).lines() {
            assert!(line.starts_with("20"), "{line}");
        }

        // The diary names watched folders, so it goes when the records do.
        storage.forget_records().unwrap();
        assert!(storage.log_tail(50).is_empty());
    }
}
