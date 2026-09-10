//! How a change reads to a person: its size, its kind, when it happened, and
//! what a finding is called.
//!
//! Shared because the same row is shown in a window, printed by the command
//! line and written into an export, and a size that reads three ways is three
//! answers to one question. The macOS app keeps its own formatters — it has a
//! locale and a timezone database to do it properly.

use std::time::SystemTime;

use crate::anomaly::{Anomaly, AnomalyKind, WINDOW};
use crate::EventKind;

/// What happened, in one word.
pub fn kind_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Created => "new",
        EventKind::Modified => "changed",
        EventKind::Deleted => "deleted",
        EventKind::Moved => "moved",
        EventKind::Aggregate => "group",
    }
}

/// The last component of a path: what the user calls the folder.
pub fn leaf(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// A signed byte count as a person reads it. The sign is always there: a
/// change of 4 KB and a loss of 4 KB are not the same event.
pub fn human_bytes(bytes: i64) -> String {
    let sign = if bytes < 0 { "-" } else { "+" };
    let mut value = bytes.unsigned_abs() as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 || unit == "TB" {
            let text = if unit == "B" || value >= 100.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.1}")
            };
            return format!("{sign}{text} {unit}");
        }
        value /= 1024.0;
    }
    unreachable!()
}

/// Relative rather than clock time on purpose: a local time needs a timezone
/// database, and a timestamp shown in the wrong zone is worse than none. An
/// export writes [`crate::ActivityEvent::timestamp_text`] instead, which is
/// UTC and unambiguous.
pub fn elapsed(since: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(since)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..=4 => "just now".to_owned(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// A finding's headline, naming the folder it is about.
pub fn alert_title(alert: &Anomaly, root: &str) -> String {
    let name = leaf(root);
    match alert.kind {
        AnomalyKind::Removal => format!("Many files deleted in {name}"),
        AnomalyKind::Burst => format!("{name} is filling up fast"),
    }
}

/// The sentence under it.
pub fn alert_body(alert: &Anomaly) -> String {
    let minutes = WINDOW.as_secs() / 60;
    // Without the sign the shared formatter puts on a delta: the wording
    // already says which direction this went.
    let size = human_bytes(alert.bytes);
    let size = size.trim_start_matches('+');
    match alert.kind {
        AnomalyKind::Removal => format!(
            "{} items ({size}) removed in the last {minutes} minutes.",
            alert.items
        ),
        AnomalyKind::Burst => format!("{size} written in the last {minutes} minutes."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_a_person_says_them() {
        assert_eq!(human_bytes(0), "+0 B");
        assert_eq!(human_bytes(-2048), "-2.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "+1.5 MB");
        // The magnitude fits in u64 even at the extreme, which `abs` would not.
        assert!(human_bytes(i64::MIN).ends_with(" TB"));
    }

    #[test]
    fn a_folder_is_named_by_its_last_component() {
        assert_eq!(leaf("C:/Users/x/Downloads"), "Downloads");
        assert_eq!(leaf("/watched/folder/"), "folder");
        assert_eq!(leaf("/"), "/");
    }

    /// A finding reads as a size rather than as a delta, and names the folder:
    /// a notification that says "+4.7 GB" about nowhere in particular is a
    /// notification nobody can act on.
    #[test]
    fn a_flood_of_bytes_is_reported_with_its_size() {
        let alert = Anomaly {
            kind: AnomalyKind::Burst,
            items: 1,
            bytes: 5_000_000_000,
        };

        let body = alert_body(&alert);
        assert!(body.contains("4.7 GB"), "unexpected wording: {body}");
        assert!(!body.contains('+'), "a size was written as a delta: {body}");
        assert!(alert_title(&alert, "/watched").contains("watched"));
    }
}
