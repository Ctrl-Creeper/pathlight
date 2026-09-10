//! gitignore-style exclusion of noisy paths before attribution.
//! Port of Swift's `ActivityExclusionFilter`; the matching itself is delegated to
//! ripgrep's `ignore` crate, which implements gitignore semantics faithfully.

use std::path::Path;

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

/// Trims, drops blanks, and de-duplicates while keeping order.
pub fn normalized_patterns<S: AsRef<str>>(patterns: &[S]) -> Vec<String> {
    let mut seen = Vec::new();
    for pattern in patterns {
        let trimmed = pattern.as_ref().trim();
        if !trimmed.is_empty() && !seen.iter().any(|s: &String| s == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    seen
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
