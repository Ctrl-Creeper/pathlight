use std::env;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("pathlight-monitor: {error}");
        std::process::exit(1);
    }
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
    let present: Vec<_> = paths.into_iter().filter(|path| path.exists()).collect();
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
        println!("Usage: pathlight-monitor ROOT JOURNAL [SECONDS=10]\n       pathlight-monitor uninstall [--yes]\nRecords an explicit live session and two interval snapshots. JOURNAL must be outside ROOT. Registration and scans add to the duration.\nUninstall lists Pathlight's own storage on this OS and, with --yes, removes it. Journals you named yourself are never guessed at.");
        return Ok(());
    }
    // `first`, not `args[0]`: no arguments at all is the most likely way this
    // binary is ever run, and it used to panic.
    if args.first().is_some_and(|arg| arg == "uninstall") {
        return uninstall(&args[1..]);
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
