//! Uninstall is about paths, and a wrong path deletes somebody's data. The
//! per-OS rules are pure functions, so every platform's layout is checked here
//! regardless of which host runs the suite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pathlight_core::uninstall;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: HashMap<String, String> = pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    move |name: &str| vars.get(name).cloned()
}

fn spellings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

#[test]
fn xdg_roots_are_honoured_when_absolute() {
    let paths = uninstall::xdg_paths(
        Path::new("/home/tester"),
        env(&[
            ("XDG_DATA_HOME", "/data/pathlight-home"),
            ("XDG_CACHE_HOME", "/scratch/cache"),
        ]),
    );

    assert_eq!(
        spellings(&paths),
        [
            "/data/pathlight-home/pathlight",
            "/home/tester/.local/state/pathlight",
            "/home/tester/.config/pathlight",
            "/scratch/cache/pathlight",
        ],
        "unset roots fall back to the specification's defaults"
    );
}

/// The XDG specification says a relative value must be ignored. Honouring one
/// would delete relative to whatever the working directory happened to be.
#[test]
fn relative_or_empty_xdg_roots_are_ignored_rather_than_resolved() {
    let paths = uninstall::xdg_paths(
        Path::new("/home/tester"),
        env(&[
            ("XDG_DATA_HOME", "relative/share"),
            ("XDG_CONFIG_HOME", ""),
        ]),
    );

    assert_eq!(
        spellings(&paths),
        [
            "/home/tester/.local/share/pathlight",
            "/home/tester/.local/state/pathlight",
            "/home/tester/.config/pathlight",
            "/home/tester/.cache/pathlight",
        ]
    );
}

#[test]
fn windows_uses_both_roaming_and_local_app_data() {
    let configured = uninstall::windows_paths(
        Path::new("C:/Users/tester"),
        env(&[
            ("APPDATA", "D:/Roaming"),
            ("LOCALAPPDATA", "D:/Local"),
        ]),
    );
    assert_eq!(
        spellings(&configured),
        ["D:/Roaming/Pathlight", "D:/Local/Pathlight"]
    );

    let unset = uninstall::windows_paths(Path::new("C:/Users/tester"), env(&[]));
    assert_eq!(
        spellings(&unset),
        [
            "C:/Users/tester/AppData/Roaming/Pathlight",
            "C:/Users/tester/AppData/Local/Pathlight",
        ],
        "a profile without the variables set still has a standard layout"
    );
}

#[test]
fn macos_leaves_the_host_specific_list_to_the_host() {
    let paths = uninstall::macos_paths(Path::new("/Users/tester"));
    assert_eq!(
        spellings(&paths),
        ["/Users/tester/Library/Application Support/Pathlight"],
        "the Swift host owns preferences, caches and the keychain"
    );
}

/// The one guarantee a user cannot verify before agreeing to an uninstall.
#[test]
fn no_layout_can_name_a_monitored_folder() {
    let home = Path::new("/home/tester");
    let watched = ["/home/tester/Documents", "/home/tester", "/"];
    let every_layout = [
        uninstall::xdg_paths(home, env(&[])),
        uninstall::windows_paths(home, env(&[])),
        uninstall::macos_paths(home),
    ];

    for layout in every_layout {
        for path in layout {
            let spelling = path.to_string_lossy().to_string();
            assert!(
                !watched.contains(&spelling.as_str()),
                "{spelling} is a folder someone might be monitoring"
            );
            assert!(
                spelling.to_lowercase().ends_with("pathlight"),
                "{spelling} does not name Pathlight's own storage"
            );
        }
    }
}

#[test]
fn removal_reports_what_survived_instead_of_claiming_success() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let journal = data.join("activity-events.jsonl");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(&journal, b"row").unwrap();
    let absent = dir.path().join("never-existed");

    // A path that was never created is the desired end state, not a failure.
    let failures = uninstall::remove_all(&[data.clone(), absent]);

    assert!(failures.is_empty(), "{failures:?}");
    assert!(!data.exists());
    assert!(!journal.exists());
}
