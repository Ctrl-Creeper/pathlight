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
pub mod snapshot;

pub use attribution::{AggregationOptions, Attributor, SizeIndex};
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
            message: error.to_string(),
        }
    }
}

/// Crate version, handy for proving the host actually linked this library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
