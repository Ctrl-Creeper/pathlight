use pathlight_core::exclusion::{normalized_patterns, DEFAULT_PATTERNS};
use pathlight_core::ExclusionFilter;

const ROOT: &str = "/Users/example";

fn filter(patterns: &[&str]) -> ExclusionFilter {
    ExclusionFilter::new(patterns, ROOT)
        .unwrap()
        .expect("non-empty patterns")
}

#[test]
fn empty_patterns_disable_filtering() {
    assert!(ExclusionFilter::new::<&str>(&[], ROOT).unwrap().is_none());
    assert!(ExclusionFilter::new(&["   ", ""], ROOT).unwrap().is_none());
    assert_eq!(normalized_patterns(&[" a ", "a", "", "b"]), ["a", "b"]);
}

#[test]
fn matches_leaf_names_extensions_and_deep_directories() {
    let filter = filter(DEFAULT_PATTERNS);
    assert!(filter.excludes(&format!("{ROOT}/Documents/.DS_Store")));
    assert!(filter.excludes(&format!("{ROOT}/scratch.tmp")));
    assert!(filter.excludes(&format!("{ROOT}/.Trash/old/movie.mov")));
    assert!(filter.excludes(&format!("{ROOT}/Library/Caches/com.app/blob.bin")));
    assert!(filter.excludes(&format!("{ROOT}/Library/Caches")));
    assert!(!filter.excludes(&format!("{ROOT}/Documents/report.pdf")));
    assert!(!filter.excludes(&format!("{ROOT}/Library/Preferences/com.app.plist")));
}

#[test]
fn deleted_directory_leaf_matches_directory_only_patterns() {
    let filter = filter(&["node_modules/"]);
    assert!(filter.excludes(&format!("{ROOT}/project/node_modules")));
    assert!(filter.excludes(&format!("{ROOT}/project/node_modules/left-pad/index.js")));
}

#[test]
fn paths_outside_root_are_never_excluded() {
    let filter = filter(&[".DS_Store"]);
    assert!(!filter.excludes("/Volumes/Other/.DS_Store"));
    assert!(!filter.excludes("/Users/example2/.DS_Store"));
}

#[test]
fn a_watch_on_a_filesystem_root_still_excludes() {
    // The root every "watch my whole disk" target has. Its own prefix
    // arithmetic used to leave the relative path without a leading separator
    // here, which made the filter answer "not mine" for every path on the
    // machine.
    let filter = ExclusionFilter::new(&["private/var/folders/"], "/")
        .unwrap()
        .expect("non-empty patterns");
    assert!(filter.excludes("/private/var/folders/zz/T/scratch"));
}
