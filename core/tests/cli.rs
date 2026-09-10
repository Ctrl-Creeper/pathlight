//! The terminal's half of the app, driven the way a person drives it.
//!
//! One settings file and one journal are shared with the windows, so what
//! matters is that a change made here is a change the next watch reads — and
//! that a command nobody typed correctly says so instead of guessing.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, UNIX_EPOCH};

use pathlight_core::{ActivityEvent, Confidence, EventKind, Journal};

/// The command, run against a throwaway home so nothing touches the tester's
/// own records. `HOME` (and `USERPROFILE`) is how the store finds its
/// directory, which is the only handle a test has on it.
fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pathlight-monitor"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        // Otherwise a Linux runner's XDG variables point the store back at the
        // tester's real home and the test writes where it was told not to.
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("the binary under test is built by cargo test")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_folder_added_in_the_terminal_is_switched_on_only_when_asked() {
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().to_string_lossy().replace('\\', "/");

    assert!(text(&run(home.path(), &["watches"])).contains("No folders yet"));

    let added = run(home.path(), &["watches", "add", &path]);
    assert!(added.status.success(), "{}", text(&added));
    // Off, like every host adds it: naming a folder must not start recording
    // it.
    let listed = text(&run(home.path(), &["watches"]));
    assert!(listed.contains("off"), "{listed}");

    run(home.path(), &["watches", "enable", &path]);
    assert!(text(&run(home.path(), &["watches"])).contains("on "));
    run(home.path(), &["watches", "disable", &path]);
    assert!(text(&run(home.path(), &["watches"])).contains("off"));

    let removed = run(home.path(), &["watches", "remove", &path]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert!(text(&run(home.path(), &["watches"])).contains("No folders yet"));

    // A folder that was never added cannot be switched off, and the message
    // says which folder rather than failing silently.
    let missing = run(home.path(), &["watches", "remove", &path]);
    assert!(!missing.status.success());
    assert!(text(&missing).contains(&path), "{}", text(&missing));
}

/// The point of the settings commands: what is set in a terminal is what the
/// windows and the next watch read, in the same file.
#[test]
fn a_setting_changed_in_the_terminal_is_what_the_next_watch_reads() {
    let home = tempfile::tempdir().unwrap();

    let shown = text(&run(home.path(), &["settings"]));
    assert!(shown.contains("retention-days            180"), "{shown}");
    assert!(shown.contains("growth-alert-mb           never"), "{shown}");

    run(home.path(), &["settings", "retention-days", "30"]);
    run(home.path(), &["settings", "latency", "power-saving"]);
    run(home.path(), &["settings", "min-file-bytes", "1024"]);
    run(home.path(), &["settings", "encrypt", "on"]);
    run(home.path(), &["settings", "patterns", "*.tmp", "logs/"]);
    run(home.path(), &["settings", "records", "grouped", "120"]);
    run(
        home.path(),
        &["settings", "aggregate-retention-days", "365"],
    );
    run(home.path(), &["settings", "growth-alert-mb", "5000"]);

    let shown = text(&run(home.path(), &["settings"]));
    for expected in [
        "retention-days            30",
        "latency                   30000 ms",
        "min-file-bytes            1024",
        "encrypt                   true",
        "patterns                  *.tmp logs/",
        "records                   grouped 120",
        "aggregate-retention-days  365",
        "growth-alert-mb           5000",
    ] {
        assert!(shown.contains(expected), "{expected} missing from {shown}");
    }

    // The shipped patterns come back, which is the only way out of an edit
    // that excluded something the user wanted.
    let restored = text(&run(home.path(), &["settings", "patterns", "default"]));
    assert!(restored.contains(".DS_Store"), "{restored}");

    // A value that is not a number changes nothing rather than being read as
    // zero, which would delete every row as it was written.
    let refused = run(home.path(), &["settings", "retention-days", "a while"]);
    assert!(!refused.status.success());
    assert!(text(&run(home.path(), &["settings"])).contains("retention-days            30"));
}

/// The three answers about somebody's own records, from a terminal: how much
/// is there, put the settings back, and start the records over. The last one
/// asks twice, because nothing recorded can be got back. What the counting
/// itself says about a journal with rows in it is `core/src/store.rs`.
#[test]
fn records_are_counted_and_only_deleted_when_asked_twice() {
    let home = tempfile::tempdir().unwrap();

    let shown = text(&run(home.path(), &["settings"]));
    assert!(shown.contains("nothing recorded yet"), "{shown}");
    // What this platform's watcher promises is not a setting, and is still
    // the thing that says how much the rows can be trusted.
    assert!(shown.contains("watcher  "), "{shown}");

    run(home.path(), &["settings", "retention-days", "30"]);
    let restored = text(&run(home.path(), &["settings", "defaults"]));
    assert!(
        restored.contains("retention-days            180"),
        "{restored}"
    );

    // Asking without --yes says what would go and deletes nothing.
    let asked = text(&run(home.path(), &["forget-records"]));
    assert!(asked.contains("--yes"), "{asked}");
    let compacted = text(&run(home.path(), &["compact"]));
    assert!(compacted.contains("row(s) left"), "{compacted}");

    let deleted = text(&run(home.path(), &["forget-records", "--yes"]));
    assert!(
        deleted.contains("Settings and folders are unchanged"),
        "{deleted}"
    );
}

/// The offer has to be a real folder on the machine running it, or it is an
/// offer to watch nothing.
#[test]
fn the_suggested_folders_are_ones_this_machine_has() {
    let home = tempfile::tempdir().unwrap();
    let listed = text(&run(home.path(), &["presets"]));
    let paths: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|word| word.starts_with('/') || word.contains(":/"))
        .collect();
    assert!(!paths.is_empty(), "{listed}");
    for path in paths {
        assert!(Path::new(path).is_dir(), "{path} is not a folder");
    }
}

#[test]
fn nothing_recorded_yet_is_said_rather_than_exported_as_an_empty_file() {
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().to_string_lossy().replace('\\', "/");

    let exported = run(home.path(), &["export", &path]);
    assert!(!exported.status.success());
    assert!(text(&exported).contains("nothing has been recorded"));

    // And a folder with no rows still has a history, because "no changes" is
    // an answer.
    let history = run(home.path(), &["history", &path]);
    assert!(history.status.success(), "{}", text(&history));
    assert!(text(&history).contains("0 change(s)"));
}

/// No arguments used to be a usage error; a monitor people run to find out
/// what it does should say what it does.
#[test]
fn the_bare_command_says_what_the_commands_are() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), &[]);
    assert!(output.status.success(), "{}", text(&output));
    let help = text(&output);
    for command in [
        "watch ",
        "watches ",
        "history ",
        "export ",
        "settings ",
        "autostart ",
    ] {
        assert!(help.contains(command), "{command} missing from {help}");
    }
}

/// Where this install keeps its records, as the command itself reports it: the
/// test writes rows the way the windows do, into the file both hosts share.
fn records_dir(home: &Path) -> String {
    let shown = text(&run(home, &["settings"]));
    shown
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("folder"))
        .map(|path| path.trim().to_owned())
        .expect("settings names the folder it keeps records in")
}

fn recorded(root: &Path, name: &str, seconds: u64, delta: i64, kind: EventKind) -> ActivityEvent {
    ActivityEvent {
        kind,
        path: root.join(name).to_string_lossy().into_owned(),
        root_path: root.to_string_lossy().into_owned(),
        timestamp: UNIX_EPOCH + Duration::from_secs(seconds),
        byte_delta: Some(delta),
        confidence: Confidence::Confirmed,
        previous_path: None,
        affected_item_count: 1,
        process_name: None,
    }
}

/// Searching a history from a terminal. The rule itself is pinned in
/// `tests/history.rs`; what matters here is that every flag reaches it and that
/// the counts printed describe what matched rather than the whole record.
#[test]
fn a_history_can_be_narrowed_to_what_somebody_is_looking_for() {
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let path = root.to_string_lossy().replace('\\', "/");

    let journal = Journal::new(format!(
        "{}/activity-events.jsonl",
        records_dir(home.path())
    ));
    journal
        .append(vec![
            recorded(root, "big.psd", 10, 8_192, EventKind::Created),
            recorded(root, "small.psd", 20, 16, EventKind::Modified),
            recorded(root, "notes.txt", 30, -4_096, EventKind::Deleted),
        ])
        .unwrap();

    let all = text(&run(home.path(), &["history", &path]));
    assert!(all.contains("3 change(s), net"), "{all}");

    // Part of a path, without regard to case.
    let found = text(&run(home.path(), &["history", &path, "--find", "PSD"]));
    assert!(found.contains(r#"2 change(s) matching "PSD""#), "{found}");
    assert!(!found.contains("notes.txt"), "{found}");

    // One kind, by the word the rows are printed with.
    let deleted = text(&run(home.path(), &["history", &path, "--kind", "deleted"]));
    assert!(deleted.contains("1 change(s) deleted"), "{deleted}");
    assert!(deleted.contains("notes.txt"), "{deleted}");

    // Biggest first, one row at a time: the page moves, the totals do not.
    let first = text(&run(
        home.path(),
        &["history", &path, "--largest", "--limit", "1"],
    ));
    assert!(first.contains("big.psd"), "{first}");
    assert!(first.contains("Rows 1–1 of 3, biggest first"), "{first}");
    let second = text(&run(
        home.path(),
        &["history", &path, "--largest", "--limit", "1", "--skip", "1"],
    ));
    assert!(second.contains("notes.txt"), "{second}");
    assert!(second.contains("Rows 2–2 of 3"), "{second}");
    let past_the_end = text(&run(home.path(), &["history", &path, "--skip", "9"]));
    assert!(
        past_the_end.contains("Nothing left past --skip 9"),
        "{past_the_end}"
    );

    // A kind nobody records is a mistyped flag, not an empty history.
    let mistyped = run(home.path(), &["history", &path, "--kind", "shredded"]);
    assert!(!mistyped.status.success());
    assert!(text(&mistyped).contains("deleted"), "{}", text(&mistyped));
}
