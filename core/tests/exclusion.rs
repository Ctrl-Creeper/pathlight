//! The exclusion rules both hosts have to agree on.
//!
//! These cases came from the Swift app's `ScanExclusionMatcher` and
//! `ActivityExclusionFilter` tests, and they live here because the matching
//! now lives here: two implementations of "is this path noise" drift, and the
//! journal both hosts read would then hold two answers about what is worth
//! recording.

use pathlight_core::exclusion::{normalized_patterns, ExclusionFilter};

fn filter(patterns: &[&str], root: &str) -> ExclusionFilter {
    ExclusionFilter::new(patterns, root)
        .expect("patterns compile")
        .expect("patterns are not empty")
}

#[test]
fn an_empty_pattern_list_is_no_filter_at_all() {
    assert!(ExclusionFilter::new::<&str>(&[], "/root")
        .unwrap()
        .is_none());
    assert!(ExclusionFilter::new(&["   ", ""], "/root")
        .unwrap()
        .is_none());
}

#[test]
fn a_leaf_name_and_an_extension_both_match() {
    let filter = filter(&[".DS_Store", "*.tmp"], "/Users/example");
    assert!(filter.excludes("/Users/example/Documents/.DS_Store"));
    assert!(filter.excludes("/Users/example/scratch.tmp"));
    assert!(!filter.excludes("/Users/example/Documents/report.pdf"));
}

#[test]
fn a_directory_pattern_takes_everything_under_it() {
    let filter = filter(&[".Trash/", "**/Caches/"], "/Users/example");
    assert!(filter.excludes("/Users/example/.Trash/old/movie.mov"));
    assert!(filter.excludes("/Users/example/Library/Caches/com.app/blob.bin"));
    assert!(filter.excludes("/Users/example/Library/Caches"));
    assert!(!filter.excludes("/Users/example/Library/Preferences/com.app.plist"));
}

/// A deleted path cannot be stat'ed, so a directory-only pattern has to match
/// the bare leaf as well — the alternative is recording the noise a watch was
/// told to ignore, on exactly the events that say it went away.
#[test]
fn a_deleted_directory_still_matches_a_directory_only_pattern() {
    let filter = filter(&["node_modules/"], "/Users/example");
    assert!(filter.excludes("/Users/example/project/node_modules"));
}

#[test]
fn a_path_outside_the_root_is_never_excluded() {
    let filter = filter(&[".DS_Store"], "/Users/example/Downloads");
    assert!(!filter.excludes("/Users/example/Documents/.DS_Store"));
    assert!(!filter.excludes("/Users/example/Downloads"));
}

/// The patterns the presets ship: rooted paths, `**` in the middle, and a
/// trailing `/**`. Rooted at `/`, which is also the whole-disk case where the
/// filter used to answer "not mine" for every path on the machine.
#[test]
fn the_shipped_preset_patterns_match_what_they_name() {
    let filter = filter(
        &[
            "private/var/folders/",
            "**/.Trash/",
            "Library/Caches/**",
            "**/*.sqlite-wal",
        ],
        "/",
    );
    assert!(filter.excludes("/private/var/folders/zz/T/scratch"));
    assert!(filter.excludes("/Users/example/.Trash/gone.mov"));
    assert!(filter.excludes("/Library/Caches/build/artifact.o"));
    assert!(filter.excludes("/Users/example/Library/Application Support/db.sqlite-wal"));
    assert!(!filter.excludes("/Users/example/Documents/report.pdf"));
}

/// Written the way a person writes them, which is not the way they have to be
/// stored: Windows separators, a leading `./` or `/`, doubled separators, and
/// the same rule twice.
#[test]
fn a_pattern_is_stored_one_way_however_it_was_typed() {
    let patterns = normalized_patterns(&[
        " node_modules/ ",
        r"build\",
        "./target/",
        "/dist/",
        "Library//Caches/",
        "node_modules/",
        "",
        "   ",
    ]);
    assert_eq!(
        patterns,
        [
            "node_modules/",
            "build/",
            "target/",
            "dist/",
            "Library/Caches/"
        ]
    );
}
