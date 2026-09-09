//! The recorder's command line, driven as a user drives it.
//!
//! Argument parsing is the one part of this binary a test can reach without a
//! filesystem to watch, and it guards a destructive subcommand.

use std::path::Path;
use std::process::{Command, Output};

/// Where `install-cli` is expected to land under a temporary home.
fn installed_path(home: &Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("AppData")
            .join("Local")
            .join("Programs")
            .join("Pathlight")
            .join("pathlight-monitor.exe")
    } else {
        home.join(".local").join("bin").join("pathlight-monitor")
    }
}

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

/// The whole point of the subcommand: after this, a shell can reach the
/// command by name. Nothing outside the temporary home is written.
#[test]
fn install_cli_puts_the_command_in_the_users_own_home() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&["install-cli"], dir.path());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed = installed_path(dir.path());
    assert!(installed.exists(), "{text}");
    assert!(
        installed.starts_with(dir.path()),
        "{} escaped the home it was given",
        installed.display()
    );
    // A directory nothing has put on PATH has to be reported as such, or the
    // command looks installed and is unreachable.
    assert!(text.contains("not in PATH"), "{text}");

    // Re-running after an update is the normal case, not an error.
    let again = run(&["install-cli"], dir.path());
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(installed.exists());
}

/// A binary left on someone's PATH after an uninstall is litter they never
/// find, so the command that installed it is the one that accounts for it.
#[test]
fn uninstall_removes_an_installed_command_too() {
    let dir = tempfile::tempdir().unwrap();
    assert!(run(&["install-cli"], dir.path()).status.success());
    let installed = installed_path(dir.path());

    let listed = run(&["uninstall"], dir.path());
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("pathlight-monitor"), "{text}");
    assert!(installed.exists(), "a dry run removed the command");

    assert!(run(&["uninstall", "--yes"], dir.path()).status.success());
    assert!(!installed.exists());
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
