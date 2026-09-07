//! macOS backend: FSEvents via a minimal hand-written FFI surface, so event IDs
//! are the kernel's own and `since_event_id` resumes across relaunches.
//! Flag mapping mirrors Swift's `FSEventsChangeMapper`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use crate::monitor::{Backend, ChangeKind, Emitter, StreamEvent};
use crate::CoreError;

type CFIndex = isize;
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFTimeInterval = f64;
type FSEventStreamRef = *mut c_void;
type FSEventStreamEventId = u64;
type FSEventStreamEventFlags = u32;
type FSEventStreamCreateFlags = u32;
type DispatchQueue = *mut c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_FSEVENT_STREAM_EVENT_ID_SINCE_NOW: FSEventStreamEventId = u64::MAX;
const FLAG_CREATE_NO_DEFER: FSEventStreamCreateFlags = 0x02;
const FLAG_CREATE_FILE_EVENTS: FSEventStreamCreateFlags = 0x10;

const EVENT_MUST_SCAN_SUB_DIRS: u32 = 0x0001;
const EVENT_USER_DROPPED: u32 = 0x0002;
const EVENT_KERNEL_DROPPED: u32 = 0x0004;
const EVENT_IDS_WRAPPED: u32 = 0x0008;
const EVENT_HISTORY_DONE: u32 = 0x0010;
const EVENT_ROOT_CHANGED: u32 = 0x0020;
const EVENT_MOUNT: u32 = 0x0040;
const EVENT_UNMOUNT: u32 = 0x0080;
const EVENT_ITEM_CREATED: u32 = 0x0100;
const EVENT_ITEM_REMOVED: u32 = 0x0200;
const EVENT_ITEM_RENAMED: u32 = 0x0800;
const RESCAN_FLAGS: u32 = EVENT_MUST_SCAN_SUB_DIRS
    | EVENT_USER_DROPPED
    | EVENT_KERNEL_DROPPED
    | EVENT_IDS_WRAPPED
    | EVENT_ROOT_CHANGED
    | EVENT_MOUNT
    | EVENT_UNMOUNT;

type RetainCallback = unsafe extern "C" fn(*const c_void) -> *const c_void;
type ReleaseCallback = unsafe extern "C" fn(*const c_void);
type StreamCallback = unsafe extern "C" fn(
    FSEventStreamRef,
    *mut c_void,
    usize,
    *mut c_void,
    *const FSEventStreamEventFlags,
    *const FSEventStreamEventId,
);

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<RetainCallback>,
    release: Option<ReleaseCallback>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
}

#[repr(C)]
struct CFArrayCallBacks {
    version: CFIndex,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: StreamCallback,
        context: *const FSEventStreamContext,
        paths_to_watch: CFArrayRef,
        since_when: FSEventStreamEventId,
        latency: CFTimeInterval,
        flags: FSEventStreamCreateFlags,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: DispatchQueue);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    #[allow(non_upper_case_globals)]
    static kCFTypeArrayCallBacks: CFArrayCallBacks;
    fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        count: CFIndex,
        callbacks: *const CFArrayCallBacks,
    ) -> CFArrayRef;
    fn CFRelease(cf: CFTypeRef);
}

extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> DispatchQueue;
    fn dispatch_release(object: *mut c_void);
}

struct FsEventsBackend {
    stream: FSEventStreamRef,
    queue: DispatchQueue,
}

// The stream is only touched from `stop`, which FSEvents allows from any thread.
unsafe impl Send for FsEventsBackend {}

impl Backend for FsEventsBackend {
    fn stop(&mut self) {
        if self.stream.is_null() {
            return;
        }
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            dispatch_release(self.queue);
        }
        self.stream = ptr::null_mut();
        self.queue = ptr::null_mut();
    }
}

impl Drop for FsEventsBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn start(
    emitter: Arc<Emitter>,
    since_event_id: Option<u64>,
    latency: Duration,
) -> Result<Box<dyn Backend>, CoreError> {
    let root = CString::new(emitter.root.as_str()).map_err(|_| CoreError::Watch {
        message: "root path contains a NUL byte".into(),
    })?;
    let label = CString::new(format!(
        "app.pathlight.core.fsevents.{}",
        std::process::id()
    ))
    .expect("no NUL");

    let latency_secs = latency.as_secs_f64();
    // NoDefer fires the first event immediately for interactive live monitors;
    // background watches let the kernel batch the whole latency window.
    let mut flags = FLAG_CREATE_FILE_EVENTS;
    if latency_secs < 1.0 {
        flags |= FLAG_CREATE_NO_DEFER;
    }

    let context = FSEventStreamContext {
        version: 0,
        info: Arc::as_ptr(&emitter) as *mut c_void,
        retain: Some(retain_emitter),
        release: Some(release_emitter),
        copy_description: None,
    };

    let stream = unsafe {
        let path = CFStringCreateWithCString(ptr::null(), root.as_ptr(), K_CF_STRING_ENCODING_UTF8);
        if path.is_null() {
            return Err(CoreError::Watch {
                message: "could not create CFString for root".into(),
            });
        }
        let paths = CFArrayCreate(ptr::null(), &path, 1, &kCFTypeArrayCallBacks);
        let stream = FSEventStreamCreate(
            ptr::null(),
            stream_callback,
            &context,
            paths,
            since_event_id.unwrap_or(K_FSEVENT_STREAM_EVENT_ID_SINCE_NOW),
            latency_secs,
            flags,
        );
        CFRelease(paths);
        CFRelease(path);
        stream
    };
    if stream.is_null() {
        return Err(CoreError::Watch {
            message: "FSEventStreamCreate failed".into(),
        });
    }

    let queue = unsafe { dispatch_queue_create(label.as_ptr(), ptr::null()) };
    let mut backend = FsEventsBackend { stream, queue };
    let started = unsafe {
        FSEventStreamSetDispatchQueue(stream, queue);
        FSEventStreamStart(stream) != 0
    };
    if !started {
        backend.stop();
        return Err(CoreError::Watch {
            message: "FSEventStreamStart failed".into(),
        });
    }

    // With SinceNow there is no history to replay, so FSEvents never sends
    // HistoryDone; the host still wants the live marker.
    if since_event_id.is_none() {
        emitter.emit(StreamEvent::HistoryCaughtUp { event_id: 0 });
    }
    Ok(Box::new(backend))
}

unsafe extern "C" fn retain_emitter(info: *const c_void) -> *const c_void {
    if !info.is_null() {
        Arc::increment_strong_count(info as *const Emitter);
    }
    info
}

unsafe extern "C" fn release_emitter(info: *const c_void) {
    if !info.is_null() {
        Arc::decrement_strong_count(info as *const Emitter);
    }
}

unsafe extern "C" fn stream_callback(
    _stream: FSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const FSEventStreamEventFlags,
    ids: *const FSEventStreamEventId,
) {
    if info.is_null() {
        return;
    }
    let emitter = &*(info as *const Emitter);
    let paths = paths as *const *const c_char;
    for index in 0..count {
        let path_ptr = *paths.add(index);
        if path_ptr.is_null() {
            continue;
        }
        let path = CStr::from_ptr(path_ptr).to_string_lossy().into_owned();
        let flag = *flags.add(index);
        let event_id = *ids.add(index);
        dispatch(emitter, flag, path, event_id);
    }
}

fn dispatch(emitter: &Emitter, flags: u32, path: String, event_id: u64) {
    if flags & EVENT_HISTORY_DONE != 0 {
        return emitter.emit(StreamEvent::HistoryCaughtUp { event_id });
    }
    if flags & RESCAN_FLAGS != 0 {
        return emitter.emit(StreamEvent::RequiresRescan { event_id });
    }
    let kind = if flags & EVENT_ITEM_REMOVED != 0 {
        ChangeKind::Deleted
    } else if flags & EVENT_ITEM_RENAMED != 0 {
        ChangeKind::Renamed {
            previous_path: None,
        }
    } else if flags & EVENT_ITEM_CREATED != 0 {
        ChangeKind::Created
    } else {
        ChangeKind::Modified
    };
    emitter.change(kind, path.trim_end_matches('/').to_owned(), event_id);
}
