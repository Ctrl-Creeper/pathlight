//! The recorder's command line, driven as a user drives it.
//!
//! Argument parsing is the one part of this binary a test can reach without a
//! filesystem to watch, and it guards a destructive subcommand.

use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pathlight-monitor"))
        .args(args)
        // Uninstall derives its paths from the home directory and the data
        // roots, so a test that did not redirect every one of them could delete
        // the developer's own storage. Cleared rather than set, so the layout
        // falls back to its standard place under this temporary home.
        .env(if cfg!(windows) { "USERPROFILE" } else { "HOME" }, home)
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("APPDATA")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("the binary this test was built alongside")
}

/// Running a command with no arguments is how everybody meets it, and it used
/// to panic with an index out of bounds.
#[test]
fn no_arguments_reports_the_usage_instead_of_crashing() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[], dir.path());

    let message = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{message}");
    assert!(message.contains("usage:"), "{message}");
    assert!(!message.contains("panicked"), "{message}");
}

#[test]
fn help_explains_both_subcommands_and_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&["--help"], dir.path());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(text.contains("ROOT JOURNAL"), "{text}");
    assert!(text.contains("uninstall"), "{text}");
}

/// The one destructive command. Listing has to remove nothing, and removal has
/// to reach exactly the storage the layout names — here inside a temporary home.
#[test]
fn uninstall_lists_before_it_removes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = pathlight_core::uninstall::paths(dir.path(), |_| None)
        .into_iter()
        .next()
        .unwrap();
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(storage.join("activity-events.jsonl"), b"row").unwrap();

    let listed = run(&["uninstall"], dir.path());
    assert!(listed.status.success());
    assert!(storage.exists(), "a dry run removed storage");

    let removed = run(&["uninstall", "--yes"], dir.path());
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!storage.exists());
}

/// A typo in the flag must not be read as consent.
#[test]
fn an_unknown_uninstall_flag_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&["uninstall", "--force"], dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage:"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
