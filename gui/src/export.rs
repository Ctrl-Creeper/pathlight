//! What was recorded, as a file something other than Pathlight can read.
//!
//! A monitor whose records only its own window can open is a monitor whose
//! answer cannot be shown to anybody. CSV because every spreadsheet and every
//! scripting language already reads it, and the journal's own JSON lines
//! remain there for anything that would rather parse those.

use pathlight_core::{ActivityEvent, Confidence};

/// One header row, then one row per change in the order given.
pub fn csv(events: &[ActivityEvent]) -> String {
    let mut text = String::from("time,change,path,moved from,bytes,confidence,process\n");
    for event in events {
        let cells = [
            // A row this build cannot date still belongs in the export: the
            // change happened, only its clock reading is unspellable.
            quoted(&event.timestamp_text().unwrap_or_default()),
            quoted(crate::kind_label(event.kind)),
            filename(&event.path),
            filename(event.previous_path.as_deref().unwrap_or_default()),
            quoted(
                &event
                    .byte_delta
                    .map(|delta| delta.to_string())
                    .unwrap_or_default(),
            ),
            quoted(confidence(event.confidence)),
            filename(event.process_name.as_deref().unwrap_or_default()),
        ];
        text.push_str(&cells.join(","));
        text.push('\n');
    }
    text
}

fn confidence(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Confirmed => "confirmed",
        Confidence::Estimated => "estimated",
        Confidence::Unknown => "unknown",
    }
}

/// A cell, quoted when the value would otherwise end it early.
fn quoted(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        return format!("\"{}\"", text.replace('"', "\"\""));
    }
    text.to_owned()
}

/// A cell holding something the user named rather than something Pathlight
/// spelled: a path or a process.
///
/// Excel and LibreOffice run a cell starting `=`, `+`, `-` or `@` as a
/// formula, so a file called `=cmd|'/c calc'!A1` would be code on the machine
/// of whoever opens the export. A leading apostrophe is the spreadsheet's own
/// "this is text" marker, and the only field it appears in is one the file
/// system, not Pathlight, decided the contents of.
fn filename(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        return quoted(&format!("'{text}"));
    }
    quoted(text)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use pathlight_core::EventKind;

    use super::*;

    fn event(path: &str) -> ActivityEvent {
        ActivityEvent {
            kind: EventKind::Modified,
            path: path.to_owned(),
            root_path: "/watched".to_owned(),
            timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            byte_delta: Some(-4096),
            confidence: Confidence::Confirmed,
            previous_path: None,
            affected_item_count: 1,
            process_name: Some("rsync".to_owned()),
        }
    }

    /// The whole point of an absolute time: a row exported today still says
    /// when it happened when it is read next year, which "2h ago" does not.
    #[test]
    fn a_row_carries_the_time_it_happened_not_how_long_ago_it_was() {
        let text = csv(&[event("/watched/report.bin")]);
        let row = text.lines().nth(1).expect("one row");

        assert_eq!(
            row,
            "2023-11-14T22:13:20Z,changed,/watched/report.bin,,-4096,confirmed,rsync"
        );
    }

    /// A comma in a filename is legal, and a comma that ends a cell early
    /// silently shifts every later column of that row.
    #[test]
    fn a_comma_in_a_name_stays_inside_its_own_cell() {
        let text = csv(&[event("/watched/a,b \"quoted\".txt")]);
        let row = text.lines().nth(1).expect("one row");

        assert_eq!(row.matches(',').count(), 7, "{row}");
        assert!(row.contains("\"/watched/a,b \"\"quoted\"\".txt\""), "{row}");
    }

    /// Exporting is handing a file to another program, and a spreadsheet
    /// treats a leading `=` as something to run. The name came from the file
    /// system, so Pathlight is the one that has to make it inert. A path is
    /// exported whole, so it starts with a separator or a drive letter and is
    /// inert already; a process name is just a name.
    #[test]
    fn a_name_a_spreadsheet_would_run_is_exported_as_text() {
        let mut event = event("/watched/report.bin");
        event.process_name = Some("=cmd|'/c calc'!A1".to_owned());
        let text = csv(&[event]);
        let row = text.lines().nth(1).expect("one row");

        assert!(row.contains(",'=cmd"), "{row}");
        // The columns Pathlight spells itself are not disarmed, or every
        // negative byte count would read as text.
        assert!(row.contains(",-4096,"), "{row}");
    }
}
