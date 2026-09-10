//! gitignore-style exclusion of noisy paths before attribution.
//! Port of Swift's `ActivityExclusionFilter`; the matching itself is delegated to
//! ripgrep's `ignore` crate, which implements gitignore semantics faithfully.

use std::path::Path;
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::CoreError;

/// Suggested starting point for long-term watches; users edit per target.
pub const DEFAULT_PATTERNS: &[&str] = &[
    ".DS_Store",
    ".Trash/",
    ".Trashes/",
    ".fseventsd/",
    ".Spotlight-V100/",
    ".TemporaryItems/",
    ".DocumentRevisions-V100/",
    "**/Caches/",
    "*.tmp",
    "*.part",
    "*.crdownload",
    "*.download",
];

/// One spelling for a pattern however it was typed, blanks dropped and
/// duplicates removed while keeping the order the user wrote them in.
///
/// A pattern is text a person types, so it arrives with whatever their
/// keyboard and habits produced: Windows separators, a leading `./` from a
/// shell completion, doubled separators from pasting two halves together. The
/// rules below are the ones the macOS app has always applied, kept here so
/// both hosts store the same list and a target's patterns mean the same thing
/// wherever it is opened.
pub fn normalized_patterns<S: AsRef<str>>(patterns: &[S]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for pattern in patterns {
        let Some(normalized) = normalized_pattern(pattern.as_ref()) else {
            continue;
        };
        if !seen.contains(&normalized) {
            seen.push(normalized);
        }
    }
    seen
}

/// `None` for a pattern that is nothing but separators and space.
// ponytail: a backslash is taken as a separator, not the gitignore escape and
// not the legal POSIX filename character it also is. Somebody typing
// `build\` means a folder far more often than they mean a file with a
// backslash in its name; add an escape for the rare one when somebody asks.
fn normalized_pattern(pattern: &str) -> Option<String> {
    let mut text = pattern.trim().replace('\\', "/");
    while let Some(rest) = text.strip_prefix("./") {
        text = rest.to_owned();
    }
    // A leading separator is dropped rather than anchoring the pattern to the
    // watch root, because that is what the app has always done with it.
    text = text.trim_start_matches('/').to_owned();
    while text.contains("//") {
        text = text.replace("//", "/");
    }
    let directory_only = text.ends_with('/');
    let trimmed = text.trim_end_matches('/');
    match trimmed.is_empty() {
        true => None,
        false => Some(match directory_only {
            true => format!("{trimmed}/"),
            false => trimmed.to_owned(),
        }),
    }
}

#[derive(Debug)]
pub struct ExclusionFilter {
    root: String,
    matcher: Gitignore,
}

impl ExclusionFilter {
    /// Returns `None` when the pattern list normalizes to empty, so callers can
    /// skip filtering entirely.
    pub fn new<S: AsRef<str>>(patterns: &[S], root: &str) -> Result<Option<Self>, CoreError> {
        let patterns = normalized_patterns(patterns);
        if patterns.is_empty() {
            return Ok(None);
        }
        let root = crate::paths::normalize(root);
        let mut builder = GitignoreBuilder::new(&root);
        for pattern in &patterns {
            builder
                .add_line(None, pattern)
                .map_err(|error| CoreError::Encoding {
                    message: format!("invalid exclusion pattern {pattern:?}: {error}"),
                })?;
        }
        let matcher = builder.build().map_err(|error| CoreError::Encoding {
            message: error.to_string(),
        })?;
        Ok(Some(Self { root, matcher }))
    }

    pub fn excludes(&self, path: &str) -> bool {
        let path = crate::paths::normalize(path);
        // The shared containment test rather than prefix arithmetic of its
        // own: a watch on a filesystem root has a root that is one separator,
        // and subtracting it left every path looking like it was somewhere
        // else — so a whole-disk watch ignored every pattern it was given.
        if !crate::paths::is_inside(&self.root, &path) {
            return false;
        }
        // Deleted paths cannot be stat'ed, so the leaf is tried both as a file and
        // as a directory; over-matching is fine for noise filtering. Ancestors are
        // covered by `matched_path_or_any_parents`.
        let candidate = Path::new(&path);
        self.matcher
            .matched_path_or_any_parents(candidate, false)
            .is_ignore()
            || self
                .matcher
                .matched_path_or_any_parents(candidate, true)
                .is_ignore()
    }
}

/// [`ExclusionFilter`] for a host: the same rules, reached across the FFI.
///
/// Holds an `Option` so an empty pattern list is still an object — a
/// constructor that returned nothing would have to be a factory function on
/// the other side, and "record everything" is a filter like any other.
#[derive(Debug, uniffi::Object)]
pub struct ExclusionMatcher {
    filter: Option<ExclusionFilter>,
}

#[uniffi::export]
impl ExclusionMatcher {
    #[uniffi::constructor]
    pub fn new(patterns: Vec<String>, root: String) -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self {
            filter: ExclusionFilter::new(&patterns, &root)?,
        }))
    }

    /// True when nothing is being excluded, so a caller can skip the filter
    /// rather than ask it about every path.
    pub fn is_empty(&self) -> bool {
        self.filter.is_none()
    }

    pub fn excludes(&self, path: String) -> bool {
        self.filter
            .as_ref()
            .is_some_and(|filter| filter.excludes(&path))
    }
}

/// [`normalized_patterns`] across the FFI, for a host that stores what the
/// user typed and wants it stored the way this crate reads it.
#[uniffi::export]
pub fn normalized_exclusion_patterns(patterns: Vec<String>) -> Vec<String> {
    normalized_patterns(&patterns)
}
