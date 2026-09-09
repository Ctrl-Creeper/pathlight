//! fanotify: the Linux watcher that needs privilege and is worth it.
//!
//! inotify costs one kernel watch per directory, so a deep tree can exhaust
//! `fs.inotify.max_user_watches`, and every subdirectory created between the
//! scan and the watch is a window where changes go unseen. One
//! `FAN_MARK_FILESYSTEM` mark covers a whole filesystem for one descriptor, with
//! no per-directory bookkeeping and no window — and it names the process that
//! caused each event, which nothing unprivileged on Linux can do.
//!
//! The price is `CAP_SYS_ADMIN` (in practice: root) and events for the entire
//! filesystem, which this filters down to the watched root. Availability is
//! probed, never inferred from a kernel version: distributions backport, and a
//! container can drop the capability from a new kernel.

use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::mem;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::monitor::{Backend, Capabilities, ChangeKind, Emitter, StreamEvent};
use crate::notify_backend::RootIdentity;
use crate::CoreError;

/// fanotify hands out no cursor, drops events when its queue fills, and is the
/// only Linux source that names a process. Whether it pairs renames is the
/// kernel's answer, not ours.
pub(crate) fn capabilities() -> Option<Capabilities> {
    if !requested() {
        return None;
    }
    Some(Capabilities {
        resumable_cursor: false,
        pairs_renames: profile()? & FAN_RENAME != 0,
        reports_process: true,
        may_drop_events: true,
    })
}

const FAN_CLOEXEC: c_uint = 0x0000_0001;
const FAN_CLASS_NOTIF: c_uint = 0x0000_0000;
const FAN_REPORT_FID: c_uint = 0x0000_0200;
const FAN_REPORT_DIR_FID: c_uint = 0x0000_0400;
const FAN_REPORT_NAME: c_uint = 0x0000_0800;
const FAN_REPORT_DFID_NAME: c_uint = FAN_REPORT_DIR_FID | FAN_REPORT_NAME;

const FAN_MARK_ADD: c_uint = 0x0000_0001;
const FAN_MARK_FILESYSTEM: c_uint = 0x0000_0100;

const FAN_MODIFY: u64 = 0x0000_0002;
const FAN_ATTRIB: u64 = 0x0000_0004;
const FAN_MOVED_FROM: u64 = 0x0000_0040;
const FAN_MOVED_TO: u64 = 0x0000_0080;
const FAN_CREATE: u64 = 0x0000_0100;
const FAN_DELETE: u64 = 0x0000_0200;
const FAN_Q_OVERFLOW: u64 = 0x0000_4000;
const FAN_RENAME: u64 = 0x1000_0000;
const FAN_ONDIR: u64 = 0x4000_0000;
const WATCHED: u64 = FAN_MODIFY | FAN_ATTRIB | FAN_CREATE | FAN_DELETE | FAN_ONDIR;
/// Linux 5.17 reports both sides of a rename in one event. Older kernels only
/// have the two halves, which nothing on the fanotify side can pair: there is
/// no cookie. Asking for both would report every rename twice.
const HALVES: u64 = FAN_MOVED_FROM | FAN_MOVED_TO;

const FAN_EVENT_INFO_TYPE_FID: u8 = 1;
const FAN_EVENT_INFO_TYPE_DFID_NAME: u8 = 2;
const FAN_EVENT_INFO_TYPE_DFID: u8 = 3;
const FAN_EVENT_INFO_TYPE_OLD_DFID_NAME: u8 = 10;
const FAN_EVENT_INFO_TYPE_NEW_DFID_NAME: u8 = 12;

const AT_FDCWD: c_int = -100;
const O_RDONLY: c_int = 0;
const O_DIRECTORY: c_int = 0x0001_0000;
const O_CLOEXEC: c_int = 0x0008_0000;
const O_PATH: c_int = 0x0020_0000;
const POLLIN: i16 = 0x0001;

// The three layouts below are the kernel's, so every field has to be here
// whether this code reads it or not.
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy)]
struct Metadata {
    event_len: u32,
    vers: u8,
    reserved: u8,
    metadata_len: u16,
    mask: u64,
    fd: i32,
    pid: i32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy)]
struct InfoHeader {
    info_type: u8,
    pad: u8,
    len: u16,
}

#[allow(dead_code)]
#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

extern "C" {
    fn fanotify_init(flags: c_uint, event_f_flags: c_uint) -> c_int;
    fn fanotify_mark(
        fd: c_int,
        flags: c_uint,
        mask: u64,
        dirfd: c_int,
        path: *const c_char,
    ) -> c_int;
    fn open_by_handle_at(mount_fd: c_int, handle: *mut c_void, flags: c_int) -> c_int;
    fn open(path: *const c_char, flags: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn pipe(fds: *mut c_int) -> c_int;
    fn poll(fds: *mut PollFd, nfds: c_uint, timeout: c_int) -> c_int;
}

/// An owned descriptor. Small enough to beat pulling in a dependency for it,
/// and every early return below would otherwise leak one.
struct Fd(c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { close(self.0) };
        }
    }
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

fn group() -> std::io::Result<Fd> {
    let fd = unsafe {
        fanotify_init(
            FAN_CLASS_NOTIF | FAN_REPORT_FID | FAN_REPORT_DFID_NAME | FAN_CLOEXEC,
            (O_RDONLY | O_CLOEXEC) as c_uint,
        )
    };
    if fd < 0 {
        return Err(last_error());
    }
    Ok(Fd(fd))
}

fn mark(group: &Fd, root: &Path, mask: u64) -> std::io::Result<()> {
    let path = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let marked = unsafe {
        fanotify_mark(
            group.0,
            FAN_MARK_ADD | FAN_MARK_FILESYSTEM,
            mask,
            AT_FDCWD,
            path.as_ptr(),
        )
    };
    if marked < 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Whether this process can actually run a fanotify watch here.
///
/// Both halves are tried, because they fail for different reasons: since Linux
/// 5.13 an unprivileged process may open a `FAN_REPORT_FID` group, and only
/// `FAN_MARK_FILESYSTEM` then fails with `EPERM`. A check that stops after the
/// first syscall would promise a backend that cannot start. The probe marks
/// `/`, which closing the group immediately undoes.
pub(crate) fn available() -> bool {
    requested() && profile().is_some()
}

/// Whether this run asked for the privileged watch.
///
/// PRIVILEGED_BACKENDS.md gates defaulting a root to a privileged source on
/// power and accuracy measurements that do not exist yet, and one filesystem
/// mark wakes this process for writes anywhere on the volume. "Monitoring
/// stays cheap" is a promise, so the backend that could break it is asked for
/// rather than assumed — by the privileged entry point, which is the only
/// place it can work at all.
fn requested() -> bool {
    std::env::var_os("PATHLIGHT_PRIVILEGED_WATCH").is_some_and(|value| value == "1")
}

/// The event mask this kernel accepts, or `None` when fanotify is out of
/// reach. `FAN_RENAME` is the difference between one rename and two loose
/// halves, and an older kernel rejects the whole mark rather than the one bit,
/// so it is asked for first and dropped if refused.
fn profile() -> Option<u64> {
    // Probed once per process. A host asks what its watcher promises while
    // drawing a label, and two syscalls per frame is not the price of a label.
    // ponytail: no re-probe. Nothing in the product gains or drops
    // CAP_SYS_ADMIN mid-run; a helper that could would call this on start.
    static PROFILE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *PROFILE.get_or_init(probe)
}

fn probe() -> Option<u64> {
    let group = group().ok()?;
    [WATCHED | FAN_RENAME, WATCHED | HALVES]
        .into_iter()
        .find(|mask| mark(&group, Path::new("/"), *mask).is_ok())
}

pub(crate) fn start(
    emitter: Arc<Emitter>,
    since_event_id: Option<u64>,
    _latency: Duration,
) -> Result<Box<dyn Backend>, CoreError> {
    let mask = profile().ok_or_else(|| CoreError::Watch {
        message: "fanotify is not available to this process".to_owned(),
    })?;
    let group = group()?;
    mark(&group, Path::new(&emitter.root), mask)?;
    // Any descriptor on the same filesystem resolves the handles the kernel
    // reports; the root is the one we already know is there.
    let root_path = CString::new(emitter.root.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mount = Fd(unsafe { open(root_path.as_ptr(), O_RDONLY | O_DIRECTORY | O_CLOEXEC) });
    if mount.0 < 0 {
        return Err(last_error().into());
    }
    let mut pipe_fds = [-1 as c_int; 2];
    if unsafe { pipe(pipe_fds.as_mut_ptr()) } < 0 {
        return Err(last_error().into());
    }
    let (read_end, write_end) = (Fd(pipe_fds[0]), Fd(pipe_fds[1]));
    let stopping = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stopping);
    let worker = thread::Builder::new()
        .name("pathlight-fanotify".into())
        .spawn(move || {
            let watch = Watch {
                group,
                mount,
                wake: read_end,
                emitter,
                stopping: flag,
            };
            watch.run(since_event_id);
        })?;
    Ok(Box::new(FanotifyBackend {
        stopping,
        wake: Some(write_end),
        worker: Some(worker),
    }))
}

struct FanotifyBackend {
    stopping: Arc<AtomicBool>,
    wake: Option<Fd>,
    worker: Option<JoinHandle<()>>,
}

impl Backend for FanotifyBackend {
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(wake) = &self.wake {
            // The worker is parked in poll(); a byte is what ends that wait.
            unsafe { write(wake.0, [0u8].as_ptr() as *const c_void, 1) };
        }
        if let Some(worker) = self.worker.take() {
            // A listener is allowed to stop its own watch.
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
        self.wake.take();
    }
}

impl Drop for FanotifyBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Watch {
    group: Fd,
    mount: Fd,
    wake: Fd,
    emitter: Arc<Emitter>,
    stopping: Arc<AtomicBool>,
}

impl Watch {
    fn run(&self, since_event_id: Option<u64>) {
        if since_event_id.is_some() {
            self.emitter
                .emit(StreamEvent::RequiresRescan { event_id: 0 });
        }
        self.emitter
            .emit(StreamEvent::HistoryCaughtUp { event_id: 0 });
        let mut root = RootIdentity::of(&self.emitter.root);
        let mut event_id = 0u64;
        let mut process = Process::default();
        let mut buffer = vec![0u8; 64 * 1024];
        while !self.stopping.load(Ordering::Acquire) {
            if !self.readable() {
                break;
            }
            if self.stopping.load(Ordering::Acquire) {
                break;
            }
            let read = unsafe {
                read(
                    self.group.0,
                    buffer.as_mut_ptr() as *mut c_void,
                    buffer.len(),
                )
            };
            if read <= 0 {
                let error = last_error();
                if read < 0 && error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if read < 0 {
                    // A broken group is a silent watch, which is the one thing
                    // it must never become without saying so.
                    event_id += 1;
                    self.emitter.emit(StreamEvent::RequiresRescan { event_id });
                }
                break;
            }
            if root.drifted() {
                event_id += 1;
                self.emitter.emit(StreamEvent::RequiresRescan { event_id });
            }
            self.publish(&buffer[..read as usize], &mut event_id, &mut process);
        }
    }

    /// Blocks until the group has something or the stop pipe says to quit. No
    /// timeout: a monitor that wakes on a timer is a monitor that costs
    /// battery while nothing happens.
    fn readable(&self) -> bool {
        let mut fds = [
            PollFd {
                fd: self.group.0,
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: self.wake.0,
                events: POLLIN,
                revents: 0,
            },
        ];
        loop {
            let ready = unsafe { poll(fds.as_mut_ptr(), 2, -1) };
            if ready < 0 {
                if last_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
            if fds[1].revents & POLLIN != 0 {
                return false;
            }
            if fds[0].revents & POLLIN != 0 {
                return true;
            }
        }
    }

    fn publish(&self, buffer: &[u8], event_id: &mut u64, process: &mut Process) {
        let mut offset = 0;
        while offset + mem::size_of::<Metadata>() <= buffer.len() {
            let meta: Metadata =
                unsafe { ptr::read_unaligned(buffer[offset..].as_ptr() as *const Metadata) };
            let len = meta.event_len as usize;
            if len < mem::size_of::<Metadata>() || offset + len > buffer.len() {
                break;
            }
            let event = &buffer[offset..offset + len];
            offset += len;
            if meta.mask & FAN_Q_OVERFLOW != 0 {
                *event_id += 1;
                self.emitter.emit(StreamEvent::RequiresRescan {
                    event_id: *event_id,
                });
                continue;
            }
            if meta.mask & FAN_RENAME != 0 {
                self.publish_rename(
                    event,
                    meta.metadata_len as usize,
                    meta.pid,
                    event_id,
                    process,
                );
                continue;
            }
            let Some(path) = self.path_of(
                event,
                meta.metadata_len as usize,
                meta.mask & FAN_ONDIR != 0,
            ) else {
                // The kernel named a file this process could not resolve. That
                // is a change we cannot report, so report the gap instead.
                *event_id += 1;
                self.emitter.emit(StreamEvent::RequiresRescan {
                    event_id: *event_id,
                });
                continue;
            };
            if !self.inside(&path) {
                // One mark covers the whole filesystem: everything written
                // outside this root arrives here and stops here.
                continue;
            }
            // Legacy UniFFI changes use String, and a lossy conversion would
            // name a different file. Report the gap instead, exactly as the
            // notify backend does.
            let Some(spelled) = path.to_str() else {
                *event_id += 1;
                self.emitter.emit(StreamEvent::RequiresRescan {
                    event_id: *event_id,
                });
                continue;
            };
            let name = process.name(meta.pid);
            let path = spelled.to_owned();
            for kind in kinds(meta.mask) {
                *event_id += 1;
                self.emitter
                    .named_change(kind, path.clone(), name.clone(), *event_id);
            }
        }
    }

    /// A kernel that supports `FAN_RENAME` reports both sides of a move in one
    /// event, as an old and a new directory-handle-plus-name record. That is a
    /// pairing fanotify cannot do afterwards, since it has no rename cookie —
    /// so an event missing either side is a gap, not half a rename.
    fn publish_rename(
        &self,
        event: &[u8],
        metadata_len: usize,
        pid: i32,
        event_id: &mut u64,
        process: &mut Process,
    ) {
        let from = self.side(event, metadata_len, FAN_EVENT_INFO_TYPE_OLD_DFID_NAME);
        let to = self.side(event, metadata_len, FAN_EVENT_INFO_TYPE_NEW_DFID_NAME);
        *event_id += 1;
        let (Some(from), Some(to)) = (from, to) else {
            return self.emitter.emit(StreamEvent::RequiresRescan {
                event_id: *event_id,
            });
        };
        // A move out of the root is a departure the host still has to see, and
        // one in is an arrival; only a move that touches this root at all is
        // ours to report.
        let (inside_from, inside_to) = (self.inside(&from), self.inside(&to));
        let (Some(from), Some(to)) = (from.to_str(), to.to_str()) else {
            return self.emitter.emit(StreamEvent::RequiresRescan {
                event_id: *event_id,
            });
        };
        let name = process.name(pid);
        match (inside_from, inside_to) {
            (_, true) => self.emitter.named_change(
                ChangeKind::Renamed {
                    previous_path: Some(crate::paths::normalize(from)),
                },
                to.to_owned(),
                name,
                *event_id,
            ),
            (true, false) => {
                self.emitter
                    .named_change(ChangeKind::Deleted, from.to_owned(), name, *event_id)
            }
            (false, false) => *event_id -= 1,
        }
    }

    /// One named record of a given type.
    fn side(&self, event: &[u8], metadata_len: usize, wanted: u8) -> Option<PathBuf> {
        let mut at = metadata_len;
        while at + mem::size_of::<InfoHeader>() <= event.len() {
            let header: InfoHeader =
                unsafe { ptr::read_unaligned(event[at..].as_ptr() as *const InfoHeader) };
            let len = header.len as usize;
            if len < mem::size_of::<InfoHeader>() || at + len > event.len() {
                return None;
            }
            if header.info_type == wanted {
                return self.resolve(&event[at..at + len], true);
            }
            at += len;
        }
        None
    }

    /// Which record names the changed object depends on the event: a created
    /// or deleted entry comes as its parent's handle plus a name, an ordinary
    /// write can come as the object's own handle with no name at all. Prefer
    /// the named record — after a delete, the object's own handle no longer
    /// opens — and fall back to the handle.
    fn path_of(&self, event: &[u8], metadata_len: usize, on_directory: bool) -> Option<PathBuf> {
        let mut fallback = None;
        let mut at = metadata_len;
        while at + mem::size_of::<InfoHeader>() <= event.len() {
            let header: InfoHeader =
                unsafe { ptr::read_unaligned(event[at..].as_ptr() as *const InfoHeader) };
            let len = header.len as usize;
            if len < mem::size_of::<InfoHeader>() || at + len > event.len() {
                break;
            }
            let record = &event[at..at + len];
            match header.info_type {
                FAN_EVENT_INFO_TYPE_DFID_NAME => {
                    if let Some(named) = self.resolve(record, true) {
                        return Some(named);
                    }
                }
                FAN_EVENT_INFO_TYPE_FID => {
                    fallback = fallback.or_else(|| self.resolve(record, false))
                }
                // A bare directory handle only names the changed object when
                // the directory *is* the changed object. Otherwise it says
                // "something in here", which is not a path.
                FAN_EVENT_INFO_TYPE_DFID if on_directory => {
                    fallback = fallback.or_else(|| self.resolve(record, false))
                }
                _ => {}
            }
            at += len;
        }
        fallback
    }

    /// `struct fanotify_event_info_fid`: a 4-byte header, an 8-byte fsid, a
    /// `struct file_handle`, and, for the record types that carry one, the
    /// NUL-terminated name right after the handle's bytes.
    fn resolve(&self, record: &[u8], named: bool) -> Option<PathBuf> {
        const HANDLE: usize = 12;
        const FHANDLE: usize = HANDLE + 8;
        let bytes = u32::from_ne_bytes(record.get(HANDLE..HANDLE + 4)?.try_into().ok()?) as usize;
        let name = if named {
            record
                .get(FHANDLE + bytes..)?
                .split(|byte| *byte == 0)
                .next()?
        } else {
            &[]
        };
        // `open_by_handle_at` writes nothing but insists on a mutable, aligned
        // `struct file_handle`; a u32 buffer is how that alignment is promised.
        let mut handle = vec![0u32; (FHANDLE - HANDLE + bytes).div_ceil(4)];
        let handle_bytes = record.get(HANDLE..FHANDLE + bytes)?;
        unsafe {
            ptr::copy_nonoverlapping(
                handle_bytes.as_ptr(),
                handle.as_mut_ptr() as *mut u8,
                handle_bytes.len(),
            );
        }
        let dir = Fd(unsafe {
            open_by_handle_at(self.mount.0, handle.as_mut_ptr() as *mut c_void, O_PATH)
        });
        if dir.0 < 0 {
            return None;
        }
        let at = std::fs::read_link(format!("/proc/self/fd/{}", dir.0)).ok()?;
        if name.is_empty() || name == b"." {
            // The event is about the directory itself.
            return Some(at);
        }
        Some(at.join(std::ffi::OsStr::from_bytes(name)))
    }

    fn inside(&self, path: &Path) -> bool {
        let spelled = crate::paths::normalize(&path.to_string_lossy());
        spelled == self.emitter.root
            || spelled.starts_with(&format!("{}/", self.emitter.root.trim_end_matches('/')))
    }
}

fn kinds(mask: u64) -> Vec<ChangeKind> {
    let mut kinds = Vec::new();
    if mask & FAN_CREATE != 0 {
        kinds.push(ChangeKind::Created);
    }
    if mask & FAN_DELETE != 0 {
        kinds.push(ChangeKind::Deleted);
    }
    // fanotify carries no rename cookie, so a move is two halves that only the
    // host can pair — which is what `pairs_renames: false` tells it to do.
    if mask & (FAN_MOVED_FROM | FAN_MOVED_TO) != 0 {
        kinds.push(ChangeKind::Renamed {
            previous_path: None,
        });
    }
    if mask & (FAN_MODIFY | FAN_ATTRIB) != 0 {
        kinds.push(ChangeKind::Modified);
    }
    kinds
}

/// The last process asked about. A write burst comes from one program, so one
/// slot spares almost every `/proc` read.
// ponytail: no cache with eviction. A map keyed by pid would also have to
// answer for pid reuse, and the burst case is what costs anything.
#[derive(Default)]
struct Process {
    pid: i32,
    name: Option<String>,
}

impl Process {
    fn name(&mut self, pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        if self.pid != pid {
            self.pid = pid;
            self.name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()
                .map(|comm| comm.trim_end().to_owned())
                .filter(|comm| !comm.is_empty());
        }
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_is_reported_as_a_half_the_host_must_pair() {
        assert_eq!(
            kinds(FAN_MOVED_FROM),
            vec![ChangeKind::Renamed {
                previous_path: None
            }]
        );
        assert_eq!(kinds(FAN_CREATE | FAN_ONDIR), vec![ChangeKind::Created]);
        assert_eq!(kinds(FAN_MODIFY | FAN_ATTRIB), vec![ChangeKind::Modified]);
        // A mask this backend never asked for is not turned into a change.
        assert!(kinds(0x20).is_empty());
    }

    #[test]
    fn this_process_can_name_itself() {
        let mut process = Process::default();
        let pid = std::process::id() as i32;
        assert!(process.name(pid).is_some(), "no /proc/{pid}/comm");
        assert_eq!(process.name(-1), None);
    }
}
