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

fn run() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("Usage: pathlight-monitor ROOT JOURNAL [SECONDS=10]\nRecords an explicit live session and two interval snapshots. JOURNAL must be outside ROOT. Registration and scans add to the duration.");
        return Ok(());
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
