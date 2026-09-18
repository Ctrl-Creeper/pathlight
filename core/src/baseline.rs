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
//!
//! It lists every file under the watch by name, which is what the journal
//! lists too — so when the journal is encrypted, this is, under the same key,
//! in frames small enough that a whole-disk baseline never has to be held
//! twice to be sealed or opened.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::measurement::{FileKind, FileMeasurement, IdentityPlatform, ObjectIdentity};
use crate::snapshot::{ScanConsistency, ScanError, ScanSnapshot};

const MAGIC: &[u8; 8] = b"PLBASE\x03\n";
const PLAIN: u8 = 0;
const SEALED: u8 = 1;
/// How much goes into one frame before it is sealed and written.
const FRAME: usize = 1 << 20;
/// A frame longer than this is not one this build wrote: the cap is what
/// stops a corrupt length from asking for the whole of memory. A frame is
/// at most `FRAME` plus the one write that overflowed it, plus the tag.
const MAX_FRAME: usize = FRAME + MAX_STRING + 64;
/// Longer than any path a filesystem here allows; the same guard, per string.
const MAX_STRING: usize = 1 << 16;

/// The body in frames, each sealed when there is a key. `Write` so the body
/// is spelled once, whichever way it is stored.
struct Frames<W: Write> {
    out: W,
    key: Option<[u8; 32]>,
    pending: Vec<u8>,
}

impl<W: Write> Frames<W> {
    fn flush_frame(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let frame = match &self.key {
            Some(key) => crate::crypt::seal_bytes(key, &self.pending)
                .map_err(|error| io::Error::other(error.to_string()))?,
            None => std::mem::take(&mut self.pending),
        };
        self.pending.clear();
        self.out.write_all(&(frame.len() as u32).to_le_bytes())?;
        self.out.write_all(&frame)
    }
}

impl<W: Write> Write for Frames<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.pending.len() + bytes.len() > FRAME {
            self.flush_frame()?;
        }
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_frame()?;
        self.out.flush()
    }
}

/// The frames back into a body. A frame that does not open is the end of a
/// file that cannot be trusted, which the caller reads as no baseline.
struct Unframed<R: Read> {
    input: R,
    key: Option<[u8; 32]>,
    buffer: Vec<u8>,
    at: usize,
}

impl<R: Read> Unframed<R> {
    fn refill(&mut self) -> io::Result<bool> {
        let mut length = [0_u8; 4];
        match self.input.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error),
        }
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
        }
        let mut frame = vec![0_u8; length];
        self.input.read_exact(&mut frame)?;
        self.buffer = match &self.key {
            Some(key) => crate::crypt::open_bytes(key, &frame).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "a frame did not open")
            })?,
            None => frame,
        };
        self.at = 0;
        Ok(true)
    }
}

impl<R: Read> Read for Unframed<R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if self.at == self.buffer.len() && !self.refill()? {
            return Ok(0);
        }
        let count = into.len().min(self.buffer.len() - self.at);
        into[..count].copy_from_slice(&self.buffer[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

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
pub(crate) fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// What every file held when the folder was last walked, for the two
/// questions a live change cannot answer on its own: how big was the file
/// that just vanished, and how big was this one before it was modified.
/// The size index knows files the watch has already touched; this knows the
/// rest.
///
/// ponytail: a sorted `(fnv1a(path), bytes)` table, 16 bytes a file, so a
/// whole-disk watch of five million files costs 80 MB rather than the paths
/// themselves. A collision misreports one size as an estimate; spill this to a
/// file beside the baseline if that memory shows up.
#[derive(Default)]
pub struct Sizes {
    table: Vec<(u64, u64)>,
}

impl Sizes {
    pub fn from(snapshot: &ScanSnapshot) -> Self {
        let mut table: Vec<(u64, u64)> = snapshot
            .entries
            .iter()
            .filter_map(|(path, measurement)| {
                let bytes = measurement.allocated_bytes?;
                Some((
                    fnv1a(&crate::paths::normalize(&path.to_string_lossy())),
                    bytes,
                ))
            })
            .collect();
        table.sort_unstable();
        table.dedup_by_key(|entry| entry.0);
        Self { table }
    }

    pub fn get(&self, path: &str) -> Option<u64> {
        let hash = fnv1a(path);
        self.table
            .binary_search_by_key(&hash, |entry| entry.0)
            .ok()
            .map(|at| self.table[at].1)
    }
}

/// Write `snapshot` where [`file`] says, through a temporary so an interrupted
/// write leaves the old baseline rather than half of a new one. Sealed under
/// `key` when there is one.
pub fn write(path: &Path, snapshot: &ScanSnapshot, key: Option<[u8; 32]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        // Owner-only, like the journal: this is a list of every file there is.
        if let Some(storage) = parent.parent() {
            crate::journal::set_permissions(storage, 0o700)?;
        }
        crate::journal::set_permissions(parent, 0o700)?;
    }
    let temporary = path.with_extension("writing");
    {
        let file = File::create(&temporary)?;
        crate::journal::set_permissions(&temporary, 0o600)?;
        let mut file = BufWriter::new(file);
        file.write_all(MAGIC)?;
        file.write_all(&[if key.is_some() { SEALED } else { PLAIN }])?;
        let mut out = Frames {
            out: file,
            key,
            pending: Vec::new(),
        };
        put_bytes(&mut out, &path_bytes(&snapshot.root))?;
        put_time(&mut out, snapshot.started_at)?;
        put_time(&mut out, snapshot.finished_at)?;
        // Which folders could not be read, not why: a comparison sets aside
        // what is under them, and needs nothing else.
        out.write_all(&(snapshot.errors.len() as u64).to_le_bytes())?;
        for error in &snapshot.errors {
            put_bytes(&mut out, &path_bytes(&error.path))?;
        }
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
pub fn read(path: &Path, root: &Path, key: Option<[u8; 32]>) -> Option<ScanSnapshot> {
    let snapshot = read_whole(path, key).ok()?;
    (snapshot.root == root).then_some(snapshot)
}

fn read_whole(path: &Path, key: Option<[u8; 32]>) -> io::Result<ScanSnapshot> {
    let mut file = BufReader::new(File::open(path)?);
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a baseline this version wrote",
        ));
    }
    let key = match (take_u8(&mut file)?, key) {
        (PLAIN, _) => None,
        (SEALED, Some(key)) => Some(key),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a sealed baseline and no key to open it",
            ))
        }
    };
    let mut input = Unframed {
        input: file,
        key,
        buffer: Vec::new(),
        at: 0,
    };
    let root = path_from_bytes(take_bytes(&mut input)?);
    let started_at = take_time(&mut input)?;
    let finished_at = take_time(&mut input)?;
    let too_many = || io::Error::new(io::ErrorKind::InvalidData, "more entries than fit");
    let unread = usize::try_from(take_u64(&mut input)?).map_err(|_| too_many())?;
    let mut errors = Vec::new();
    for _ in 0..unread {
        errors.push(ScanError {
            path: path_from_bytes(take_bytes(&mut input)?),
            kind: io::ErrorKind::Other,
            message: "could not be read when this folder was last scanned".to_owned(),
        });
    }
    let count = usize::try_from(take_u64(&mut input)?).map_err(|_| too_many())?;
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
        errors,
    })
}

fn put_bytes(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    out.write_all(&(bytes.len() as u32).to_le_bytes())?;
    out.write_all(bytes)
}

fn take_bytes(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_STRING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "string too long",
        ));
    }
    let mut bytes = vec![0_u8; length];
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
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
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

        write(&path, &taken, None).unwrap();
        let back = read(&path, &root, None).unwrap();

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
        write(&path, &taken, None).unwrap();

        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 4]).unwrap();
        assert!(
            read(&path, &root, None).is_none(),
            "half a baseline was accepted"
        );

        std::fs::write(&path, b"something else entirely").unwrap();
        assert!(read(&path, &root, None).is_none());

        // The right file for the wrong folder is the wrong answer too.
        std::fs::write(&path, &whole).unwrap();
        assert!(read(&path, Path::new("/somewhere/else"), None).is_none());
    }

    /// An encrypted journal keeps file names off the disk in the clear. A
    /// baseline that listed them beside it would undo that.
    #[test]
    fn a_sealed_baseline_names_no_file_and_opens_only_with_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("watched");
        std::fs::create_dir(&root).unwrap();
        // More than one frame, so the seam between frames is read across too.
        for index in 0..8000 {
            std::fs::write(
                root.join(format!(
                    "confidential-{index:04}-name-that-must-not-show.txt"
                )),
                b"x",
            )
            .unwrap();
        }
        let taken = snapshot::scan(&root).unwrap();
        let key = [7_u8; 32];
        let path = file(dir.path(), "scope");

        write(&path, &taken, Some(key)).unwrap();

        let stored = std::fs::read(&path).unwrap();
        assert!(stored.len() > FRAME, "one frame does not test the seam");
        assert!(
            !stored.windows(12).any(|window| window == b"confidential"),
            "a file name is on disk in the clear"
        );
        assert!(read(&path, &root, None).is_none(), "opened without the key");
        assert!(
            read(&path, &root, Some([8_u8; 32])).is_none(),
            "opened with the wrong key"
        );
        let back = read(&path, &root, Some(key)).unwrap();
        assert_eq!(back.entries.len(), taken.entries.len());
        assert_eq!(back.entries.last(), taken.entries.last());
    }
}
