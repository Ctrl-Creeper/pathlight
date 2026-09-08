use std::fs::{self, File};
use std::io;
use std::time::SystemTime;

use pathlight_core::measurement::{measure_file, FileKind};

#[test]
fn rename_preserves_object_identity() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("before");
    let renamed = dir.path().join("after");
    fs::write(&original, b"retained object").unwrap();
    let before = measure_file(&original).unwrap();
    fs::rename(&original, &renamed).unwrap();
    let after = measure_file(&renamed).unwrap();

    assert!(
        before.identity.is_some(),
        "local test filesystem must expose identity"
    );
    assert_eq!(before.identity, after.identity);
    assert_eq!(after.logical_bytes, 15);
}

#[test]
fn hard_links_share_identity_but_separate_objects_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let alias = dir.path().join("alias");
    let separate = dir.path().join("separate");
    fs::write(&original, b"same contents").unwrap();
    fs::hard_link(&original, &alias).unwrap();
    fs::write(&separate, b"same contents").unwrap();

    let original = measure_file(&original).unwrap();
    assert!(original.identity.is_some());
    assert_eq!(original.identity, measure_file(&alias).unwrap().identity);
    assert_ne!(original.identity, measure_file(&separate).unwrap().identity);
    assert_eq!(original.link_count, Some(2));
}

#[test]
fn logical_length_is_separate_from_sparse_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse");
    let file = File::create(&path).unwrap();
    #[cfg(windows)]
    set_sparse(&file);
    file.set_len(8 * 1024 * 1024).unwrap();
    file.sync_all().unwrap();
    let before = SystemTime::now();
    let measurement = measure_file(&path).unwrap();

    assert_eq!(measurement.kind, FileKind::File);
    assert_eq!(measurement.logical_bytes, 8 * 1024 * 1024);
    assert!(measurement.allocated_bytes.unwrap() < measurement.logical_bytes);
    assert!(measurement.measured_at >= before);
    assert!(measurement.measured_at <= SystemTime::now());
    assert_eq!(
        pathlight_core::attribution::allocated_size(&path),
        measurement
            .allocated_bytes
            .map(|n| i64::try_from(n).unwrap())
    );
}

#[test]
fn directory_entries_do_not_contribute_file_content_bytes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("child"), vec![1; 16_384]).unwrap();
    let measurement = measure_file(dir.path()).unwrap();

    assert_eq!(measurement.kind, FileKind::Directory);
    assert_eq!(measurement.logical_bytes, 0);
    assert_eq!(measurement.allocated_bytes, Some(0));
}

#[test]
fn missing_paths_are_errors_instead_of_zero_byte_objects() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        measure_file(&dir.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_measured_without_following_the_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    fs::write(&target, vec![1; 16_384]).unwrap();
    symlink("target", &link).unwrap();
    let measurement = measure_file(&link).unwrap();

    assert_eq!(measurement.kind, FileKind::Symlink);
    assert_eq!(measurement.logical_bytes, 6);
    assert_ne!(
        measurement.identity,
        measure_file(&target).unwrap().identity
    );
    fs::remove_file(&target).unwrap();
    assert_eq!(measure_file(&link).unwrap().identity, measurement.identity);
}

#[cfg(windows)]
#[test]
fn windows_symbolic_links_are_measured_without_following_the_target() {
    use std::os::windows::fs::symlink_file;
    use windows_sys::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    fs::write(&target, vec![1; 16_384]).unwrap();
    if let Err(error) = symlink_file("target", &link) {
        if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD as i32) {
            eprintln!("symlink fixture requires Windows Developer Mode or symlink privilege");
            return;
        }
        panic!("symlink fixture: {error}");
    }
    let measurement = measure_file(&link).unwrap();

    assert_eq!(measurement.kind, FileKind::Symlink);
    assert!(measurement.identity.is_some());
    assert_ne!(
        measurement.identity,
        measure_file(&target).unwrap().identity
    );
    fs::remove_file(&target).unwrap();
    assert_eq!(measure_file(&link).unwrap().identity, measurement.identity);
}

#[cfg(windows)]
fn set_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let mut returned = 0;
    // The test runs on a local NTFS/ReFS volume supporting sparse files.
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            null(),
            0,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    };
    assert_ne!(success, 0, "sparse fixture: {}", io::Error::last_os_error());
}
