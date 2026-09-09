//! The NTFS change journal: the Windows watcher that needs privilege.
//!
//! `ReadDirectoryChangesW` watches one directory handle through a fixed
//! buffer, and when a burst overruns that buffer the kernel says only that
//! something was lost. It also hands out no cursor at all, so a restart cannot
//! ask what changed while the app was closed and every launch re-baselines.
//! The USN journal is the volume's own log of every change, numbered with a
//! USN that keeps increasing across reboots: the one source on Windows a
//! stored cursor can resume from, and one handle covers the whole volume with
//! no per-directory bookkeeping.
//!
//! The price is a handle to the volume device, which only an administrator can
//! open, and records for everything on that volume, which this filters down to
//! the watched root. Availability is probed, never inferred: the journal can
//! be absent, disabled, or on a filesystem that serves only the newer record
//! format.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::mem::size_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, Prefix};
use std::ptr;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_IO_PENDING, ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED,
    ERROR_JOURNAL_NOT_ACTIVE, ERROR_OPERATION_ABORTED, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileIdType, GetFinalPathNameByHandleW, OpenFileById, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OVERLAPPED, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_CLOSE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_RECORD_V2,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE,
};
use windows_sys::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED,
};

use crate::monitor::{Backend, Capabilities, ChangeKind, Emitter, StreamEvent};
use crate::notify_backend::RootIdentity;

/// The journal numbers its own records, so a stored cursor really resumes, and
/// it holds both names of a move, so a rename is one event. It cannot name a
/// process: USN records carry no pid, and promising the host a column nothing
/// can fill is worse than admitting Windows does not tell.
pub(crate) fn capabilities() -> Option<Capabilities> {
    if !requested() || !journaled() {
        return None;
    }
    Some(Capabilities {
        resumable_cursor: true,
        pairs_renames: true,
        reports_process: false,
        may_drop_events: true,
    })
}

/// Whether this run asked for the privileged watch. The same switch
/// `fanotify.rs` documents: PRIVILEGED_BACKENDS.md gates defaulting a root to
/// a privileged source on measurements that do not exist yet, and a
/// volume-wide log wakes this process for writes anywhere on the disk.
fn requested() -> bool {
    std::env::var_os("PATHLIGHT_PRIVILEGED_WATCH").is_some_and(|value| value == "1")
}

/// Whether this process can read a journal at all.
///
/// Probed once, on the volume this process runs from: what decides the answer
/// is the token, not the drive, and a host asks what its watcher promises
/// while drawing a label. A watch on a drive that turns out to have no journal
/// still falls back to `ReadDirectoryChangesW`, which announces its missing
/// cursor with `RequiresRescan`.
// ponytail: no re-probe. Nothing in the product gains administrator rights
// mid-run; a helper that could would ask again when it starts.
fn journaled() -> bool {
    static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PRESENT.get_or_init(|| {
        std::env::current_dir()
            .ok()
            .and_then(|dir| Journal::open(&dir.to_string_lossy()))
            .is_some()
    })
}

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

const WATCHED: u32 = USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_TRUNCATION
    | USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | USN_REASON_RENAME_OLD_NAME
    | USN_REASON_RENAME_NEW_NAME
    | USN_REASON_BASIC_INFO_CHANGE;

/// Bounded memory under a rename storm, as in `inotify.rs`: past this the
/// stream reports a gap instead of growing without limit.
const MAX_PENDING: usize = 1024;

/// The drive a watch lives on, or `None` when the root is not on a local
/// volume: a UNC share has no journal this process can read.
fn disk_of(root: &str) -> Option<char> {
    match Path::new(root).components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter as char),
            _ => None,
        },
        _ => None,
    }
}

fn owned(handle: HANDLE) -> Option<OwnedHandle> {
    (!handle.is_null() && handle != INVALID_HANDLE_VALUE)
        .then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn event(manual: bool) -> Option<OwnedHandle> {
    owned(unsafe { CreateEventW(ptr::null(), manual as i32, 0, ptr::null()) })
}

/// A `HANDLE` and std's `RawHandle` are the same pointer type, so this is a
/// name for the borrow, not a conversion.
fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle()
}

/// The volume's journal, plus the handles a blocking read needs to stay
/// interruptible.
struct Journal {
    volume: File,
    /// `OpenFileById` resolves a reference number against any handle on the
    /// volume; the volume's root directory is the one always there.
    hint: File,
    io: OwnedHandle,
    stop: OwnedHandle,
}

impl Journal {
    /// `None` for every reason a journal cannot be read here — not a local
    /// disk, not an administrator, no journal, or a filesystem that does not
    /// serve version 2 records. They all mean the same thing to the caller.
    fn open(root: &str) -> Option<Self> {
        let disk = disk_of(root)?;
        let journal = Self {
            volume: OpenOptions::new()
                .access_mode(GENERIC_READ)
                .share_mode(SHARE_ALL)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(format!(r"\\.\{disk}:"))
                .ok()?,
            hint: OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
                .share_mode(SHARE_ALL)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(format!(r"{disk}:\"))
                .ok()?,
            io: event(false)?,
            stop: event(true)?,
        };
        journal.data()?;
        Some(journal)
    }

    fn data(&self) -> Option<USN_JOURNAL_DATA_V0> {
        let mut data = USN_JOURNAL_DATA_V0::default();
        self.control(
            FSCTL_QUERY_USN_JOURNAL,
            ptr::null(),
            0,
            (&mut data as *mut USN_JOURNAL_DATA_V0).cast::<c_void>(),
            size_of::<USN_JOURNAL_DATA_V0>() as u32,
        )
        .ok()
        .map(|_| data)
    }

    fn read(&self, request: &READ_USN_JOURNAL_DATA_V0, buffer: &mut [u8]) -> Result<usize, u32> {
        self.control(
            FSCTL_READ_USN_JOURNAL,
            (request as *const READ_USN_JOURNAL_DATA_V0).cast::<c_void>(),
            size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
        )
        .map(|returned| returned as usize)
    }

    /// One control call on the overlapped volume handle. A journal read that
    /// waits for bytes parks in the kernel until something changes — no timer,
    /// no polling — so the wait watches the stop event too, and cancelling
    /// waits for the kernel to release both buffers before returning.
    fn control(
        &self,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
    ) -> Result<u32, u32> {
        let mut overlapped = OVERLAPPED {
            hEvent: raw(&self.io),
            ..Default::default()
        };
        let overlapped = &mut overlapped as *mut OVERLAPPED;
        let mut returned = 0u32;
        // A read that keeps completing at once would never reach the wait
        // below, so stopping while the volume is busy must not wait for it to
        // fall quiet.
        let stop = [raw(&self.stop)];
        if unsafe { WaitForMultipleObjects(1, stop.as_ptr(), 0, 0) } == WAIT_OBJECT_0 {
            return Err(ERROR_OPERATION_ABORTED);
        }
        unsafe { ResetEvent(raw(&self.io)) };
        let issued = unsafe {
            DeviceIoControl(
                raw(&self.volume),
                code,
                input,
                input_len,
                output,
                output_len,
                &mut returned,
                overlapped,
            )
        };
        if issued != 0 {
            return Ok(returned);
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(error);
        }
        let handles = [raw(&self.io), raw(&self.stop)];
        if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } != WAIT_OBJECT_0 {
            unsafe { CancelIoEx(raw(&self.volume), overlapped) };
            unsafe { GetOverlappedResult(raw(&self.volume), overlapped, &mut returned, 1) };
            return Err(ERROR_OPERATION_ABORTED);
        }
        if unsafe { GetOverlappedResult(raw(&self.volume), overlapped, &mut returned, 1) } == 0 {
            return Err(unsafe { GetLastError() });
        }
        Ok(returned)
    }

    /// A record names its parent by reference number and carries only the
    /// entry's own name, which is what makes a deletion resolvable: the parent
    /// is still there when the child is gone.
    fn directory(&self, frn: u64) -> Option<String> {
        let id = FILE_ID_DESCRIPTOR {
            dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: frn as i64 },
        };
        let directory = owned(unsafe {
            OpenFileById(
                raw(&self.hint),
                &id,
                FILE_READ_ATTRIBUTES,
                SHARE_ALL,
                ptr::null(),
                FILE_FLAG_BACKUP_SEMANTICS,
            )
        })?;
        let mut buffer = [0u16; 1024];
        let len = unsafe {
            GetFinalPathNameByHandleW(raw(&directory), buffer.as_mut_ptr(), buffer.len() as u32, 0)
        } as usize;
        // Zero is failure; anything from the buffer's length up is the size it
        // would have needed, not a path.
        if len == 0 || len >= buffer.len() {
            return None;
        }
        String::from_utf16(&buffer[..len]).ok()
    }
}

pub(crate) fn start(
    emitter: Arc<Emitter>,
    since_event_id: Option<u64>,
) -> Option<Box<dyn Backend>> {
    if !requested() {
        return None;
    }
    let journal = Arc::new(Journal::open(&emitter.root)?);
    let data = journal.data()?;
    // A cursor the journal can no longer serve is not a cursor: the log
    // wrapped past it, or this is a different journal that never issued it.
    // Either way the host re-baselines instead of being handed a replay that
    // silently starts in the middle.
    let stored = since_event_id.map(|stored| stored as i64);
    let stale = stored.is_some_and(|at| at < data.LowestValidUsn || at > data.NextUsn);
    let watch = Watch {
        journal: Arc::clone(&journal),
        emitter,
    };
    let from = stored.filter(|_| !stale).unwrap_or(data.NextUsn);
    let worker = thread::Builder::new()
        .name("pathlight-usn".into())
        .spawn(move || watch.run(from, data, stale))
        .ok()?;
    Some(Box::new(UsnBackend {
        journal,
        worker: Some(worker),
    }))
}

struct UsnBackend {
    journal: Arc<Journal>,
    worker: Option<JoinHandle<()>>,
}

impl Backend for UsnBackend {
    fn stop(&mut self) {
        // The worker is parked in the kernel waiting for the next record, and
        // this event is what ends that wait.
        unsafe { SetEvent(raw(&self.journal.stop)) };
        if let Some(worker) = self.worker.take() {
            // A listener is allowed to stop its own watch.
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for UsnBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What the reader carries between journal reads.
#[derive(Default)]
struct State {
    /// Whether the host has been told the replay is over.
    announced: bool,
    /// Old names waiting for the new name that follows them, by reference
    /// number.
    pending: Vec<(u64, String)>,
    names: Directory,
    session: Session,
}

struct Watch {
    journal: Arc<Journal>,
    emitter: Arc<Emitter>,
}

impl Watch {
    /// Reads from `from`, where `data.NextUsn` is where history ends: a
    /// resumed watch replays what the host missed and only then says it is
    /// live, which is what the marker means on macOS too.
    fn run(&self, from: i64, mut data: USN_JOURNAL_DATA_V0, stale: bool) {
        let live = data.NextUsn;
        let mut cursor = from;
        let mut state = State::default();
        if stale {
            self.gap(cursor);
        }
        let mut root = RootIdentity::of(&self.emitter.root);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            if cursor >= live && !state.announced {
                state.announced = true;
                self.emitter.emit(StreamEvent::HistoryCaughtUp {
                    event_id: cursor.max(0) as u64,
                });
            }
            let request = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: cursor,
                ReasonMask: WATCHED,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                // Park until the journal has something. Zero here would mean
                // returning empty-handed and asking again, which is polling.
                BytesToWaitFor: 1,
                UsnJournalID: data.UsnJournalID,
            };
            let read = match self.journal.read(&request, &mut buffer) {
                Ok(read) if read >= size_of::<i64>() => read,
                Err(ERROR_OPERATION_ABORTED) => break,
                Err(
                    ERROR_JOURNAL_ENTRY_DELETED
                    | ERROR_JOURNAL_NOT_ACTIVE
                    | ERROR_JOURNAL_DELETE_IN_PROGRESS,
                ) => {
                    // The log no longer covers where this watch was reading.
                    // Say so, then rejoin it wherever it now begins.
                    self.gap(cursor);
                    match self.journal.data() {
                        Some(fresh) => {
                            cursor = fresh.NextUsn;
                            data = fresh;
                            state.pending.clear();
                            continue;
                        }
                        None => break,
                    }
                }
                // A broken read is a silent watch, which is the one thing it
                // must never become without saying so.
                Ok(_) | Err(_) => {
                    self.gap(cursor);
                    break;
                }
            };
            if root.drifted() {
                self.gap(cursor);
            }
            // The journal answers with the USN to continue from, followed by
            // the records themselves.
            cursor = i64::from_ne_bytes(buffer[..8].try_into().expect("eight bytes"));
            self.publish(&buffer[..read], live, &mut state);
        }
        // Stopping must not discard a departure already accepted: an old name
        // with no new name is a file that left, whatever it became.
        for (_, gone) in state.pending.drain(..) {
            if crate::paths::is_inside(&self.emitter.root, &gone) {
                self.emitter
                    .change(ChangeKind::Deleted, gone, cursor.max(0) as u64);
            }
        }
    }

    fn gap(&self, cursor: i64) {
        self.emitter.emit(StreamEvent::RequiresRescan {
            event_id: cursor.max(0) as u64,
        });
    }

    fn publish(&self, buffer: &[u8], live: i64, state: &mut State) {
        let mut at = size_of::<i64>();
        while at + size_of::<USN_RECORD_V2>() <= buffer.len() {
            let record: USN_RECORD_V2 =
                unsafe { ptr::read_unaligned(buffer[at..].as_ptr().cast::<USN_RECORD_V2>()) };
            let len = record.RecordLength as usize;
            // A version 0 read request is answered with version 2 records, so
            // anything else means this is not the layout being read here.
            if record.MajorVersion != 2
                || len < size_of::<USN_RECORD_V2>()
                || at + len > buffer.len()
            {
                break;
            }
            let bytes = &buffer[at..at + len];
            at += len;
            let event_id = record.Usn.max(0) as u64;
            if !state.announced && record.Usn >= live {
                state.announced = true;
                self.emitter.emit(StreamEvent::HistoryCaughtUp { event_id });
            }
            // The journal writes one record when a change happens and another
            // when the handle closes, carrying every reason accumulated since
            // it opened. Reporting both would double every write.
            let reason = state
                .session
                .fresh(record.FileReferenceNumber, record.Reason);
            if reason & WATCHED == 0 {
                // Every close on the volume arrives here. Nothing new in it
                // for this watch means nothing worth opening a handle over.
                continue;
            }
            let Some(name) = name_of(bytes, &record) else {
                // Legacy UniFFI changes use String, and an NTFS name is not
                // required to be valid UTF-16. A lossy spelling would name a
                // different file, so report the gap instead.
                self.emitter.emit(StreamEvent::RequiresRescan { event_id });
                continue;
            };
            if reason & (USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME) != 0 {
                // A rename moves whole subtrees, so any path remembered below
                // is now the old spelling.
                state.names.forget();
            }
            let Some(path) =
                state
                    .names
                    .path(&self.journal, record.ParentFileReferenceNumber, &name)
            else {
                // The parent is gone too, which is what a tree being deleted
                // looks like: the change is real and unnameable.
                if !state.names.reported {
                    state.names.reported = true;
                    self.emitter.emit(StreamEvent::RequiresRescan { event_id });
                }
                continue;
            };
            // The new name comes first: the closing record of a rename repeats
            // the reason bits, and taking that for another old name would
            // leave a half nothing can pair.
            if reason & USN_REASON_RENAME_NEW_NAME != 0 {
                let Some(index) = state
                    .pending
                    .iter()
                    .position(|(frn, _)| *frn == record.FileReferenceNumber)
                else {
                    continue;
                };
                let (_, from) = state.pending.remove(index);
                let inside = |path: &str| crate::paths::is_inside(&self.emitter.root, path);
                // A move out of the root is a departure the host still has to
                // see and one in is an arrival; a move that touches this root
                // nowhere is not ours to report.
                match (inside(&from), inside(&path)) {
                    (_, true) => self.emitter.change(
                        ChangeKind::Renamed {
                            previous_path: Some(from),
                        },
                        path,
                        event_id,
                    ),
                    (true, false) => self.emitter.change(ChangeKind::Deleted, from, event_id),
                    (false, false) => {}
                }
                continue;
            }
            if reason & USN_REASON_RENAME_OLD_NAME != 0 {
                if state.pending.len() == MAX_PENDING {
                    state.pending.clear();
                    self.emitter.emit(StreamEvent::RequiresRescan { event_id });
                }
                state.pending.push((record.FileReferenceNumber, path));
                continue;
            }
            if !crate::paths::is_inside(&self.emitter.root, &path) {
                // One handle covers the whole volume: everything written
                // outside this root arrives here and stops here.
                continue;
            }
            for kind in kinds(reason) {
                self.emitter.change(kind, path.clone(), event_id);
            }
        }
    }
}

/// The name in a record: UTF-16, not NUL-terminated, and not aligned.
fn name_of(record: &[u8], header: &USN_RECORD_V2) -> Option<String> {
    let at = header.FileNameOffset as usize;
    let bytes = record.get(at..at + header.FileNameLength as usize)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_ne_bytes(*pair))
        .collect();
    String::from_utf16(&units).ok()
}

fn kinds(reason: u32) -> Vec<ChangeKind> {
    let mut kinds = Vec::new();
    if reason & USN_REASON_FILE_CREATE != 0 {
        kinds.push(ChangeKind::Created);
    }
    if reason & USN_REASON_FILE_DELETE != 0 {
        kinds.push(ChangeKind::Deleted);
    }
    if reason
        & (USN_REASON_DATA_OVERWRITE
            | USN_REASON_DATA_EXTEND
            | USN_REASON_DATA_TRUNCATION
            | USN_REASON_BASIC_INFO_CHANGE)
        != 0
    {
        kinds.push(ChangeKind::Modified);
    }
    kinds
}

/// Which reasons one file's open session has already reported.
///
/// A record's reason field is cumulative within a session, so creating,
/// writing and closing a file arrives as `CREATE`, then `CREATE|EXTEND`, then
/// `CREATE|EXTEND|CLOSE`. Only the bits that are new are a change. The close
/// bit ends the session, so the next open reports its own writes again.
// ponytail: one slot, not a map. Two writers interleaving cost a duplicate
// row, which attribution already folds together; the burst is what costs.
#[derive(Default)]
struct Session {
    frn: u64,
    reported: u32,
}

impl Session {
    fn fresh(&mut self, frn: u64, reason: u32) -> u32 {
        if self.frn != frn {
            self.frn = frn;
            self.reported = 0;
        }
        let fresh = reason & !self.reported;
        self.reported |= reason;
        if reason & USN_REASON_CLOSE != 0 {
            self.frn = 0;
            self.reported = 0;
        }
        fresh
    }
}

/// The last directory resolved. A burst of records shares a parent, and
/// resolving one costs a handle open plus a path query — on every change made
/// anywhere on the volume, which is what "monitoring stays cheap" is about.
// ponytail: one slot, dropped on any rename rather than a cache with
// eviction; a stale spelling would be worse than a slow one.
#[derive(Default)]
struct Directory {
    frn: u64,
    path: Option<String>,
    /// Latched: a tree being deleted makes every record in it unnameable, and
    /// one gap per record would bury the report it already made.
    reported: bool,
}

impl Directory {
    fn forget(&mut self) {
        self.frn = 0;
        self.path = None;
    }

    fn path(&mut self, journal: &Journal, parent: u64, name: &str) -> Option<String> {
        if self.frn != parent || self.path.is_none() {
            self.frn = parent;
            self.path = journal.directory(parent);
        }
        let directory = self.path.as_deref()?;
        self.reported = false;
        Some(crate::paths::normalize(&format!(
            r"{}\{name}",
            directory.trim_end_matches('\\')
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_record_does_not_repeat_the_write_it_closes() {
        let mut session = Session::default();
        assert_eq!(
            session.fresh(7, USN_REASON_FILE_CREATE),
            USN_REASON_FILE_CREATE
        );
        assert_eq!(
            session.fresh(7, USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND),
            USN_REASON_DATA_EXTEND
        );
        assert_eq!(
            session.fresh(
                7,
                USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND | USN_REASON_CLOSE
            ),
            USN_REASON_CLOSE
        );
        // A second open of the same file reports its own writes again.
        assert_eq!(
            session.fresh(7, USN_REASON_DATA_EXTEND),
            USN_REASON_DATA_EXTEND
        );
    }

    #[test]
    fn a_reason_this_watch_never_asked_for_is_not_a_change() {
        assert_eq!(kinds(USN_REASON_CLOSE), vec![]);
        assert_eq!(kinds(USN_REASON_FILE_DELETE), vec![ChangeKind::Deleted]);
        assert_eq!(
            kinds(USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND),
            vec![ChangeKind::Created, ChangeKind::Modified]
        );
    }

    #[test]
    fn only_a_local_disk_has_a_journal_to_read() {
        assert_eq!(disk_of(r"C:\Users\x"), Some('C'));
        assert_eq!(disk_of("D:/watched"), Some('D'));
        assert_eq!(disk_of(r"\\server\share\watched"), None);
    }
}
