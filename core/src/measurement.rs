//! Filesystem-reported object measurements, separate from I/O byte counters.
//!
//! Identity links names to an object within an observed filesystem lifetime.
//! IDs can be reused after deletion or a filesystem lifecycle change; callers
//! must not infer continuity across an unknown gap or remount from equality
//! alone. Allocation is not exclusive physical usage: clones and snapshots may
//! share extents. Reads use metadata only and never follow the final symlink.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IdentityPlatform {
    Unix,
    Windows,
}

/// A native object identifier scoped to a volume and its observed lifetime.
/// Unix IDs contain the inode as eight little-endian bytes; Windows IDs retain
/// all 128 native bits. `volume_id` is a device ID or volume serial number, not
/// a globally unique or permanent filesystem identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectIdentity {
    pub platform: IdentityPlatform,
    pub volume_id: u64,
    pub file_id: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeasurement {
    pub identity: Option<ObjectIdentity>,
    pub kind: FileKind,
    /// File length, not the number of bytes written. Directories contribute
    /// zero by this module's file-content accounting convention.
    pub logical_bytes: u64,
    /// Filesystem-reported allocation, never substituted with logical length.
    /// `None` means unsupported or not representable; zero is a known value.
    pub allocated_bytes: Option<u64>,
    pub link_count: Option<u64>,
    /// Receipt time after the metadata reads. This is not a change timestamp
    /// or a claim that the fields were observed atomically.
    pub measured_at: SystemTime,
}

/// Measure the named object without following its final symlink. A missing or
/// unreadable object is an error, not an object with a zero-byte measurement.
/// The path may still have symlink ancestors; this is not a sandbox boundary.
pub fn measure_file(path: &Path) -> io::Result<FileMeasurement> {
    platform::measure_file(path)
}

fn file_kind(file_type: std::fs::FileType) -> FileKind {
    if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub(super) fn measure_file(path: &Path) -> io::Result<FileMeasurement> {
        // One no-follow stat supplies both identity and size, avoiding a
        // separate path lookup that could observe a replacement object.
        let metadata = std::fs::symlink_metadata(path)?;
        let kind = file_kind(metadata.file_type());
        let is_directory = kind == FileKind::Directory;
        Ok(FileMeasurement {
            identity: Some(ObjectIdentity {
                platform: IdentityPlatform::Unix,
                volume_id: metadata.dev(),
                file_id: metadata.ino().to_le_bytes().to_vec(),
            }),
            kind,
            logical_bytes: if is_directory { 0 } else { metadata.len() },
            allocated_bytes: if is_directory {
                Some(0)
            } else {
                metadata.blocks().checked_mul(512)
            },
            link_count: Some(metadata.nlink()),
            measured_at: SystemTime::now(),
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
        ERROR_NOT_SUPPORTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_STANDARD_INFO,
    };

    pub(super) fn measure_file(path: &Path) -> io::Result<FileMeasurement> {
        // A read-attributes handle also works for directories. Share-delete
        // avoids obstructing rename/unlink while observing; opening the reparse
        // point preserves the final symlink's identity instead of its target.
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let metadata = file.metadata()?;
        let kind = file_kind(metadata.file_type());
        let standard = query_info::<FILE_STANDARD_INFO>(&file, FileStandardInfo)?;
        let identity = query_info::<FILE_ID_INFO>(&file, FileIdInfo)?.map(|info| ObjectIdentity {
            platform: IdentityPlatform::Windows,
            volume_id: info.VolumeSerialNumber,
            file_id: info.FileId.Identifier.to_vec(),
        });
        let is_directory = kind == FileKind::Directory;

        Ok(FileMeasurement {
            identity,
            kind,
            logical_bytes: if is_directory {
                0
            } else if let Some(info) = standard.as_ref() {
                u64::try_from(info.EndOfFile).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "negative file length")
                })?
            } else {
                metadata.len()
            },
            allocated_bytes: if is_directory {
                Some(0)
            } else {
                standard
                    .as_ref()
                    .and_then(|info| u64::try_from(info.AllocationSize).ok())
            },
            link_count: standard.map(|info| u64::from(info.NumberOfLinks)),
            measured_at: SystemTime::now(),
        })
    }

    fn query_info<T: Default>(
        file: &File,
        class: FILE_INFO_BY_HANDLE_CLASS,
    ) -> io::Result<Option<T>> {
        let mut info = T::default();
        // The two callers pair each documented information class with its
        // matching repr(C) windows-sys structure. The handle stays open and the
        // initialized output buffer remains alive for the synchronous call.
        let success = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                class,
                (&mut info as *mut T).cast(),
                size_of::<T>() as u32,
            )
        };
        if success != 0 {
            return Ok(Some(info));
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(
                ERROR_INVALID_FUNCTION
                    | ERROR_NOT_SUPPORTED
                    | ERROR_INVALID_PARAMETER
                    | ERROR_CALL_NOT_IMPLEMENTED
            )
        ) {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;

    pub(super) fn measure_file(path: &Path) -> io::Result<FileMeasurement> {
        let metadata = std::fs::symlink_metadata(path)?;
        let kind = file_kind(metadata.file_type());
        let is_directory = kind == FileKind::Directory;
        Ok(FileMeasurement {
            identity: None,
            kind,
            logical_bytes: if is_directory { 0 } else { metadata.len() },
            allocated_bytes: is_directory.then_some(0),
            link_count: None,
            measured_at: SystemTime::now(),
        })
    }
}
