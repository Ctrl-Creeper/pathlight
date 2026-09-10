use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pathlight_core::store::{Storage, WatchTarget, BACKGROUND_LATENCY_MS, DEFAULT_LATENCY_MS};
use pathlight_core::text::{alert_body, alert_title, human_bytes, kind_label};
use pathlight_core::{paths, ActivityEvent};

fn main() {
    if let Err(error) = run() {
        eprintln!("pathlight-monitor: {error}");
        std::process::exit(1);
    }
}

/// Where this command goes so a shell can find it.
///
/// Always inside the user's own home. `/usr/local/bin` is on the default PATH
/// and would be the obvious place, but it needs an administrator on a clean
/// macOS — and a monitor that asks for a password to install a convenience is
/// a monitor people stop trusting.
fn cli_path() -> io::Result<PathBuf> {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no home directory in the environment, and guessing one is not an option",
            )
        })?;
    if cfg!(windows) {
        let local = env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        Ok(local
            .join("Programs")
            .join("Pathlight")
            .join("pathlight-monitor.exe"))
    } else {
        Ok(home.join(".local").join("bin").join("pathlight-monitor"))
    }
}

/// Whether a shell in this environment would find a command in `dir`.
fn on_path(dir: &Path) -> bool {
    env::var_os("PATH").is_some_and(|path| env::split_paths(&path).any(|entry| entry == dir))
}

/// Puts this binary where a terminal can reach it by name.
///
/// The GUI installs are a dragged `.app` and an unpacked archive, so nothing
/// touches PATH on any platform. This is the one step that does, per user, with
/// no installer to write and no privileges to ask for.
fn install_cli(rest: &[OsString]) -> io::Result<()> {
    if !rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pathlight-monitor install-cli",
        ));
    }
    let source = env::current_exe()?;
    let target = cli_path()?;
    let dir = target
        .parent()
        .expect("a file name has a parent")
        .to_owned();
    if source == target {
        println!("Already installed at {}.", target.display());
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    // An older install is replaced rather than reported as an error: this
    // command is the one people re-run after an update.
    if std::fs::symlink_metadata(&target).is_ok() {
        std::fs::remove_file(&target)?;
    }
    // ponytail: a symlink on Unix, so an app update is picked up without
    // re-running this; Windows copies, because a symlink there needs either
    // developer mode or an administrator.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &target)?;
    #[cfg(not(unix))]
    std::fs::copy(&source, &target)?;

    println!("Installed {}.", target.display());
    if !on_path(&dir) {
        println!("{} is not in PATH yet. Add it:", dir.display());
        if cfg!(windows) {
            println!(
                "  [Environment]::SetEnvironmentVariable(\"Path\", [Environment]::GetEnvironmentVariable(\"Path\", \"User\") + \";{}\", \"User\")",
                dir.display()
            );
        } else {
            println!(
                "  export PATH=\"{}:$PATH\"   # in your shell profile",
                dir.display()
            );
        }
    }
    Ok(())
}

/// Dry run by default: a destructive command that needs no confirmation is a
/// command someone runs by accident. Journals passed to a recording are not
/// touched, because this must never delete a path the user typed.
fn uninstall(rest: &[std::ffi::OsString]) -> io::Result<()> {
    let confirmed = match rest {
        [] => false,
        [flag] if flag == "--yes" => true,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: pathlight-monitor uninstall [--yes]",
            ))
        }
    };
    let paths = pathlight_core::uninstall::current_paths().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory in the environment, and guessing one is not an option",
        )
    })?;
    // The installed command is not storage, so it is not in the layout — but
    // leaving a binary on someone's PATH after an uninstall is exactly the
    // litter people never find. This command knows where it put it.
    let installed = cli_path().ok().filter(|path| path.exists());
    let present: Vec<_> = paths
        .into_iter()
        .filter(|path| path.exists())
        .chain(installed)
        .collect();
    if present.is_empty() {
        println!("Nothing of Pathlight's own storage is on this system.");
        return Ok(());
    }
    for path in &present {
        println!("{}", path.display());
    }
    if !confirmed {
        println!("Nothing was removed. Re-run with --yes to remove the paths above.");
        return Ok(());
    }
    let failures = pathlight_core::uninstall::remove_all(&present);
    if failures.is_empty() {
        println!("Removed {} path(s).", present.len());
        return Ok(());
    }
    for (path, error) in &failures {
        eprintln!("could not remove {}: {error}", path.display());
    }
    Err(io::Error::other(format!(
        "{} of {} path(s) could not be removed",
        failures.len(),
        present.len()
    )))
}

/// Takes `--name VALUE` out of `args` and parses it as a byte count.
///
/// Removed rather than ignored, so the positional arguments keep their meaning
/// wherever the flag was typed. An unparsable value is an error and not a
/// silently absent bound: a recording that quietly kept everything the user
/// asked it to skip is a recording they would have to redo.
fn take_bytes(args: &mut Vec<OsString>, name: &str) -> io::Result<Option<i64>> {
    let Some(at) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    let value = args
        .get(at + 1)
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} needs a byte count that is zero or more"),
            )
        })?;
    args.drain(at..=at + 1);
    Ok(Some(value))
}

const SETTING_VALUES: &str = "
Change one with `settings KEY VALUE`: latency takes immediate, power-saving or a number
of milliseconds; the file bounds take a byte count or `any`; encrypt takes on or off;
records takes file-names or `grouped [SECONDS]`; growth-alert-mb takes megabytes a folder
may gain in a day before you are told, or 0 for never; patterns takes a list, `default` for
the shipped one, or `none` to record everything.";

const HELP: &str = "\
Usage: pathlight-monitor <command> [arguments]

  watch [FOLDER…]         Watch folders and print what changes, recording it.
                          With no folder, watches the ones that are switched on.
  presets                 The folders worth watching on this machine, named.
  watches                 List the folders this install remembers.
  watches add FOLDER      Remember a folder, switched off.
  watches enable FOLDER   Switch a folder on, so `watch` picks it up.
  watches disable FOLDER  Switch it off again.
  watches remove FOLDER   Forget it. Nothing already recorded is deleted.
  history FOLDER          What the journal holds for a folder.
  export FOLDER [FILE]    Write that as CSV, to FILE or to standard output.
  report FOLDER [FILE]    Write it as a report to read: totals, where inside
                          the folder the bytes went, and what was running.
  settings                Show every setting, and where records are kept.
  settings KEY VALUE…     Change one. Run `settings` to see the keys.
  autostart [on|off]      Whether the watches start when you sign in.
  record FOLDER JOURNAL [SECONDS=10] [--min-bytes N] [--max-bytes N]
                          One explicit recording into a journal you name, with
                          two interval snapshots. JOURNAL must be outside
                          FOLDER, and registration and scans add to the
                          duration.
  install-cli             Put this command in your own home, on your PATH.
  uninstall [--yes]       List Pathlight's own storage, and with --yes remove it.

Every command but `record` shares one settings file and one journal with the
Pathlight windows, so a folder added here shows up there. No watch ever records
Pathlight's own storage.
";

/// The shared settings file and journal, or why there is none.
fn storage() -> io::Result<Storage> {
    Storage::current().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory in the environment, so there is nowhere to record; set HOME \
             (or USERPROFILE on Windows)",
        )
    })
}

/// A folder as the settings file spells it, so the same folder typed two ways
/// is one entry and the storage guard can recognise it.
fn root_of(argument: &OsString) -> String {
    paths::normalize(&argument.to_string_lossy())
}

fn one_folder(rest: &[OsString], usage: &'static str) -> io::Result<String> {
    match rest {
        [folder] => Ok(root_of(folder)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, usage)),
    }
}

/// Watches folders until the terminal is closed, printing rows as they are
/// recorded.
///
/// The journal, the settings and the exclusions are the ones the windows use:
/// this is the same watch, without a window. Folders named here are watched for
/// as long as the command runs and not remembered — `watches enable` is how a
/// folder is switched on for good.
fn watch(rest: &[OsString]) -> io::Result<()> {
    let storage = storage()?;
    let roots: Vec<String> = if rest.is_empty() {
        storage
            .watches()
            .into_iter()
            .filter(|watch| watch.enabled)
            .map(|watch| watch.path)
            .collect()
    } else {
        rest.iter().map(root_of).collect()
    };
    if roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nothing to watch: name a folder, or switch one on with `watches enable FOLDER`",
        ));
    }
    let mut sessions = Vec::new();
    for root in roots {
        // Watching the journal's own folder is a feedback loop; the store
        // refuses to record it either way, and saying so beats a watch that
        // silently reports nothing.
        if storage.is_own(&root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{root} is where Pathlight keeps its own records, so it cannot be watched"),
            ));
        }
        let session =
            pathlight_core::watch::Session::start(&root, storage.clone(), |alert, scope| {
                // On stderr, so `watch | grep` still reads as rows while a finding
                // is still seen by somebody watching the terminal.
                eprintln!("! {} — {}", alert_title(alert, scope), alert_body(alert));
            })
            .map_err(io::Error::other)?;
        println!("Watching {root}");
        sessions.push((root, session, 0u64));
    }
    println!(
        "Recording to {}. Press Ctrl-C to stop.",
        storage.journal().display()
    );
    loop {
        std::thread::sleep(Duration::from_millis(500));
        for (root, session, printed) in &mut sessions {
            let live = session.live();
            // Newest first in `rows`, so the new ones are the front slice;
            // printed oldest first, which is how a log reads.
            let fresh = (live.event_count - *printed).min(live.rows.len() as u64) as usize;
            for event in live.rows.iter().take(fresh).rev() {
                print_row(event);
            }
            *printed = live.event_count;
            if let Some(error) = &live.error {
                eprintln!("! {root}: {error}");
            }
        }
    }
}

fn print_row(event: &ActivityEvent) {
    let size = event
        .byte_delta
        .map(|bytes| format!("  {}", human_bytes(bytes)))
        .unwrap_or_default();
    let writer = event
        .process_name
        .as_deref()
        .map(|name| format!("  by {name}"))
        .unwrap_or_default();
    println!(
        "{}  {:<9}  {}{size}{writer}",
        event
            .timestamp_text()
            .unwrap_or_else(|| "unknown time".to_owned()),
        kind_label(event.kind),
        event.path,
    );
}

/// The folders a machine keeps its churn in, which nobody would think to type:
/// `AppData\Local`, `~/.cache`, the whole disk. The same list the window
/// offers beside its folder picker.
fn presets(rest: &[OsString]) -> io::Result<()> {
    if !rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pathlight-monitor presets",
        ));
    }
    for preset in pathlight_core::presets::available() {
        println!("{:<20}{}", preset.title, preset.path);
    }
    println!("\nAdd one with `watches add FOLDER`.");
    Ok(())
}

/// The remembered folders, and the switches the windows show as checkboxes.
fn watches(rest: &[OsString]) -> io::Result<()> {
    let storage = storage()?;
    let verb = rest.first().and_then(|arg| arg.to_str()).unwrap_or("list");
    match verb {
        "list" if rest.len() <= 1 => {
            let watches = storage.watches();
            if watches.is_empty() {
                println!("No folders yet. Add one with `watches add FOLDER`.");
            }
            for watch in watches {
                println!(
                    "{}  {}",
                    if watch.enabled { "on " } else { "off" },
                    watch.path
                );
            }
            Ok(())
        }
        "add" => {
            let root = one_folder(&rest[1..], "usage: pathlight-monitor watches add FOLDER")?;
            if storage.is_own(&root) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{root} is where Pathlight keeps its own records"),
                ));
            }
            let mut watches = storage.watches();
            if watches.iter().any(|watch| watch.path == root) {
                println!("{root} is already remembered.");
                return Ok(());
            }
            // Switched off, like the windows add it: nothing starts recording
            // because a folder was named.
            watches.push(WatchTarget {
                path: root.clone(),
                enabled: false,
            });
            storage.set_watches(&watches)?;
            println!("Added {root}, switched off. `watches enable {root}` switches it on.");
            Ok(())
        }
        "remove" => {
            let root = one_folder(&rest[1..], "usage: pathlight-monitor watches remove FOLDER")?;
            let mut watches = storage.watches();
            let before = watches.len();
            watches.retain(|watch| watch.path != root);
            if watches.len() == before {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{root} is not one of the remembered folders"),
                ));
            }
            storage.set_watches(&watches)?;
            println!("Forgot {root}. What was recorded for it is still in the journal.");
            Ok(())
        }
        "enable" | "disable" => {
            let enabled = verb == "enable";
            let root = one_folder(
                &rest[1..],
                "usage: pathlight-monitor watches enable|disable FOLDER",
            )?;
            storage.set_watch_enabled(&root, enabled)?;
            println!("{root} is switched {}.", if enabled { "on" } else { "off" });
            Ok(())
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pathlight-monitor watches [list|add|remove|enable|disable] [FOLDER]",
        )),
    }
}

fn history(rest: &[OsString]) -> io::Result<()> {
    let root = one_folder(rest, "usage: pathlight-monitor history FOLDER")?;
    let snapshot = storage()?
        .history(&root)
        .map_err(|error| io::Error::other(error.to_string()))?;
    println!("{}", snapshot.root_path);
    println!(
        "{} change(s), net {}{}",
        snapshot.event_count,
        human_bytes(snapshot.total_net_byte_delta),
        match snapshot.unknown_size_event_count {
            0 => String::new(),
            unknown => format!(" ({unknown} of unknown size)"),
        }
    );
    if snapshot.is_truncated {
        println!("The rows below are the newest; the totals above cover every row.");
    }
    for event in &snapshot.recent_events {
        print_row(event);
    }
    Ok(())
}

/// What was recorded, as a file: a spreadsheet's CSV, or the report a person
/// reads. One command for both, because everything but the last line — which
/// folder, is there anything, where does it go — is the same question.
fn export(rest: &[OsString], as_report: bool) -> io::Result<()> {
    let command = match as_report {
        true => "report",
        false => "export",
    };
    let (root, target) = match rest {
        [folder] => (root_of(folder), None),
        [folder, file] => (root_of(folder), Some(PathBuf::from(file))),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("usage: pathlight-monitor {command} FOLDER [FILE]"),
            ))
        }
    };
    let events = storage()?
        .rows(&root)
        .map_err(|error| io::Error::other(error.to_string()))?;
    if events.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("nothing has been recorded for {root} yet"),
        ));
    }
    let text = match as_report {
        true => pathlight_core::export::markdown(&events, &root),
        false => pathlight_core::export::csv(&events),
    };
    match target {
        Some(path) => {
            std::fs::write(&path, text)?;
            println!("Exported {} change(s) to {}.", events.len(), path.display());
        }
        // Standard output, so this composes with the rest of a shell. The
        // count goes to stderr rather than into the CSV.
        None => {
            print!("{text}");
            eprintln!("{} change(s).", events.len());
        }
    }
    Ok(())
}

/// Every setting the watches read, and the one command that changes them.
///
/// The same values the windows edit, in the same file: a `settings` change here
/// is what the next watch on any host reads.
fn settings(rest: &[OsString]) -> io::Result<()> {
    let storage = storage()?;
    let Some(key) = rest.first().and_then(|arg| arg.to_str()) else {
        return show_settings(&storage);
    };
    let values: Vec<String> = rest[1..]
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    let first = values.first().map(String::as_str).unwrap_or_default();
    match key {
        "retention-days" => {
            let (_, cap) = storage.retention();
            storage.set_retention(number(first, key)?, cap)?;
        }
        "journal-limit-mb" => {
            let (days, _) = storage.retention();
            let megabytes: u64 = number(first, key)?;
            let cap = megabytes.checked_mul(1_000_000).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "that cap is larger than any disk",
                )
            })?;
            storage.set_retention(days, cap)?;
        }
        "aggregate-retention-days" => storage.set_aggregate_retention_days(number(first, key)?)?,
        "min-delta-bytes" => storage.set_minimum_byte_delta(number(first, key)?)?,
        "growth-alert-mb" => {
            let megabytes: i64 = number(first, key)?;
            let bytes = megabytes.checked_mul(1_000_000).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "that size is larger than any disk",
                )
            })?;
            storage.set_growth_alert_bytes(bytes)?;
        }
        "records" => match first {
            // Named rows and grouped rows are the two answers; the window only
            // means anything for the second.
            "file-names" => {
                storage.set_recording(true, storage.options().aggregation_window_secs)?
            }
            "grouped" => storage.set_recording(
                false,
                match values.get(1) {
                    Some(seconds) => number(seconds, "records grouped")?,
                    None => pathlight_core::store::DEFAULT_AGGREGATION_WINDOW_SECS,
                },
            )?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "records takes file-names, or grouped [SECONDS]",
                ))
            }
        },
        "min-file-bytes" | "max-file-bytes" => {
            let bound = match first {
                "" | "none" | "any" => None,
                value => Some(number(value, key)?),
            };
            let (min, max) = storage.size_bounds();
            let (min, max) = match key {
                "min-file-bytes" => (bound, max),
                _ => (min, bound),
            };
            if min.zip(max).is_some_and(|(min, max)| min > max) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the smallest size is above the largest, which records nothing",
                ));
            }
            storage.set_size_bounds(min, max)?;
        }
        "latency" => match first {
            "immediate" => storage.set_latency_ms(DEFAULT_LATENCY_MS)?,
            "power-saving" => storage.set_latency_ms(BACKGROUND_LATENCY_MS)?,
            value => storage.set_latency_ms(number(value, "latency")?)?,
        },
        "encrypt" => storage.set_encrypting(switch(first, key)?)?,
        "patterns" => {
            let patterns: Vec<String> = match first {
                "default" => pathlight_core::exclusion::DEFAULT_PATTERNS
                    .iter()
                    .map(|pattern| (*pattern).to_owned())
                    .collect(),
                // An empty list is "record everything", which is a different
                // answer from never having chosen and has to be asked for.
                "none" => Vec::new(),
                _ => values.clone(),
            };
            storage.set_patterns(&patterns)?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{key} is not a setting; run `pathlight-monitor settings` to see them"),
            ))
        }
    }
    show_settings(&storage)
}

fn show_settings(storage: &Storage) -> io::Result<()> {
    let (days, cap) = storage.retention();
    let options = storage.options();
    let (min, max) = storage.size_bounds();
    let bound =
        |value: Option<i64>| value.map_or_else(|| "any".to_owned(), |value| value.to_string());
    // Padded so the keys read as a column; the first two are what this install
    // is, not settings, and are named as such.
    let say = |key: &str, value: String| println!("{key:<26}{value}");
    say("folder", storage.dir().to_string_lossy().into_owned());
    say(
        "recorded",
        match std::fs::metadata(storage.journal()).map(|meta| meta.len()) {
            Ok(bytes) => human_bytes(bytes as i64).trim_start_matches('+').to_owned(),
            Err(_) => "nothing recorded yet".to_owned(),
        },
    );
    say("retention-days", days.to_string());
    say(
        "aggregate-retention-days",
        storage.aggregate_retention_days().to_string(),
    );
    say("journal-limit-mb", (cap / 1_000_000).to_string());
    say(
        "min-delta-bytes",
        options.minimum_recorded_byte_delta.to_string(),
    );
    say(
        "growth-alert-mb",
        match storage.growth_alert_bytes() {
            0 => "never".to_owned(),
            bytes => (bytes / 1_000_000).to_string(),
        },
    );
    say("min-file-bytes", bound(min));
    say("max-file-bytes", bound(max));
    say(
        "records",
        match options.records_file_names {
            true => "file-names".to_owned(),
            false => format!("grouped {}", options.aggregation_window_secs),
        },
    );
    say(
        "latency",
        format!(
            "{} ms  ({})",
            storage.latency_ms(),
            match storage.latency_ms() >= BACKGROUND_LATENCY_MS {
                true => "power-saving",
                false => "immediate",
            }
        ),
    );
    say("encrypt", storage.encrypting().to_string());
    say("patterns", storage.patterns().join(" "));
    println!("{SETTING_VALUES}");
    Ok(())
}

fn number<T: std::str::FromStr>(value: &str, key: &str) -> io::Result<T> {
    value.trim().parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{key} needs a whole number"),
        )
    })
}

fn switch(value: &str, key: &str) -> io::Result<bool> {
    match value {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{key} is on or off"),
        )),
    }
}

/// Whether the watches come back after a sign-out.
///
/// The item launches this command with `watch`, so the folders that are
/// switched on are watched with no terminal open.
fn autostart(rest: &[OsString]) -> io::Result<()> {
    let item = pathlight_core::autostart::login_item(&["watch"])?;
    match rest.first().and_then(|arg| arg.to_str()) {
        None | Some("status") => {}
        Some(value) => item.set(switch(value, "autostart")?)?,
    }
    println!(
        "autostart {}
  {}",
        match item.is_enabled() {
            true => "on",
            false => "off",
        },
        item.command()
    );
    Ok(())
}

fn run() -> io::Result<()> {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    // `first`, not `args[0]`: no arguments at all is the most likely way this
    // binary is ever run, and it used to panic.
    match args
        .first()
        .and_then(|arg| arg.to_str())
        .unwrap_or_default()
    {
        "" | "-h" | "--help" | "help" => {
            print!("{HELP}");
            return Ok(());
        }
        "uninstall" => return uninstall(&args[1..]),
        "install-cli" => return install_cli(&args[1..]),
        "watch" => return watch(&args[1..]),
        "watches" => return watches(&args[1..]),
        "presets" => return presets(&args[1..]),
        "history" => return history(&args[1..]),
        "export" => return export(&args[1..], false),
        "report" => return export(&args[1..], true),
        "settings" => return settings(&args[1..]),
        "autostart" => return autostart(&args[1..]),
        // Named or not: the recording form is what this binary was before the
        // other commands existed, and scripts pass the two paths bare.
        "record" => {
            args.remove(0);
        }
        _ => {}
    }
    let bounds = pathlight_core::recording::SizeBounds {
        min_bytes: take_bytes(&mut args, "--min-bytes")?,
        max_bytes: take_bytes(&mut args, "--max-bytes")?,
    };
    if bounds
        .min_bytes
        .zip(bounds.max_bytes)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--min-bytes is above --max-bytes, which would record nothing",
        ));
    }
    if !(2..=3).contains(&args.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pathlight-monitor ROOT JOURNAL [SECONDS=10] [--min-bytes N] [--max-bytes N]",
        ));
    }
    let seconds = args.get(2).map_or(Ok(10), |value| {
        value
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "seconds must be a positive integer",
                )
            })
    })?;
    let root = PathBuf::from(&args[0]);
    let journal = PathBuf::from(&args[1]);
    eprintln!(
        "Preparing {} for a {seconds}-second recording…",
        root.display()
    );
    let summary = pathlight_core::recording::record_bounded(
        &root,
        &journal,
        Duration::from_secs(seconds),
        bounds,
        || {},
    )?;
    println!(
        "Saved {} observations, {} gap records, {} total evidence records to {}",
        summary.observations,
        summary.gaps,
        summary.records_written,
        journal.display()
    );
    Ok(())
}
