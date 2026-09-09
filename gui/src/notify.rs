//! What is worth interrupting somebody for.
//!
//! A watch keeps running after its window closes, so a notification is the
//! only way a background watch can tell anybody anything — which sets the bar
//! high. Two findings clear it: a folder losing a lot at once (a mistake, or
//! ransomware) and a folder filling up fast. The window and the thresholds are
//! the macOS app's `ActivityAnomalyDetector`, so the two hosts never disagree
//! about what counts as an anomaly.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use pathlight_core::{ActivityEvent, EventKind};

/// The span the thresholds are measured over.
const WINDOW: Duration = Duration::from_secs(10 * 60);
const REMOVED_ITEMS: u64 = 100;
const REMOVED_BYTES: i64 = 1_000_000_000;
const WRITTEN_BYTES: i64 = 5_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A lot of this folder just stopped existing.
    Removal,
    /// A lot of this folder just arrived.
    Burst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alert {
    pub kind: Kind,
    pub items: u64,
    /// Always positive: the size of what happened, not its direction.
    pub bytes: i64,
}

impl Alert {
    pub fn title(&self, root: &str) -> String {
        let name = crate::leaf(root);
        match self.kind {
            Kind::Removal => format!("Many files deleted in {name}"),
            Kind::Burst => format!("{name} is filling up fast"),
        }
    }

    pub fn body(&self) -> String {
        let minutes = WINDOW.as_secs() / 60;
        // Without the sign the shared formatter puts on a delta: the wording
        // already says which direction this went.
        let size = crate::human_bytes(self.bytes);
        let size = size.trim_start_matches('+');
        match self.kind {
            Kind::Removal => format!(
                "{} items ({size}) removed in the last {minutes} minutes.",
                self.items
            ),
            Kind::Burst => format!("{size} written in the last {minutes} minutes."),
        }
    }

    /// Tells the desktop, best effort.
    ///
    /// On its own thread because posting talks to another process — a session
    /// worker that waited on a notification daemon would be a session worker
    /// not draining its queue, and a lost event costs more than a lost
    /// notification. The finding is on screen and in the journal either way.
    // ponytail: on Windows a toast wants a registered AppUserModelID, which an
    // installer provides; run unpackaged, the desktop may drop it silently.
    pub fn post(&self, root: &str) {
        let summary = self.title(root);
        let body = self.body();
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
    pub fn news(&mut self, rows: &VecDeque<ActivityEvent>, now: SystemTime) -> Vec<Alert> {
        let Some(cutoff) = now.checked_sub(WINDOW) else {
            return Vec::new();
        };
        let recent = rows.iter().filter(|row| row.timestamp >= cutoff);

        let mut removed_items = 0u64;
        let mut removed_bytes = 0i64;
        let mut written_bytes = 0i64;
        for row in recent {
            let delta = row.byte_delta.unwrap_or(0);
            if row.kind == EventKind::Deleted || delta < 0 {
                removed_items = removed_items.saturating_add(u64::from(row.affected_item_count));
                removed_bytes = removed_bytes.saturating_add(delta.min(0));
            }
            if delta > 0 {
                written_bytes = written_bytes.saturating_add(delta);
            }
        }

        let mut news = Vec::new();
        if removed_items >= REMOVED_ITEMS || -removed_bytes >= REMOVED_BYTES {
            news.extend(self.once(
                Kind::Removal,
                Alert {
                    kind: Kind::Removal,
                    items: removed_items,
                    bytes: -removed_bytes,
                },
                now,
            ));
        }
        if written_bytes >= WRITTEN_BYTES {
            news.extend(self.once(
                Kind::Burst,
                Alert {
                    kind: Kind::Burst,
                    items: 0,
                    bytes: written_bytes,
                },
                now,
            ));
        }
        news
    }

    fn once(&mut self, kind: Kind, alert: Alert, now: SystemTime) -> Option<Alert> {
        let slot = &mut self.said[kind as usize];
        if slot.is_some_and(|said| now.duration_since(said).unwrap_or_default() < WINDOW) {
            return None;
        }
        *slot = Some(now);
        Some(alert)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: EventKind, delta: i64, items: u32, at: SystemTime) -> ActivityEvent {
        ActivityEvent {
            kind,
            path: "/watched/file.bin".to_owned(),
            root_path: "/watched".to_owned(),
            timestamp: at,
            byte_delta: Some(delta),
            confidence: pathlight_core::Confidence::Confirmed,
            previous_path: None,
            affected_item_count: items,
            process_name: None,
        }
    }

    /// The whole point of the feature: a folder being emptied says so without
    /// being asked, and says it once rather than on every flush.
    #[test]
    fn a_folder_losing_a_lot_at_once_is_worth_saying_once() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let rows: VecDeque<_> = (0..2)
            .map(|_| row(EventKind::Deleted, -1, 60, now))
            .collect();
        let mut alerts = Alerts::default();

        let news = alerts.news(&rows, now);
        assert_eq!(
            news.len(),
            1,
            "120 deleted items went unmentioned: {news:?}"
        );
        assert_eq!(news[0].kind, Kind::Removal);
        assert_eq!(news[0].items, 120);
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

    /// Ordinary use is not an anomaly, and old rows are not news: a threshold
    /// measured over the whole journal would fire on every folder eventually.
    #[test]
    fn quiet_and_long_ago_are_both_quiet() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut alerts = Alerts::default();
        let ordinary = VecDeque::from([row(EventKind::Modified, 4096, 1, now)]);
        assert!(alerts.news(&ordinary, now).is_empty());

        let long_ago = VecDeque::from([row(
            EventKind::Created,
            WRITTEN_BYTES * 2,
            1,
            now - WINDOW - Duration::from_secs(1),
        )]);
        assert!(
            alerts.news(&long_ago, now).is_empty(),
            "a burst from yesterday was reported as happening now"
        );
    }

    /// A burst is measured in bytes, and the size in the message is the size
    /// of what happened rather than a signed delta.
    #[test]
    fn a_flood_of_bytes_is_reported_with_its_size() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let rows = VecDeque::from([row(EventKind::Created, WRITTEN_BYTES, 1, now)]);
        let news = Alerts::default().news(&rows, now);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].kind, Kind::Burst);
        assert!(news[0].bytes > 0);
        assert!(
            news[0].body().contains("4.7 GB"),
            "unexpected wording: {}",
            news[0].body()
        );
    }
}
