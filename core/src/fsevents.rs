//! macOS backend: FSEvents via a minimal hand-written FFI surface, so event IDs
//! are the kernel's own and `since_event_id` resumes across relaunches.
//! Flag mapping mirrors Swift's `FSEventsChangeMapper`.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::monitor::{Backend, ChangeKind, Emitter, StreamEvent};
use crate::CoreError;

type CFIndex = isize;
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFNumberRef = *const c_void;
type CFTimeInterval = f64;
type FSEventStreamRef = *mut c_void;
type FSEventStreamEventId = u64;
type FSEventStreamEventFlags = u32;
type FSEventStreamCreateFlags = u32;
type DispatchQueue = *mut c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;
const K_FSEVENT_STREAM_EVENT_ID_SINCE_NOW: FSEventStreamEventId = u64::MAX;
const FLAG_CREATE_USE_CF_TYPES: FSEventStreamCreateFlags = 0x01;
const FLAG_CREATE_NO_DEFER: FSEventStreamCreateFlags = 0x02;
const FLAG_CREATE_FILE_EVENTS: FSEventStreamCreateFlags = 0x10;
const FLAG_CREATE_USE_EXTENDED_DATA: FSEventStreamCreateFlags = 0x40;
/// How long a rename departure waits for its arrival in a later batch.
const RENAME_PAIRING_WINDOW: Duration = Duration::from_secs(2);
const PATH_BUFFER_LEN: usize = 4096;

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
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> u8;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, number_type: CFIndex, value: *mut c_void) -> u8;
}

extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> DispatchQueue;
    fn dispatch_release(object: *mut c_void);
}

/// Per-stream state handed to FSEvents as the context `info` pointer.
struct FsState {
    emitter: Arc<Emitter>,
    /// inode -> (departed path, when) for renames whose arrival has not shown up yet.
    pending_renames: Mutex<HashMap<u64, (String, Instant)>>,
}

/// Dictionary keys used by `kFSEventStreamCreateFlagUseExtendedData`.
struct Keys {
    path: CFStringRef,
    file_id: CFStringRef,
}

// Immutable CFStrings are safe to read from any thread.
unsafe impl Send for Keys {}
unsafe impl Sync for Keys {}

fn keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| unsafe {
        Keys {
            path: CFStringCreateWithCString(
                ptr::null(),
                c"path".as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            ),
            file_id: CFStringCreateWithCString(
                ptr::null(),
                c"fileID".as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            ),
        }
    })
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
    // Extended data carries the inode, which is what pairs the two halves of a rename.
    let mut flags =
        FLAG_CREATE_FILE_EVENTS | FLAG_CREATE_USE_CF_TYPES | FLAG_CREATE_USE_EXTENDED_DATA;
    if latency_secs < 1.0 {
        flags |= FLAG_CREATE_NO_DEFER;
    }

    let state = Arc::new(FsState {
        emitter: Arc::clone(&emitter),
        pending_renames: Mutex::new(HashMap::new()),
    });
    let context = FSEventStreamContext {
        version: 0,
        info: Arc::as_ptr(&state) as *mut c_void,
        retain: Some(retain_state),
        release: Some(release_state),
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

unsafe extern "C" fn retain_state(info: *const c_void) -> *const c_void {
    if !info.is_null() {
        Arc::increment_strong_count(info as *const FsState);
    }
    info
}

unsafe extern "C" fn release_state(info: *const c_void) {
    if !info.is_null() {
        Arc::decrement_strong_count(info as *const FsState);
    }
}

struct RawEvent {
    path: String,
    flags: u32,
    event_id: u64,
    inode: Option<u64>,
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
    let state = &*(info as *const FsState);
    let keys = keys();
    let mut batch = Vec::with_capacity(count);
    for index in 0..count {
        let dictionary = CFArrayGetValueAtIndex(paths as CFArrayRef, index as CFIndex);
        if dictionary.is_null() {
            continue;
        }
        let Some(path) = dictionary_string(dictionary, keys.path) else {
            continue;
        };
        batch.push(RawEvent {
            path: path.trim_end_matches('/').to_owned(),
            flags: *flags.add(index),
            event_id: *ids.add(index),
            inode: dictionary_u64(dictionary, keys.file_id),
        });
    }
    state.handle(batch);
}

unsafe fn dictionary_string(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    let value = CFDictionaryGetValue(dictionary, key);
    if value.is_null() {
        return None;
    }
    let mut buffer = vec![0 as c_char; PATH_BUFFER_LEN];
    if CFStringGetCString(
        value,
        buffer.as_mut_ptr(),
        PATH_BUFFER_LEN as CFIndex,
        K_CF_STRING_ENCODING_UTF8,
    ) == 0
    {
        return None;
    }
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

unsafe fn dictionary_u64(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<u64> {
    let value = CFDictionaryGetValue(dictionary, key);
    if value.is_null() {
        return None;
    }
    let mut number: i64 = 0;
    if CFNumberGetValue(
        value,
        K_CF_NUMBER_SINT64_TYPE,
        &mut number as *mut i64 as *mut c_void,
    ) == 0
    {
        return None;
    }
    Some(number as u64)
}

impl FsState {
    /// Emits one batch in delivery order. Renames are paired by inode: the
    /// departure (path gone) is emitted as-is, and the arrival (path present)
    /// carries `previous_path` when its departure was seen in this batch or
    /// within `RENAME_PAIRING_WINDOW` before it.
    fn handle(&self, batch: Vec<RawEvent>) {
        let now = Instant::now();
        let mut departures: HashMap<u64, String> = HashMap::new();
        for event in &batch {
            if let (true, Some(inode)) = (
                is_rename(event.flags) && !Path::new(&event.path).exists(),
                event.inode,
            ) {
                departures
                    .entry(inode)
                    .or_insert_with(|| event.path.clone());
            }
        }

        let mut pending = self
            .pending_renames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, (_, seen)| now.duration_since(*seen) < RENAME_PAIRING_WINDOW);

        for event in batch {
            if event.flags & EVENT_HISTORY_DONE != 0 {
                self.emitter.emit(StreamEvent::HistoryCaughtUp {
                    event_id: event.event_id,
                });
                continue;
            }
            if event.flags & RESCAN_FLAGS != 0 {
                self.emitter.emit(StreamEvent::RequiresRescan {
                    event_id: event.event_id,
                });
                continue;
            }
            let kind = if event.flags & EVENT_ITEM_REMOVED != 0 {
                ChangeKind::Deleted
            } else if is_rename(event.flags) {
                let exists = Path::new(&event.path).exists();
                match (exists, event.inode) {
                    (true, Some(inode)) => ChangeKind::Renamed {
                        previous_path: departures
                            .get(&inode)
                            .filter(|departed| **departed != event.path)
                            .cloned()
                            .or_else(|| pending.remove(&inode).map(|(path, _)| path)),
                    },
                    (false, Some(inode)) => {
                        pending.insert(inode, (event.path.clone(), now));
                        ChangeKind::Renamed {
                            previous_path: None,
                        }
                    }
                    _ => ChangeKind::Renamed {
                        previous_path: None,
                    },
                }
            } else if event.flags & EVENT_ITEM_CREATED != 0 {
                ChangeKind::Created
            } else {
                ChangeKind::Modified
            };
            self.emitter.change(kind, event.path, event.event_id);
        }
    }
}

fn is_rename(flags: u32) -> bool {
    flags & EVENT_ITEM_RENAMED != 0
}
