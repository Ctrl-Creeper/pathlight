//! What is worth interrupting somebody for.
//!
//! A watch keeps running after its window closes, so a notification is the
//! only way a background watch can tell anybody anything — which sets the bar
//! high. Which findings clear it, and the window and thresholds that decide,
//! belong to `pathlight_core::anomaly` — both hosts notify from those numbers.
//! What is left here is the part that should not be shared: the wording, and
//! saying it at most once per window.

use std::collections::VecDeque;
use std::time::SystemTime;

use pathlight_core::anomaly::{anomalies, Anomaly, AnomalyKind, WINDOW};
use pathlight_core::ActivityEvent;

pub fn title(alert: &Anomaly, root: &str) -> String {
    let name = crate::leaf(root);
    match alert.kind {
        AnomalyKind::Removal => format!("Many files deleted in {name}"),
        AnomalyKind::Burst => format!("{name} is filling up fast"),
    }
}

pub fn body(alert: &Anomaly) -> String {
    let minutes = WINDOW.as_secs() / 60;
    // Without the sign the shared formatter puts on a delta: the wording
    // already says which direction this went.
    let size = crate::human_bytes(alert.bytes);
    let size = size.trim_start_matches('+');
    match alert.kind {
        AnomalyKind::Removal => format!(
            "{} items ({size}) removed in the last {minutes} minutes.",
            alert.items
        ),
        AnomalyKind::Burst => format!("{size} written in the last {minutes} minutes."),
    }
}

/// Tells the desktop, best effort.
///
/// On its own thread because posting talks to another process — a session
/// worker that waited on a notification daemon would be a session worker not
/// draining its queue, and a lost event costs more than a lost notification.
/// The finding is on screen and in the journal either way.
// ponytail: on Windows a toast wants a registered AppUserModelID, which an
// installer provides; run unpackaged, the desktop may drop it silently.
pub fn post(alert: &Anomaly, root: &str) {
    let summary = title(alert, root);
    let body = body(alert);
    let _ = std::thread::Builder::new()
        .name("pathlight-notify".into())
        .spawn(move || {
            let _ = notify_rust::Notification::new()
                .appname("Pathlight")
                .summary(&summary)
                .body(&body)
                .show();
        });
}

/// Remembers what has already been said, so one finding is one notification.
#[derive(Debug, Default)]
pub struct Alerts {
    /// When each kind was last raised. A folder being emptied stays a folder
    /// being emptied for as long as it takes; saying so every flush would
    /// teach the user to ignore it.
    said: [Option<SystemTime>; 2],
}

impl Alerts {
    /// What is worth saying about these rows right now.
    ///
    /// `rows` is newest first and bounded, so a long enough burst is measured
    /// from what was kept: the thresholds are floors, and a floor that is
    /// crossed is still crossed.
    pub fn news(&mut self, rows: &VecDeque<ActivityEvent>, now: SystemTime) -> Vec<Anomaly> {
        anomalies(rows, now)
            .into_iter()
            .filter(|alert| self.once(alert, now))
            .collect()
    }

    /// Whether this finding has gone unsaid for a window.
    fn once(&mut self, alert: &Anomaly, now: SystemTime) -> bool {
        // Matched rather than indexed by discriminant, so a finding added to
        // the core's policy fails to compile here instead of silently
        // borrowing another kind's cooldown.
        let slot = &mut self.said[match alert.kind {
            AnomalyKind::Removal => 0,
            AnomalyKind::Burst => 1,
        }];
        if slot.is_some_and(|said| now.duration_since(said).unwrap_or_default() < WINDOW) {
            return false;
        }
        *slot = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use pathlight_core::{Confidence, EventKind};

    fn row(kind: EventKind, delta: i64, items: u32, at: SystemTime) -> ActivityEvent {
        ActivityEvent {
            kind,
            path: "/watched/file.bin".to_owned(),
            root_path: "/watched".to_owned(),
            timestamp: at,
            byte_delta: Some(delta),
            confidence: Confidence::Confirmed,
            previous_path: None,
            affected_item_count: items,
            process_name: None,
        }
    }

    /// The part this host owns: a folder being emptied stays a folder being
    /// emptied for as long as it takes, and saying so on every flush would
    /// teach the user to ignore it. What counts as emptied is the core's
    /// decision, checked in `core/tests/anomaly.rs`.
    #[test]
    fn a_finding_is_said_once_per_window() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let rows: VecDeque<_> = (0..2)
            .map(|_| row(EventKind::Deleted, -1, 60, now))
            .collect();
        let mut alerts = Alerts::default();

        let news = alerts.news(&rows, now);
        assert_eq!(news.len(), 1, "120 deleted items went unmentioned");
        assert_eq!(news[0].kind, AnomalyKind::Removal);
        assert!(
            alerts.news(&rows, now + Duration::from_secs(60)).is_empty(),
            "the same finding was raised twice"
        );
        assert_eq!(
            alerts.news(&rows, now + WINDOW).len(),
            1,
            "still happening a window later, and still worth saying"
        );
    }

    /// Nothing to say stays nothing to say, without consuming the window.
    #[test]
    fn a_quiet_folder_is_never_interrupted() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ordinary = VecDeque::from([row(EventKind::Modified, 4096, 1, now)]);
        assert!(Alerts::default().news(&ordinary, now).is_empty());
    }

    /// The wording is this host's, and it reads as a size rather than a delta.
    #[test]
    fn a_flood_of_bytes_is_reported_with_its_size() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let rows = VecDeque::from([row(EventKind::Created, 5_000_000_000, 1, now)]);
        let news = Alerts::default().news(&rows, now);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].kind, AnomalyKind::Burst);
        let body = body(&news[0]);
        assert!(body.contains("4.7 GB"), "unexpected wording: {body}");
        assert!(
            title(&news[0], "/watched").contains("watched"),
            "the folder was not named"
        );
    }
}
