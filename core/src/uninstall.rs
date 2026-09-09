//! Where Pathlight's own storage lives on each OS, and how to remove it.
//!
//! Uninstall is one of the few operations whose correctness is entirely about
//! paths, and the only one where getting a path wrong deletes somebody's data.
//! So every entry here is derived from the platform's own data directories and
//! a fixed application name — never from a watch target, a journal argument or
//! anything else a user typed. A folder the user asked Pathlight to monitor
//! cannot reach this list.
//!
//! The three layouts are plain functions of their inputs rather than
//! `cfg`-gated bodies, so the Windows and Linux rules are exercised by the
//! test suite on every host instead of only where they ship.
//!
//! Scope: the storage this core writes and the standard per-OS locations a
//! host keeps its settings in. Host-specific state a host alone can reach —
//! login items, keychains, notification centres, the application bundle — is
//! that host's job. On macOS `Pathlight/Services/UninstallService.swift` is
//! the authority and owns the longer `~/Library` list; this module deliberately
//! does not restate it, because two lists that must agree eventually do not.

use std::io;
use std::path::{Path, PathBuf};

/// The directory name on platforms that use display-style names.
pub const APP_DIR: &str = "Pathlight";
/// The directory name under XDG roots, which are all-lowercase by convention.
pub const XDG_APP_DIR: &str = "pathlight";

/// Storage locations for the OS this build targets.
///
/// `var` reads an environment variable; taking it as an argument keeps the
/// layout rules testable and lets a host point them at a portable install.
pub fn paths(home: &Path, var: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = var;
        macos_paths(home)
    }
    #[cfg(windows)]
    {
        windows_paths(home, var)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        xdg_paths(home, var)
    }
}

/// Storage locations for the current user, or `None` when the OS will not say
/// where home is — in which case guessing is the one unacceptable answer.
pub fn current_paths() -> Option<Vec<PathBuf>> {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok()?;
    let home = PathBuf::from(home);
    if home.as_os_str().is_empty() {
        return None;
    }
    Some(paths(&home, |name| std::env::var(name).ok()))
}

/// Only the core's own journal directory. The Swift host owns the rest of the
/// macOS list; see the module documentation.
pub fn macos_paths(home: &Path) -> Vec<PathBuf> {
    vec![home
        .join("Library")
        .join("Application Support")
        .join(APP_DIR)]
}

/// `%APPDATA%` holds roaming settings, `%LOCALAPPDATA%` the journal and caches.
/// Both fall back to their standard place under the profile when unset.
pub fn windows_paths(home: &Path, var: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let app_data = present(var("APPDATA")).unwrap_or_else(|| home.join("AppData").join("Roaming"));
    let local_app_data =
        present(var("LOCALAPPDATA")).unwrap_or_else(|| home.join("AppData").join("Local"));
    vec![app_data.join(APP_DIR), local_app_data.join(APP_DIR)]
}

/// The XDG base directories. A variable holding a relative path is ignored per
/// the specification, which is the case a host would otherwise get wrong and
/// then delete relative to whatever the working directory happened to be.
pub fn xdg_paths(home: &Path, var: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let root = |name: &str, default: &[&str]| -> PathBuf {
        absolute(var(name)).unwrap_or_else(|| {
            default
                .iter()
                .fold(home.to_path_buf(), |path, component| path.join(component))
        })
    };
    vec![
        root("XDG_DATA_HOME", &[".local", "share"]).join(XDG_APP_DIR),
        root("XDG_STATE_HOME", &[".local", "state"]).join(XDG_APP_DIR),
        root("XDG_CONFIG_HOME", &[".config"]).join(XDG_APP_DIR),
        root("XDG_CACHE_HOME", &[".cache"]).join(XDG_APP_DIR),
    ]
}

/// Removes each path that exists, reporting the ones that survived.
///
/// Best-effort per entry: one locked file must not hide the rest, and a host
/// that says "everything is gone" has to have been told otherwise.
pub fn remove_all(paths: &[PathBuf]) -> Vec<(PathBuf, io::Error)> {
    let mut failures = Vec::new();
    for path in paths {
        let removed = match path.metadata() {
            Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            // Nothing there is the desired end state, not a failure.
            Err(_) => continue,
        };
        if let Err(error) = removed {
            failures.push((path.clone(), error));
        }
    }
    failures
}

fn present(value: Option<String>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// `Path::is_absolute` answers for the *host*, so it would call a POSIX path
/// relative on Windows and a drive path relative anywhere else. The XDG rule
/// is about POSIX paths, and this spells that rule directly.
fn absolute(value: Option<String>) -> Option<PathBuf> {
    value
        .filter(|value| value.starts_with('/'))
        .map(PathBuf::from)
}
