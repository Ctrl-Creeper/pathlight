//! The folders worth watching without being asked where they are.
//!
//! Every desktop keeps its churn in places a person would not think to type:
//! `AppData\Local`, `~/.cache`, `~/Library`. Naming them is the difference
//! between a monitor somebody points at Downloads and a monitor that shows
//! them where their disk actually went. The macOS app ships the same idea in
//! `MonitoringPresets`.

use std::env;
use std::path::{Path, PathBuf};

/// One offered folder. Only ever a path and a name: what a watch records is
/// this install's settings, the same for a preset as for a folder picked by
/// hand.
// ponytail: no per-preset exclusion patterns, which macOS does have. The
// patterns here are one list for the install; a preset that quietly added to
// it would change what every other watch records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub title: &'static str,
    pub path: String,
}

/// The presets whose folder exists on this machine, in the order a person
/// would look for them. A preset pointing at a folder that is not there is an
/// offer to watch nothing.
pub fn available() -> Vec<Preset> {
    all().into_iter().filter(exists).collect()
}

fn exists(preset: &Preset) -> bool {
    Path::new(&preset.path).is_dir()
}

/// Every preset this OS has, existing or not.
pub fn all() -> Vec<Preset> {
    let home = home();
    let mut presets = vec![
        Preset {
            title: "Whole disk",
            path: crate::paths::normalize(&whole_disk().to_string_lossy()),
        },
        under(&home, "Downloads", "Downloads"),
        under(&home, "Desktop", "Desktop"),
        under(&home, "Documents", "Documents"),
    ];
    if cfg!(windows) {
        presets.push(under(&home, "App data (local)", "AppData/Local"));
        presets.push(under(&home, "App data (roaming)", "AppData/Roaming"));
    } else if cfg!(target_os = "macos") {
        presets.push(under(&home, "Library", "Library"));
        presets.push(Preset {
            title: "Applications",
            path: "/Applications".to_owned(),
        });
    } else {
        presets.push(under(&home, "Cache", ".cache"));
        presets.push(under(&home, "Application data", ".local/share"));
        presets.push(Preset {
            title: "Logs",
            path: "/var/log".to_owned(),
        });
    }
    presets
}

fn under(home: &Path, title: &'static str, relative: &str) -> Preset {
    Preset {
        title,
        path: crate::paths::normalize(&home.join(relative).to_string_lossy()),
    }
}

/// Where a whole-disk watch starts. On Windows that is the drive the system
/// booted from rather than a hard `C:`, because it is not always `C:`.
fn whole_disk() -> PathBuf {
    if cfg!(windows) {
        let drive = env::var_os("SystemDrive")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "C:".into());
        let mut root = PathBuf::from(drive);
        root.push(std::path::MAIN_SEPARATOR_STR);
        return root;
    }
    PathBuf::from("/")
}

fn home() -> PathBuf {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: the offered paths are real ones on this machine, and
    /// the home-relative ones are under this user's home rather than under
    /// somebody's idea of where homes go.
    #[test]
    fn every_preset_names_a_folder_this_os_actually_uses() {
        let presets = all();
        let home = crate::paths::normalize(&home().to_string_lossy());

        assert!(presets.iter().any(|preset| preset.title == "Whole disk"));
        let downloads = presets
            .iter()
            .find(|preset| preset.title == "Downloads")
            .expect("Downloads is offered everywhere");
        assert_eq!(downloads.path, format!("{home}/Downloads"));
        // Normalized, because these paths go straight into a watch and the
        // storage guard compares spellings.
        assert!(
            presets.iter().all(|preset| !preset.path.contains('\\')),
            "{presets:?}"
        );
        // The home directory of whoever runs the tests exists, so at least the
        // whole disk and one folder under it survive the filter.
        assert!(available().len() >= 2, "{:?}", available());
    }
}
