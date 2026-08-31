#![cfg(test)]

use super::*;

#[test]
fn minting_then_splitting_returns_the_parts_that_went_in() {
    let name = subtask_name("build", "web");
    assert_eq!(name, "build:web");
    assert_eq!(split_subtask(&name), Some(("build", "web")));
    assert_eq!(parent_of(&name), Some("build"));
    assert_eq!(identifier_of(&name), Some("web"));
}

#[test]
fn a_name_without_a_separator_is_not_a_subtask() {
    assert_eq!(split_subtask("build"), None);
    assert_eq!(parent_of("build"), None);
    assert_eq!(identifier_of("build"), None);
    assert!(!is_subtask("build"));
}

#[test]
fn parent_or_self_is_the_lookup_form() {
    assert_eq!(parent_or_self("build:web"), "build");
    assert_eq!(parent_or_self("build"), "build");
}

/// The behavior the six open-coded copies disagreed about. A `foreach` over
/// paths mints identifiers like `src/a.txt`, and a `range` mints `1:10`, so an
/// identifier carrying its own colon is not hypothetical. Splitting on the
/// first separator keeps the parent whole; splitting on the last would have
/// returned `build:src/a` as the parent of `build:src/a:txt`.
#[test]
fn an_identifier_may_contain_the_separator_and_the_parent_stays_whole() {
    let name = subtask_name("build", "1:10");
    assert_eq!(name, "build:1:10");
    assert_eq!(parent_of(&name), Some("build"));
    assert_eq!(identifier_of(&name), Some("1:10"));
}

/// A dotted parent name is the case that lost every output in `7acb653`. It is
/// pinned here too, because this module is where a future "just split on the
/// last separator" change would reintroduce it.
#[test]
fn a_dotted_parent_name_survives_the_round_trip() {
    let name = subtask_name("build.web", "a");
    assert_eq!(name, "build.web:a");
    assert_eq!(parent_of(&name), Some("build.web"));
    assert_eq!(identifier_of(&name), Some("a"));
}

#[test]
fn is_subtask_agrees_with_split_subtask() {
    for name in ["build", "build:web", "build:1:10", "build.web:a", "", ":", "a:"] {
        assert_eq!(
            is_subtask(name),
            split_subtask(name).is_some(),
            "is_subtask and split_subtask disagree on {name:?}"
        );
    }
}

/// An empty parent or identifier is malformed, but the split must still be
/// total: no panic, and the halves are what the separator position says.
#[test]
fn degenerate_names_split_without_panicking() {
    assert_eq!(split_subtask(":"), Some(("", "")));
    assert_eq!(split_subtask("a:"), Some(("a", "")));
    assert_eq!(split_subtask(":a"), Some(("", "a")));
    assert_eq!(split_subtask(""), None);
}

#[test]
fn is_subtask_of_matches_only_the_named_parent() {
    assert!(is_subtask_of("build:web", "build"));
    assert!(is_subtask_of("build:1:10", "build"));
    assert!(is_subtask_of("build.web:a", "build.web"));

    // The prefix hazard, asserted rather than assumed: a longer parent name that
    // merely starts with the same characters is not a match.
    assert!(!is_subtask_of("build_all:web", "build"));
    assert!(!is_subtask_of("buildweb:x", "build"));
    // A top-level task is nobody's subtask, including its own.
    assert!(!is_subtask_of("build", "build"));
}
