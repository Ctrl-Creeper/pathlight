//! Pathlight core: platform-neutral event model, JSONL journal, and a
//! notify-backed folder watcher, exposed to Swift/Kotlin through UniFFI.
//!
//! The journal line format is byte-for-byte compatible with the Swift
//! `JSONLActivityEventStore` so both sides can read one file.

uniffi::setup_scaffolding!();

pub mod event;
pub mod journal;
pub mod monitor;

pub use event::{ActivityEvent, Confidence, EventKind};
pub use journal::Journal;
pub use monitor::{ActivityListener, Change, ChangeKind, StreamEvent, Watcher};

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
        CoreError::Io { message: error.to_string() }
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        CoreError::Encoding { message: error.to_string() }
    }
}

impl From<notify::Error> for CoreError {
    fn from(error: notify::Error) -> Self {
        CoreError::Watch { message: error.to_string() }
    }
}
