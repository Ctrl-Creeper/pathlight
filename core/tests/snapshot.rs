use std::fs;

#[cfg(any(target_os = "linux", target_os = "android"))]
use pathlight_core::measurement::measure_file;
#[cfg(unix)]
use pathlight_core::measurement::FileKind;
use pathlight_core::snapshot::{
    reconcile, scan, BindingChangeKind, IdentityContinuity, ScanConsistency,
};

#[test]
fn matching_object_numbers_are_not_paired_across_unknown_continuity() {
    let directory = tempfile::tempdir().unwrap();
    let old_path = directory.path().join("old-name");
    let new_path = directory.path().join("new-name");
    fs::write(&old_path, b"old contents").unwrap();
    let before = scan(directory.path()).unwrap();
    fs::rename(&old_path, &new_path).unwrap();
    let after = scan(directory.path()).unwrap();

    let delta = reconcile(&before, &after, IdentityContinuity::Unknown);

    assert!(!delta
        .bindings
        .iter()
        .any(|binding| binding.kind == BindingChangeKind::Renamed));
    assert!(delta
        .bindings
        .iter()
        .any(|binding| binding.kind == BindingChangeKind::Removed && binding.path == old_path));
    assert!(delta
        .bindings
        .iter()
        .any(|binding| binding.kind == BindingChangeKind::Created && binding.path == new_path));
}

#[test]
fn identity_change_without_continuity_cannot_invent_a_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file");
    fs::write(&path, b"before").unwrap();
    let before = scan(directory.path()).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"after").unwrap();
    let after = scan(directory.path()).unwrap();

    let delta = reconcile(&before, &after, IdentityContinuity::Unknown);

    assert!(delta.bindings.is_empty());
    assert!(!delta.historical_complete);
}

#[test]
fn hard_links_keep_both_names_without_double_counting_size() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original");
    let alias = directory.path().join("alias");
    fs::write(&original, b"one shared file").unwrap();
    let before = scan(directory.path()).unwrap();
    fs::hard_link(&original, &alias).unwrap();
    let after = scan(directory.path()).unwrap();

    assert!(after.entries.contains_key(&original));
    assert!(after.entries.contains_key(&alias));
    assert_eq!(after.totals().logical_bytes, Some(15));
    assert_eq!(
        after.totals().allocated_bytes,
        before.totals().allocated_bytes
    );
    let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);
    assert_eq!(delta.logical_delta, Some(0));
    assert!(delta.bindings.iter().any(|binding| {
        binding.kind == BindingChangeKind::HardLinkAdded && binding.path == alias
    }));

    fs::remove_file(&alias).unwrap();
    let after_removal = scan(directory.path()).unwrap();
    let delta = reconcile(
        &after,
        &after_removal,
        IdentityContinuity::ObservedWithoutGap,
    );
    assert_eq!(delta.logical_delta, Some(0));
    assert!(delta.bindings.iter().any(|binding| {
        binding.kind == BindingChangeKind::HardLinkRemoved && binding.path == alias
    }));
}

#[test]
fn rename_changes_a_binding_without_claiming_space_was_reclaimed() {
    let directory = tempfile::tempdir().unwrap();
    let old_name = directory.path().join("before");
    let new_name = directory.path().join("after");
    fs::write(&old_name, b"same bytes").unwrap();
    let before = scan(directory.path()).unwrap();
    fs::rename(&old_name, &new_name).unwrap();
    let after = scan(directory.path()).unwrap();
    let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);

    assert_eq!(delta.logical_delta, Some(0));
    assert_eq!(delta.allocated_delta, Some(0));
    assert_eq!(delta.bindings.len(), 1);
    assert_eq!(delta.bindings[0].kind, BindingChangeKind::Renamed);
    assert_eq!(delta.bindings[0].previous_path, Some(old_name));
    assert_eq!(delta.bindings[0].path, new_name);
    assert!(!delta.historical_complete);
    assert_eq!(after.consistency, ScanConsistency::ObservedInterval);
    assert!(after.started_at <= after.finished_at);
}

#[test]
fn replacement_at_the_same_name_preserves_both_object_identities() {
    let directory = tempfile::tempdir().unwrap();
    let name = directory.path().join("target");
    fs::write(&name, b"old").unwrap();
    let before = scan(directory.path()).unwrap();
    // Keeping the old object alive prevents inode reuse from making this test
    // dependent on the allocator of the filesystem running it.
    let retained = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
    fs::rename(&name, retained.path().join("old-object")).unwrap();
    fs::write(&name, b"replacement").unwrap();
    let after = scan(directory.path()).unwrap();
    let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);

    assert_eq!(delta.logical_delta, Some(8));
    assert_eq!(delta.bindings.len(), 1);
    assert_eq!(delta.bindings[0].kind, BindingChangeKind::Replaced);
    assert_eq!(delta.bindings[0].path, name);
    assert_ne!(
        delta.bindings[0].identity,
        delta.bindings[0].previous_identity
    );
}

#[test]
fn removing_the_last_name_changes_observed_totals_not_physical_reclaim() {
    let directory = tempfile::tempdir().unwrap();
    let name = directory.path().join("removed");
    fs::write(&name, b"removed bytes").unwrap();
    let before = scan(directory.path()).unwrap();
    fs::remove_file(&name).unwrap();
    let after = scan(directory.path()).unwrap();
    let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);

    assert_eq!(delta.logical_delta, Some(-13));
    assert_eq!(delta.bindings.len(), 1);
    assert_eq!(delta.bindings[0].kind, BindingChangeKind::Removed);
    assert_eq!(delta.bindings[0].path, name);
}

#[test]
fn snapshots_from_different_roots_cannot_be_reconciled() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join("file"), b"large file").unwrap();
    let delta = reconcile(
        &scan(left.path()).unwrap(),
        &scan(right.path()).unwrap(),
        IdentityContinuity::ObservedWithoutGap,
    );
    assert_eq!(delta.logical_delta, None);
    assert_eq!(delta.allocated_delta, None);
    assert!(delta.bindings.is_empty());
}

#[test]
fn replacing_the_root_ends_its_identity_scope_even_at_the_same_path() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("old-file"), b"old bytes").unwrap();
    let before = scan(&root).unwrap();
    fs::rename(&root, parent.path().join("old-root")).unwrap();
    fs::create_dir(&root).unwrap();
    let after = scan(&root).unwrap();

    let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);
    assert_eq!(delta.logical_delta, None);
    assert_eq!(delta.allocated_delta, None);
    assert!(delta.bindings.is_empty());
}

#[test]
fn unknown_identity_or_allocation_cannot_be_presented_as_unique_space() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file");
    fs::write(&path, b"known length").unwrap();
    let snapshot = scan(directory.path()).unwrap();

    let mut unsupported_allocation = snapshot.clone();
    unsupported_allocation
        .entries
        .get_mut(&path)
        .unwrap()
        .allocated_bytes = None;
    assert_eq!(unsupported_allocation.totals().logical_bytes, Some(12));
    assert_eq!(unsupported_allocation.totals().allocated_bytes, None);

    let mut unsupported_identity = snapshot.clone();
    unsupported_identity
        .entries
        .get_mut(&path)
        .unwrap()
        .identity = None;
    assert_eq!(unsupported_identity.totals().logical_bytes, None);
    assert_eq!(unsupported_identity.totals().allocated_bytes, None);
    assert_eq!(
        reconcile(
            &snapshot,
            &unsupported_identity,
            IdentityContinuity::ObservedWithoutGap,
        )
        .logical_delta,
        None
    );
}

#[cfg(unix)]
#[test]
fn sparse_files_keep_logical_and_allocated_metrics_separate() {
    let directory = tempfile::tempdir().unwrap();
    let file = fs::File::create(directory.path().join("sparse")).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    let snapshot = scan(directory.path()).unwrap();
    assert_eq!(snapshot.totals().logical_bytes, Some(64 * 1024 * 1024));
    assert!(snapshot.totals().allocated_bytes.unwrap() < 64 * 1024 * 1024);
}

#[cfg(unix)]
#[test]
fn symlink_cycles_and_links_outside_the_root_are_not_traversed() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside-file"), b"outside").unwrap();
    symlink(directory.path(), directory.path().join("cycle")).unwrap();
    symlink(outside.path(), directory.path().join("outside")).unwrap();
    let snapshot = scan(directory.path()).unwrap();
    assert!(snapshot.is_complete());
    assert_eq!(snapshot.entries.len(), 3); // root and two links, no targets
    assert_eq!(
        snapshot.entries[&directory.path().join("cycle")].kind,
        FileKind::Symlink
    );
    assert!(scan(&directory.path().join("outside")).is_err());
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn distinct_non_utf8_names_remain_distinct_native_bindings() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let first = directory
        .path()
        .join(OsString::from_vec(b"name\xff".to_vec()));
    let second = directory
        .path()
        .join(OsString::from_vec(b"name\xfe".to_vec()));
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let snapshot = scan(directory.path()).unwrap();
    assert!(snapshot.entries.contains_key(&first));
    assert!(snapshot.entries.contains_key(&second));
    assert_eq!(snapshot.totals().logical_bytes, Some(11));
    assert_ne!(
        measure_file(&first).unwrap().identity,
        measure_file(&second).unwrap().identity
    );
}
