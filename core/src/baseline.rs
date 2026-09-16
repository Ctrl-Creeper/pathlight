//! The last known state of a watched folder, on disk.
//!
//! Resident it was the largest thing a background watch held — a whole-disk
//! baseline is millions of entries and only two moments need it: a gap, and
//! the restart after one. On disk it costs nothing between those moments, and
//! it survives the process, so the changes made while Pathlight was closed
//! stop being invisible.
//!
//! The format is this crate's own and only this machine reads it: a version
//! byte guards it, and anything that does not read back is treated as no
//! baseline at all rather than as a wrong one.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::measurement::{FileKind, FileMeasurement, IdentityPlatform, ObjectIdentity};
use crate::snapshot::{ScanConsistency, ScanError, ScanSnapshot};

const MAGIC: &[u8; 8] = b"PLBASE\x01\n";

/// Where the baseline for `scope` lives under `dir`.
///
/// Named by a hash rather than by the path it stands for: a watch scope is a
/// whole path, which is longer than a file name may be and full of separators.
pub fn file(dir: &Path, scope: &str) -> PathBuf {
    directory(dir).join(format!("{:016x}.plb", fnv1a(scope)))
}

/// Where every baseline under `dir` lives. One place, so "forget everything
/// recorded" can empty it: a baseline names the same files the rows do.
pub fn directory(dir: &Path) -> PathBuf {
    dir.join("baselines")
}

/// ponytail: FNV-1a rather than a real hash — this names a cache file, and a
/// collision costs one unnecessary rescan, not a wrong answer, because the
/// root the file records is checked on the way back in.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Write `snapshot` where [`file`] says, through a temporary so an interrupted
/// write leaves the old baseline rather than half of a new one.
pub fn write(path: &Path, snapshot: &ScanSnapshot) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("writing");
    {
        let mut out = BufWriter::new(File::create(&temporary)?);
        out.write_all(MAGIC)?;
        put_bytes(&mut out, &path_bytes(&snapshot.root))?;
        put_time(&mut out, snapshot.started_at)?;
        put_time(&mut out, snapshot.finished_at)?;
        // Only whether the scan was whole is kept: the errors themselves are
        // read once, when they happen, and a list of them is not what the next
        // run compares against.
        out.write_all(&[u8::from(snapshot.is_complete())])?;
        out.write_all(&(snapshot.entries.len() as u64).to_le_bytes())?;
        for (entry, measurement) in &snapshot.entries {
            put_bytes(&mut out, &path_bytes(entry))?;
            put_measurement(&mut out, measurement)?;
        }
        out.flush()?;
    }
    fs::rename(&temporary, path)
}

/// The baseline at `path`, or `None` when there is none, it is from another
/// version, or it does not read back whole. Never a partial snapshot: half a
/// baseline would report every file it is missing as deleted.
pub fn read(path: &Path, root: &Path) -> Option<ScanSnapshot> {
    let snapshot = read_whole(path).ok()?;
    (snapshot.root == root).then_some(snapshot)
}

fn read_whole(path: &Path) -> io::Result<ScanSnapshot> {
    let mut input = BufReader::new(File::open(path)?);
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a baseline this version wrote",
        ));
    }
    let root = path_from_bytes(take_bytes(&mut input)?);
    let started_at = take_time(&mut input)?;
    let finished_at = take_time(&mut input)?;
    let complete = take_u8(&mut input)? == 1;
    let count = usize::try_from(take_u64(&mut input)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "more entries than fit"))?;
    let mut entries = Vec::new();
    // Not `with_capacity(count)`: a corrupt length would ask for the whole of
    // memory before a single entry had been read.
    for _ in 0..count {
        let entry = path_from_bytes(take_bytes(&mut input)?);
        entries.push((
            Box::<Path>::from(entry.as_path()),
            take_measurement(&mut input)?,
        ));
    }
    Ok(ScanSnapshot {
        root,
        started_at,
        finished_at,
        consistency: ScanConsistency::ObservedInterval,
        entries,
        errors: match complete {
            true => Vec::new(),
            false => vec![ScanError {
                path: PathBuf::new(),
                kind: io::ErrorKind::Other,
                message: "parts of this folder could not be read when it was last scanned"
                    .to_owned(),
            }],
        },
    })
}

fn put_bytes(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    out.write_all(&(bytes.len() as u32).to_le_bytes())?;
    out.write_all(bytes)
}

fn take_bytes(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length)?;
    let mut bytes = vec![0_u8; u32::from_le_bytes(length) as usize];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn put_time(out: &mut impl Write, time: SystemTime) -> io::Result<()> {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    out.write_all(&since.as_secs().to_le_bytes())?;
    out.write_all(&since.subsec_nanos().to_le_bytes())
}

fn take_time(input: &mut impl Read) -> io::Result<SystemTime> {
    let secs = take_u64(input)?;
    let mut nanos = [0_u8; 4];
    input.read_exact(&mut nanos)?;
    Ok(UNIX_EPOCH + Duration::new(secs, u32::from_le_bytes(nanos)))
}

fn take_u8(input: &mut impl Read) -> io::Result<u8> {
    let mut byte = [0_u8; 1];
    input.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn take_u64(input: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn put_option_u64(out: &mut impl Write, value: Option<u64>) -> io::Result<()> {
    out.write_all(&[u8::from(value.is_some())])?;
    out.write_all(&value.unwrap_or_default().to_le_bytes())
}

fn take_option_u64(input: &mut impl Read) -> io::Result<Option<u64>> {
    let present = take_u8(input)? == 1;
    let value = take_u64(input)?;
    Ok(present.then_some(value))
}

fn put_measurement(out: &mut impl Write, measurement: &FileMeasurement) -> io::Result<()> {
    match &measurement.identity {
        Some(identity) => {
            out.write_all(&[1, platform_byte(identity.platform)])?;
            out.write_all(&identity.volume_id.to_le_bytes())?;
            out.write_all(&identity.file_id.to_le_bytes())?;
        }
        None => out.write_all(&[0, 0])?,
    }
    out.write_all(&[kind_byte(measurement.kind)])?;
    out.write_all(&measurement.logical_bytes.to_le_bytes())?;
    put_option_u64(out, measurement.allocated_bytes)?;
    put_option_u64(out, measurement.link_count)?;
    put_time(out, measurement.measured_at)
}

fn take_measurement(input: &mut impl Read) -> io::Result<FileMeasurement> {
    let present = take_u8(input)? == 1;
    let platform = take_u8(input)?;
    let identity = match present {
        true => {
            let volume_id = take_u64(input)?;
            let mut file_id = [0_u8; 16];
            input.read_exact(&mut file_id)?;
            Some(ObjectIdentity {
                platform: platform_of(platform),
                volume_id,
                file_id: u128::from_le_bytes(file_id),
            })
        }
        false => None,
    };
    Ok(FileMeasurement {
        identity,
        kind: kind_of(take_u8(input)?),
        logical_bytes: take_u64(input)?,
        allocated_bytes: take_option_u64(input)?,
        link_count: take_option_u64(input)?,
        measured_at: take_time(input)?,
    })
}

fn platform_byte(platform: IdentityPlatform) -> u8 {
    match platform {
        IdentityPlatform::Unix => 0,
        IdentityPlatform::Windows => 1,
    }
}

fn platform_of(byte: u8) -> IdentityPlatform {
    match byte {
        1 => IdentityPlatform::Windows,
        _ => IdentityPlatform::Unix,
    }
}

fn kind_byte(kind: FileKind) -> u8 {
    match kind {
        FileKind::File => 0,
        FileKind::Directory => 1,
        FileKind::Symlink => 2,
        FileKind::Other => 3,
    }
}

fn kind_of(byte: u8) -> FileKind {
    match byte {
        1 => FileKind::Directory,
        2 => FileKind::Symlink,
        3 => FileKind::Other,
        _ => FileKind::File,
    }
}

/// Names go out the way the platform spells them. A file whose name is not
/// valid Unicode is a file this watch still has to name correctly.
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    PathBuf::from(std::ffi::OsString::from_wide(&wide))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot;

    /// What is written has to come back the same, or a gap would be recovered
    /// against a folder that never existed.
    #[test]
    fn a_baseline_reads_back_as_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("watched");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::create_dir(root.join("inner")).unwrap();
        std::fs::write(root.join("inner").join("b.bin"), vec![0_u8; 300]).unwrap();
        let taken = snapshot::scan(&root).unwrap();
        let path = file(dir.path(), &root.to_string_lossy());

        write(&path, &taken).unwrap();
        let back = read(&path, &root).unwrap();

        assert_eq!(back.entries.len(), taken.entries.len());
        assert_eq!(back.root, taken.root);
        assert_eq!(back.is_complete(), taken.is_complete());
        for ((left, one), (right, two)) in back.entries.iter().zip(taken.entries.iter()) {
            assert_eq!(left, right);
            assert_eq!(one, two);
        }
        // The point of keeping it: nothing changed, so nothing is recovered.
        let comparison = snapshot::reconcile(&back, &taken, snapshot::IdentityContinuity::Unknown);
        assert!(comparison.bindings.is_empty(), "{:?}", comparison.bindings);
    }

    /// A truncated or foreign file is no baseline, not a baseline saying every
    /// file it is missing was deleted.
    #[test]
    fn a_file_that_does_not_read_back_is_no_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("watched");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        let taken = snapshot::scan(&root).unwrap();
        let path = file(dir.path(), "scope");
        write(&path, &taken).unwrap();

        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 4]).unwrap();
        assert!(read(&path, &root).is_none(), "half a baseline was accepted");

        std::fs::write(&path, b"something else entirely").unwrap();
        assert!(read(&path, &root).is_none());

        // The right file for the wrong folder is the wrong answer too.
        std::fs::write(&path, &whole).unwrap();
        assert!(read(&path, Path::new("/somewhere/else")).is_none());
    }
}
