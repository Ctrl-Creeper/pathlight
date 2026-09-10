//! What is worth interrupting somebody for.
//!
//! Two findings clear that bar: a folder losing a lot at once (a mistake, or
//! ransomware) and a folder filling up fast. The window and the thresholds
//! live here because both hosts notify from them, and a second definition of
//! "anomaly" means one host stays quiet through what the other calls an
//! emergency.
//!
//! Only the policy is shared. What the message says, and whether the desktop
//! is even willing to show it, stays with the host: a notification is the one
//! place where sounding native matters more than sounding identical.

use std::time::{Duration, SystemTime};

use crate::{ActivityEvent, EventKind};

/// The span the thresholds are measured over.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);
/// Enough gone at once that nobody deleted it one file at a time.
const REMOVED_ITEMS: u64 = 100;
/// Or enough bytes gone that the count does not matter.
const REMOVED_BYTES: i64 = 1_000_000_000;
/// Arriving this fast is either a copy the user started or one they did not.
const WRITTEN_BYTES: i64 = 5_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AnomalyKind {
    /// A lot of this folder just stopped existing.
    Removal,
    /// A lot of this folder just arrived.
    Burst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Anomaly {
    pub kind: AnomalyKind,
    pub items: u64,
    /// Always positive: the size of what happened, not its direction.
    pub bytes: i64,
}

/// The findings in `events` as of `now`, newest-first or not — order does not
/// matter, only whether a row falls inside the window.
///
/// Takes references so a host can hand over whatever it already keeps — a
/// ring buffer, a page of history — without copying it first.
///
/// A caller passing a bounded list measures from what it kept: the thresholds
/// are floors, and a floor that is crossed is still crossed. A row stamped in
/// the future is ignored rather than counted, because a clock that jumped is
/// not a folder that changed.
pub fn anomalies<'a>(
    events: impl IntoIterator<Item = &'a ActivityEvent>,
    now: SystemTime,
) -> Vec<Anomaly> {
    let Some(cutoff) = now.checked_sub(WINDOW) else {
        return Vec::new();
    };
    let mut removed_items = 0u64;
    let mut removed_bytes = 0i64;
    let mut written_items = 0u64;
    let mut written_bytes = 0i64;
    for event in events
        .into_iter()
        .filter(|event| event.timestamp >= cutoff && event.timestamp <= now)
    {
        let delta = event.byte_delta.unwrap_or(0);
        if event.kind == EventKind::Deleted || delta < 0 {
            removed_items = removed_items.saturating_add(u64::from(event.affected_item_count));
            removed_bytes = removed_bytes.saturating_add(delta.min(0));
        }
        if delta > 0 {
            written_items = written_items.saturating_add(u64::from(event.affected_item_count));
            written_bytes = written_bytes.saturating_add(delta);
        }
    }

    let mut found = Vec::new();
    let removed_bytes = removed_bytes.saturating_neg();
    if removed_items >= REMOVED_ITEMS || removed_bytes >= REMOVED_BYTES {
        found.push(Anomaly {
            kind: AnomalyKind::Removal,
            items: removed_items,
            bytes: removed_bytes,
        });
    }
    if written_bytes >= WRITTEN_BYTES {
        found.push(Anomaly {
            kind: AnomalyKind::Burst,
            items: written_items,
            bytes: written_bytes,
        });
    }
    found
}

/// [`anomalies`] across the FFI.
#[uniffi::export]
pub fn activity_anomalies(events: Vec<ActivityEvent>, now: SystemTime) -> Vec<Anomaly> {
    anomalies(&events, now)
}

/// The window, for a host whose message has to say how long "recently" is.
#[uniffi::export]
pub fn activity_anomaly_window_secs() -> u64 {
    WINDOW.as_secs()
}
