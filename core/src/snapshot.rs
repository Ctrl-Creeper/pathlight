//! Native-name observations and comparisons of directory state.
//!
//! A traversal observes an interval. Neither successful enumeration nor a
//! comparison of two traversals proves a coherent instant or replays history.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::measurement::{measure_file, FileKind, FileMeasurement, ObjectIdentity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanConsistency {
    ObservedInterval,
}

#[derive(Clone, Debug)]
pub struct ScanError {
    pub path: PathBuf,
    pub kind: io::ErrorKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ScanSnapshot {
    pub root: PathBuf,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub consistency: ScanConsistency,
    pub entries: BTreeMap<PathBuf, FileMeasurement>,
    pub errors: Vec<ScanError>,
}

#[derive(Clone, Debug)]
pub struct SnapshotTotals {
    /// Sum over observed unique objects, never exclusive/reclaimable space.
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub measured_paths: u64,
    pub measured_objects: Option<u64>,
    pub partial: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingChangeKind {
    Created,
    Removed,
    Replaced,
    Renamed,
    HardLinkAdded,
    HardLinkRemoved,
}

/// Whether native object IDs can be compared across these two observations.
/// A gap, remount, journal reset, or unknown source lifecycle must use
/// `Unknown`: inode and file-reference values can be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityContinuity {
    ObservedWithoutGap,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct BindingChange {
    /// Endpoint correspondence, not proof that this operation occurred.
    pub kind: BindingChangeKind,
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub identity: Option<ObjectIdentity>,
    pub previous_identity: Option<ObjectIdentity>,
}

#[derive(Clone, Debug)]
pub struct Reconciliation {
    pub logical_delta: Option<i128>,
    pub allocated_delta: Option<i128>,
    pub bindings: Vec<BindingChange>,
    pub historical_complete: bool,
}

impl ScanSnapshot {
    /// All attempted reads succeeded. Does not imply an atomic observation.
    pub fn is_complete(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn totals(&self) -> SnapshotTotals {
        let mut totals = SnapshotTotals {
            logical_bytes: Some(0),
            allocated_bytes: Some(0),
            measured_paths: self.entries.len() as u64,
            measured_objects: Some(0),
            partial: !self.is_complete(),
        };
        let mut observed: BTreeMap<&ObjectIdentity, &FileMeasurement> = BTreeMap::new();
        for measurement in self.entries.values() {
            let Some(identity) = &measurement.identity else {
                totals.logical_bytes = None;
                totals.allocated_bytes = None;
                totals.measured_objects = None;
                totals.partial = true;
                continue;
            };
            if let Some(previous) = observed.insert(identity, measurement) {
                // The same object changed during traversal. An arbitrary alias
                // must not determine its size just because it sorted first.
                if previous.logical_bytes != measurement.logical_bytes {
                    totals.logical_bytes = None;
                    totals.partial = true;
                }
                if previous.allocated_bytes != measurement.allocated_bytes {
                    totals.allocated_bytes = None;
                    totals.partial = true;
                }
                continue;
            }
            totals.logical_bytes = totals
                .logical_bytes
                .and_then(|n| n.checked_add(measurement.logical_bytes));
            totals.allocated_bytes = totals
                .allocated_bytes
                .zip(measurement.allocated_bytes)
                .and_then(|(n, size)| n.checked_add(size));
        }
        totals.measured_objects = totals.measured_objects.map(|_| observed.len() as u64);
        totals
    }
}

/// Records native directory-entry names, skipping symlink targets. A root link
/// is rejected; callers may explicitly resolve it before choosing the scope.
/// This is a live traversal, not an adversarial race-safe filesystem snapshot.
pub fn scan(root: &Path) -> io::Result<ScanSnapshot> {
    scan_with(root, measure_file)
}

fn scan_with(
    root: &Path,
    mut measure: impl FnMut(&Path) -> io::Result<FileMeasurement>,
) -> io::Result<ScanSnapshot> {
    let started_at = SystemTime::now();
    let root = if root.is_absolute() {
        root.to_owned()
    } else {
        std::env::current_dir()?.join(root)
    };
    let root_measurement = measure(&root)?;
    if root_measurement.kind != FileKind::Directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "scan root must be a directory, not a symlink",
        ));
    }
    let mut snapshot = ScanSnapshot {
        root: root.clone(),
        started_at,
        finished_at: started_at,
        consistency: ScanConsistency::ObservedInterval,
        entries: BTreeMap::from([(root.clone(), root_measurement)]),
        errors: Vec::new(),
    };
    let mut directories = vec![root];
    while let Some(directory) = directories.pop() {
        // A directory may have been replaced since its name was queued. Never
        // knowingly descend through a replacement or a newly installed symlink.
        // This narrows that race; it cannot close the later path lookup window.
        match measure(&directory) {
            Ok(current)
                if current.kind == FileKind::Directory
                    && current.identity == snapshot.entries[&directory].identity => {}
            Ok(_) => {
                snapshot.record_error(
                    directory,
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "directory binding changed during scan",
                    ),
                );
                continue;
            }
            Err(error) => {
                snapshot.record_error(directory, error);
                continue;
            }
        }
        let children = match std::fs::read_dir(&directory) {
            Ok(children) => children,
            Err(error) => {
                snapshot.record_error(directory, error);
                continue;
            }
        };
        for child in children {
            let path = match child {
                Ok(child) => child.path(),
                Err(error) => {
                    snapshot.record_error(directory.clone(), error);
                    continue;
                }
            };
            match measure(&path) {
                Ok(measurement) => {
                    if measurement.kind == FileKind::Directory {
                        directories.push(path.clone());
                    }
                    snapshot.entries.insert(path, measurement);
                }
                Err(error) => snapshot.record_error(path, error),
            }
        }
    }
    snapshot.finished_at = SystemTime::now();
    Ok(snapshot)
}

impl ScanSnapshot {
    fn record_error(&mut self, path: PathBuf, error: io::Error) {
        self.errors.push(ScanError {
            path,
            kind: error.kind(),
            message: error.to_string(),
        });
    }
}

/// Identity matches are scoped to these observations. Native IDs may be reused
/// across deletion, gaps or remounts; a matching ID does not prove continuity.
/// Renamed denotes one lost and one gained name with matching observed IDs,
/// which can also result from unlink/link. It must not count filesystem calls.
pub fn reconcile(
    previous: &ScanSnapshot,
    current: &ScanSnapshot,
    identity_continuity: IdentityContinuity,
) -> Reconciliation {
    let mut result = Reconciliation {
        logical_delta: None,
        allocated_delta: None,
        bindings: Vec::new(),
        historical_complete: false,
    };
    if previous.root != current.root
        || !previous.is_complete()
        || !current.is_complete()
        || previous
            .entries
            .get(&previous.root)
            .and_then(|entry| entry.identity.as_ref())
            != current
                .entries
                .get(&current.root)
                .and_then(|entry| entry.identity.as_ref())
    {
        return result;
    }
    let before = previous.totals();
    let after = current.totals();
    result.logical_delta = before
        .logical_bytes
        .zip(after.logical_bytes)
        .map(|(a, b)| i128::from(b) - i128::from(a));
    result.allocated_delta = before
        .allocated_bytes
        .zip(after.allocated_bytes)
        .map(|(a, b)| i128::from(b) - i128::from(a));

    let old_ids = identity_names(previous);
    let new_ids = identity_names(current);
    let removed: BTreeMap<_, _> = previous
        .entries
        .iter()
        .filter(|(path, _)| !current.entries.contains_key(*path))
        .collect();
    let added: BTreeMap<_, _> = current
        .entries
        .iter()
        .filter(|(path, _)| !previous.entries.contains_key(*path))
        .collect();
    let mut paired_old = BTreeSet::new();
    let mut paired_new = BTreeSet::new();
    if identity_continuity == IdentityContinuity::ObservedWithoutGap {
        for (identity, old_paths) in &old_ids {
            let Some(new_paths) = new_ids.get(identity) else {
                continue;
            };
            let lost: Vec<_> = old_paths
                .iter()
                .copied()
                .filter(|path| removed.contains_key(path))
                .collect();
            let gained: Vec<_> = new_paths
                .iter()
                .copied()
                .filter(|path| added.contains_key(path))
                .collect();
            if let ([old_path], [new_path]) = (lost.as_slice(), gained.as_slice()) {
                result.bindings.push(BindingChange {
                    kind: BindingChangeKind::Renamed,
                    path: (**new_path).clone(),
                    previous_path: Some((**old_path).clone()),
                    identity: Some((*identity).clone()),
                    previous_identity: Some((*identity).clone()),
                });
                paired_old.insert(*old_path);
                paired_new.insert(*new_path);
            }
        }
    }
    for (path, measurement) in removed {
        if paired_old.contains(path) {
            continue;
        }
        result.bindings.push(BindingChange {
            kind: if identity_continuity == IdentityContinuity::ObservedWithoutGap
                && measurement
                    .identity
                    .as_ref()
                    .is_some_and(|id| new_ids.contains_key(id))
            {
                BindingChangeKind::HardLinkRemoved
            } else {
                BindingChangeKind::Removed
            },
            path: path.clone(),
            previous_path: None,
            identity: measurement.identity.clone(),
            previous_identity: measurement.identity.clone(),
        });
    }
    for (path, measurement) in added {
        if paired_new.contains(path) {
            continue;
        }
        result.bindings.push(BindingChange {
            kind: if identity_continuity == IdentityContinuity::ObservedWithoutGap
                && measurement
                    .identity
                    .as_ref()
                    .is_some_and(|id| old_ids.contains_key(id))
            {
                BindingChangeKind::HardLinkAdded
            } else {
                BindingChangeKind::Created
            },
            path: path.clone(),
            previous_path: None,
            identity: measurement.identity.clone(),
            previous_identity: None,
        });
    }
    if identity_continuity == IdentityContinuity::ObservedWithoutGap {
        for (path, measurement) in &current.entries {
            let Some(old) = previous.entries.get(path) else {
                continue;
            };
            if old.identity.is_some()
                && measurement.identity.is_some()
                && old.identity != measurement.identity
            {
                result.bindings.push(BindingChange {
                    kind: BindingChangeKind::Replaced,
                    path: path.clone(),
                    previous_path: Some(path.clone()),
                    identity: measurement.identity.clone(),
                    previous_identity: old.identity.clone(),
                });
            }
        }
    }
    result
}

fn identity_names(snapshot: &ScanSnapshot) -> BTreeMap<&ObjectIdentity, Vec<&PathBuf>> {
    let mut names: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (path, measurement) in &snapshot.entries {
        if let Some(identity) = &measurement.identity {
            names.entry(identity).or_default().push(path);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::measure_file;

    #[cfg(unix)]
    #[test]
    fn queued_directory_replaced_by_a_symlink_is_a_gap_not_an_external_subtree() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let queued = directory.path().join("queued");
        std::fs::create_dir(&queued).unwrap();
        std::fs::write(outside.path().join("outside-file"), b"outside").unwrap();
        let mut swapped = false;
        let snapshot = scan_with(directory.path(), |path| {
            let measurement = measure_file(path)?;
            if path == queued && !swapped {
                std::fs::remove_dir(&queued).unwrap();
                symlink(outside.path(), &queued).unwrap();
                swapped = true;
            }
            Ok(measurement)
        })
        .unwrap();

        assert!(!snapshot.entries.contains_key(&queued.join("outside-file")));
        assert!(!snapshot.is_complete());
        assert_eq!(snapshot.errors[0].path, queued);
        assert!(snapshot.totals().partial);
    }

    #[test]
    fn failed_measurement_marks_totals_partial_and_cannot_create_a_removal_delta() {
        let directory = tempfile::tempdir().unwrap();
        let unreadable = directory.path().join("unreadable");
        std::fs::write(&unreadable, b"still exists").unwrap();
        let before = scan(directory.path()).unwrap();
        let after = scan_with(directory.path(), |path| {
            if path == unreadable {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                measure_file(path)
            }
        })
        .unwrap();

        assert!(!after.is_complete());
        assert!(after.totals().partial);
        assert_eq!(after.errors.len(), 1);
        assert_eq!(after.errors[0].path, unreadable);
        assert_eq!(after.errors[0].kind, io::ErrorKind::PermissionDenied);
        let delta = reconcile(&before, &after, IdentityContinuity::ObservedWithoutGap);
        assert_eq!(delta.logical_delta, None);
        assert_eq!(delta.allocated_delta, None);
        assert!(delta.bindings.is_empty());
    }

    #[test]
    fn inconsistent_measurements_of_a_hard_link_do_not_choose_an_arbitrary_size() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let alias = directory.path().join("alias");
        std::fs::write(&first, b"shared bytes").unwrap();
        std::fs::hard_link(&first, &alias).unwrap();
        let mut snapshot = scan(directory.path()).unwrap();
        snapshot.entries.get_mut(&alias).unwrap().logical_bytes += 100;
        snapshot.entries.get_mut(&alias).unwrap().allocated_bytes = Some(9000);

        assert!(snapshot.totals().partial);
        assert_eq!(snapshot.totals().logical_bytes, None);
        assert_eq!(snapshot.totals().allocated_bytes, None);
    }
}
