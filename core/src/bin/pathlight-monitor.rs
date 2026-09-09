use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

fn run() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("Usage: pathlight-monitor ROOT JOURNAL [SECONDS=10]\n       pathlight-monitor install-cli\n       pathlight-monitor uninstall [--yes]\nRecords an explicit live session and two interval snapshots. JOURNAL must be outside ROOT. Registration and scans add to the duration.\nInstall-cli puts this command in your own home directory and says how to reach it from a shell.\nUninstall lists Pathlight's own storage on this OS, plus an installed command, and with --yes removes them. Journals you named yourself are never guessed at.");
        return Ok(());
    }
    // `first`, not `args[0]`: no arguments at all is the most likely way this
    // binary is ever run, and it used to panic.
    if args.first().is_some_and(|arg| arg == "uninstall") {
        return uninstall(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "install-cli") {
        return install_cli(&args[1..]);
    }
    if !(2..=3).contains(&args.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pathlight-monitor ROOT JOURNAL [SECONDS=10]",
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
    let summary =
        pathlight_core::recording::record_for(&root, &journal, Duration::from_secs(seconds))?;
    println!(
        "Saved {} observations, {} gap records, {} total evidence records to {}",
        summary.observations,
        summary.gaps,
        summary.records_written,
        journal.display()
    );
    Ok(())
}
