//! What was recorded, as a file something other than Pathlight can read.
//!
//! A monitor whose records only its own window can open is a monitor whose
//! answer cannot be shown to anybody. CSV because every spreadsheet and every
//! scripting language already reads it, and the journal's own JSON lines
//! remain there for anything that would rather parse those.

use std::collections::HashMap;

use crate::text::{human_bytes, kind_label, leaf};
use crate::{ActivityEvent, Confidence};

/// How many rows a report spells out before it says how many it left out. A
/// report nobody can scroll to the end of is a report nobody reads.
const REPORT_ROWS: usize = 500;

/// One header row, then one row per change in the order given.
pub fn csv(events: &[ActivityEvent]) -> String {
    let mut text = String::from("time,change,path,moved from,bytes,confidence,process\n");
    for event in events {
        let cells = [
            // A row this build cannot date still belongs in the export: the
            // change happened, only its clock reading is unspellable.
            quoted(&event.timestamp_text().unwrap_or_default()),
            quoted(kind_label(event.kind)),
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

/// The same records as a report a person reads rather than a spreadsheet
/// opens: the totals, where inside the folder they landed, what was running,
/// and then the rows.
///
/// Markdown because it is the answer to "what did this installer touch" that
/// gets pasted into a ticket, and it stays readable when nothing renders it.
pub fn markdown(events: &[ActivityEvent], root: &str) -> String {
    let mut text = format!("# Pathlight report: {}\n\n- Folder: `{root}`\n", leaf(root));
    // Oldest and newest rather than first and last in the list: a host hands
    // over what it kept, newest-first in one and oldest-first in another.
    let stamp = |event: Option<&ActivityEvent>| event.and_then(|event| event.timestamp_text());
    if let Some(first) = stamp(events.iter().min_by_key(|event| event.timestamp)) {
        text.push_str(&format!("- First change: {first}\n"));
    }
    if let Some(last) = stamp(events.iter().max_by_key(|event| event.timestamp)) {
        text.push_str(&format!("- Last change: {last}\n"));
    }
    let net: i64 = events
        .iter()
        .filter_map(|event| event.byte_delta)
        .sum::<i64>();
    let unknown = events
        .iter()
        .filter(|event| event.byte_delta.is_none())
        .count();
    text.push_str(&format!(
        "- Changes: {} ({unknown} with unknown size)\n- Net change: {}\n",
        events.len(),
        human_bytes(net)
    ));

    let mut inside: HashMap<String, (i64, u64)> = HashMap::new();
    let mut processes: HashMap<&str, u64> = HashMap::new();
    for event in events {
        let entry = inside.entry(within(root, &event.path)).or_default();
        entry.0 += event.byte_delta.unwrap_or(0);
        entry.1 += u64::from(event.affected_item_count);
        if let Some(process) = event.process_name.as_deref() {
            *processes.entry(process).or_default() += 1;
        }
    }

    // Largest first in both tables, by size of the change rather than its
    // direction: a folder that lost 10 GB is as much the answer as one that
    // gained it.
    let mut inside: Vec<_> = inside.into_iter().collect();
    inside.sort_by_key(|(name, (bytes, _))| (-bytes.abs(), name.clone()));
    text.push_str(
        "\n## Where inside the folder\n\n| Item | Net change | Items |\n|---|---:|---:|\n",
    );
    for (name, (bytes, items)) in inside {
        text.push_str(&format!(
            "| {} | {} | {items} |\n",
            cell(&name),
            human_bytes(bytes)
        ));
    }

    if !processes.is_empty() {
        let mut processes: Vec<_> = processes.into_iter().collect();
        processes.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), *name));
        text.push_str("\n## What was running\n\n");
        for (process, count) in processes {
            text.push_str(&format!("- {}: {count} changes\n", cell(process)));
        }
    }

    text.push_str("\n## Changes\n\n");
    for event in events.iter().take(REPORT_ROWS) {
        text.push_str(&format!(
            "- {} {} `{}`",
            event
                .timestamp_text()
                .unwrap_or_else(|| "unknown time".into()),
            kind_label(event.kind),
            event.path
        ));
        if let Some(previous) = &event.previous_path {
            text.push_str(&format!(" (from `{previous}`)"));
        }
        text.push_str(&match event.byte_delta {
            Some(delta) => format!(" {}", human_bytes(delta)),
            None => " unknown size".to_owned(),
        });
        if let Some(process) = &event.process_name {
            text.push_str(&format!(" · {}", cell(process)));
        }
        text.push('\n');
    }
    if let Some(omitted) = events
        .len()
        .checked_sub(REPORT_ROWS)
        .filter(|left| *left > 0)
    {
        text.push_str(&format!("- … {omitted} more changes not listed\n"));
    }
    text
}

/// The top-level thing inside `root` that `path` is part of, which is what a
/// person looking for "where did the 10 GB go" is asking about.
fn within(root: &str, path: &str) -> String {
    let inside = path
        .strip_prefix(root)
        .map(|rest| rest.trim_start_matches(['/', '\\']))
        .unwrap_or(path);
    match inside.split(['/', '\\']).next().unwrap_or_default() {
        "" => "(the folder itself)".to_owned(),
        first => first.to_owned(),
    }
}

/// A name the file system chose, inside a table cell Pathlight spelled: a `|`
/// in a filename would otherwise become a column of its own.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
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

    use crate::EventKind;

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

    /// What a report is for: the totals, and where inside the folder the
    /// bytes went — the answer a row list makes the reader add up themselves.
    #[test]
    fn a_report_says_where_inside_the_folder_the_bytes_went() {
        let mut big = event("/watched/Caches/blob.bin");
        big.byte_delta = Some(2_000_000_000);
        big.affected_item_count = 3;
        let mut moved = event("/watched/notes.txt");
        moved.previous_path = Some("/watched/old.txt".to_owned());
        moved.byte_delta = None;
        let text = markdown(&[event("/watched/report.bin"), big, moved], "/watched");

        assert!(text.starts_with("# Pathlight report: watched"), "{text}");
        assert!(
            text.contains("- Changes: 3 (1 with unknown size)"),
            "{text}"
        );
        assert!(text.contains("- Net change: +1.9 GB"), "{text}");
        // The largest change first, whichever direction it went.
        let table = text.split("## Where inside the folder").nth(1).unwrap();
        let rows: Vec<&str> = table.lines().filter(|line| line.starts_with('|')).collect();
        assert!(rows[2].starts_with("| Caches |"), "{table}");
        assert!(rows[2].contains("| 3 |"), "{table}");
        assert!(text.contains("- rsync: 3 changes"), "{text}");
        assert!(text.contains("(from `/watched/old.txt`)"), "{text}");
        assert!(text.contains("unknown size"), "{text}");
    }

    /// A pipe in a filename would otherwise open a column of its own and
    /// shift every later cell of that row.
    #[test]
    fn a_pipe_in_a_name_stays_inside_its_own_cell() {
        let text = markdown(&[event("/watched/a|b.txt")], "/watched");
        assert!(text.contains(r"| a\|b.txt |"), "{text}");
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
