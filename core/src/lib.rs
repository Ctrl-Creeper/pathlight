//! Pathlight core: platform-neutral event model, JSONL journal, and a
//! notify-backed folder watcher, exposed to Swift/Kotlin through UniFFI.
//!
//! The journal line format is byte-for-byte compatible with the Swift
//! `JSONLActivityEventStore` so both sides can read one file.

uniffi::setup_scaffolding!();

pub mod attribution;
pub mod event;
pub mod evidence;
pub mod exclusion;
#[cfg(target_os = "linux")]
mod fanotify;
#[cfg(target_os = "macos")]
mod fsevents;
pub mod history;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod inotify;
pub mod journal;
pub mod measurement;
pub mod monitor;
#[cfg(not(target_os = "macos"))]
mod notify_backend;
pub mod paths;
pub mod recording;
pub mod snapshot;
pub mod uninstall;
#[cfg(windows)]
mod usn;

pub use attribution::{ActivityAttributor, AggregationOptions, Attributor, SizeIndex, SizeLookup};
pub use event::{ActivityEvent, Confidence, EventKind};
pub use exclusion::ExclusionFilter;
pub use history::{HistoryBucket, HistorySnapshot};
pub use journal::Journal;
pub use monitor::{
    watcher_capabilities, ActivityListener, Capabilities, Change, ChangeKind, StreamEvent, Watcher,
};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("I/O error: {message}")]
    Io { message: String },
    #[error("encoding error: {message}")]
    Encoding { message: String },
    #[error("watch error: {message}")]
    Watch { message: String },
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        CoreError::Io {
            message: error.to_string(),
        }
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        CoreError::Encoding {
            message: error.to_string(),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl From<notify::Error> for CoreError {
    fn from(error: notify::Error) -> Self {
        CoreError::Watch {
            message: match error.kind {
                // inotify needs one kernel watch per directory and the per-user
                // limit is small on many distributions. What notify passes on is
                // "No space left on device", which sends people looking at their
                // free disk. Name the limit that was actually reached, its current
                // value, and who can raise it.
                notify::ErrorKind::MaxFilesWatch => format!(
                    "this folder has more directories than one user may watch at once \
                     (fs.inotify.max_user_watches is {}). Watch a smaller folder, or \
                     raise the limit with `sudo sysctl -w fs.inotify.max_user_watches=<larger>`; \
                     Pathlight does not change system settings itself.",
                    watch_limit()
                ),
                _ => error.to_string(),
            },
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn watch_limit() -> String {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|_| "unreadable".to_owned())
}

/// Crate version, handy for proving the host actually linked this library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn hitting_the_inotify_watch_limit_says_which_limit_and_how_to_raise_it() {
        let error = CoreError::from(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
        let CoreError::Watch { message } = error else {
            panic!("the watch limit is a watch error");
        };
        assert!(message.contains("fs.inotify.max_user_watches"), "{message}");
        assert!(message.contains("sysctl"), "{message}");
        assert!(!message.contains("No space left"), "{message}");
    }
}
