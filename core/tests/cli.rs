//! The terminal's half of the app, driven the way a person drives it.
//!
//! One settings file and one journal are shared with the windows, so what
//! matters is that a change made here is a change the next watch reads — and
//! that a command nobody typed correctly says so instead of guessing.

use std::path::Path;
use std::process::{Command, Output};

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
    assert!(shown.contains("retention-days     180"), "{shown}");

    run(home.path(), &["settings", "retention-days", "30"]);
    run(home.path(), &["settings", "latency", "power-saving"]);
    run(home.path(), &["settings", "min-file-bytes", "1024"]);
    run(home.path(), &["settings", "encrypt", "on"]);
    run(home.path(), &["settings", "patterns", "*.tmp", "logs/"]);

    let shown = text(&run(home.path(), &["settings"]));
    for expected in [
        "retention-days     30",
        "latency            30000 ms",
        "min-file-bytes     1024",
        "encrypt            true",
        "patterns           *.tmp logs/",
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
    assert!(text(&run(home.path(), &["settings"])).contains("retention-days     30"));
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
