//! What is worth interrupting somebody for, said the way a desktop says it.
//!
//! A watch keeps running after its window closes, so a notification is the
//! only way a background watch can tell anybody anything — which sets the bar
//! high. Which findings clear it, the window and thresholds that decide, and
//! the wording all belong to the core, so the command line says the same thing
//! about the same folder. What is left here is the one part a desktop owns:
//! handing the toast to whatever is listening for one.

use pathlight_core::anomaly::Anomaly;
use pathlight_core::text::{alert_body, alert_title};

/// Tells the desktop, best effort.
///
/// On its own thread because posting talks to another process — a session
/// worker that waited on a notification daemon would be a session worker not
/// draining its queue, and a lost event costs more than a lost notification.
/// The finding is on screen and in the journal either way.
// ponytail: on Windows a toast wants a registered AppUserModelID, which an
// installer provides; run unpackaged, the desktop may drop it silently.
pub fn post(alert: &Anomaly, root: &str) {
    let summary = alert_title(alert, root);
    let body = alert_body(alert);
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
