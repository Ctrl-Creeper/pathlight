//! One spelling for every path in the crate.
//!
//! Backends disagree about how to write the same location: FSEvents gives
//! `/private/var/x`, inotify gives raw bytes, and ReadDirectoryChangesW hands
//! back `\\?\C:\x` with a verbatim prefix nothing else echoes. Left alone,
//! that disagreement is not cosmetic — the size index misses, and a missed
//! size turns a confirmed byte delta into `unknown`.
//!
//! So paths are normalized once at the boundary ([`Watcher::start`] for roots,
//! [`Emitter::change`] for events) and every comparison downstream is a plain
//! string operation that means the same thing on all three platforms.
//!
//! [`Watcher::start`]: crate::monitor::Watcher::start
//! [`Emitter::change`]: crate::monitor::Emitter::change

/// The canonical spelling of `path`: forward separators, no Windows verbatim
/// prefix, and no trailing separator (except on a filesystem root).
///
// ponytail: no case folding, though Windows and default APFS are both
// case-insensitive. Within one watch, the root comes from the picker and the
// event paths come from the kernel echoing that same root, so the casing
// already agrees. Fold here if a backend ever reports its own casing.
pub fn normalize(path: &str) -> String {
    #[cfg(windows)]
    {
        normalize_windows(path)
    }
    #[cfg(not(windows))]
    {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() && path.starts_with('/') {
            "/".into()
        } else {
            trimmed.into()
        }
    }
}

/// A Windows spelling conversion, never applied to a POSIX filename. Kept
/// independently testable so the verbatim/UNC contract is checked on every OS.
#[cfg(any(windows, test))]
fn normalize_windows(path: &str) -> String {
    // `\\?\UNC\server\share` is really `\\server\share`; plain `\\?\C:\x` is `C:\x`.
    let stripped = path
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(path).to_owned());

    let mut text = stripped.replace('\\', "/");
    // Keep the slash that *is* the location: `/` and `C:/` are not `""` and `C:`.
    while text.len() > 1 && text.ends_with('/') && !text[..text.len() - 1].ends_with(':') {
        text.pop();
    }
    text
}

/// True when `path` sits strictly below `root`. Both are expected in
/// [`normalize`] form; a path equal to the root is not "inside" it.
pub fn is_inside(root: &str, path: &str) -> bool {
    let root = root.strip_suffix('/').unwrap_or(root);
    path.strip_prefix(root)
        .is_some_and(|relative| relative.starts_with('/') && relative.len() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn posix_backslashes_are_filename_characters() {
        assert_eq!(normalize(r"/watched/a\b.txt"), r"/watched/a\b.txt");
        assert_ne!(
            normalize(r"/watched/a\b.txt"),
            normalize("/watched/a/b.txt")
        );
        assert_eq!(normalize("/watched/name:/"), "/watched/name:");
    }

    #[test]
    fn trailing_separators_go_away_but_roots_survive() {
        assert_eq!(normalize("/Users/x/Documents/"), "/Users/x/Documents");
        assert_eq!(normalize("/Users/x//"), "/Users/x");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize_windows("C:/"), "C:/");
    }

    #[test]
    fn windows_spellings_collapse_onto_the_portable_one() {
        assert_eq!(
            normalize_windows(r"\\?\C:\Users\x\Documents"),
            "C:/Users/x/Documents"
        );
        assert_eq!(normalize_windows(r"C:\Users\x\"), "C:/Users/x");
        assert_eq!(
            normalize_windows(r"\\?\UNC\server\share\x"),
            "//server/share/x"
        );
    }

    /// The bug this module exists to prevent: a verbatim-prefixed event path
    /// tested against a plain watch root looks like it is outside the watch.
    #[test]
    fn a_verbatim_event_path_is_inside_its_plain_root() {
        let root = normalize_windows(r"C:\Watched");
        assert!(is_inside(
            &root,
            &normalize_windows(r"\\?\C:\Watched\file.bin")
        ));
    }

    #[test]
    fn a_root_is_not_inside_itself_and_neither_are_its_siblings() {
        assert!(!is_inside("/a/b", "/a/b"));
        assert!(!is_inside("/a/b", "/a/b/"));
        assert!(!is_inside("/a/b", "/a/bc/file"));
        assert!(is_inside("/a/b", "/a/b/file"));
        assert!(is_inside("/a/b/", "/a/b/file"));
    }
}
