//! The one place that knows how a foreach subtask is named.
//!
//! A foreach subtask is named `<parent>:<identifier>`. That rule is used in two
//! directions - minting a subtask name and recovering the parent from one - and
//! before this module existed both directions were open-coded: six copies of
//! `split_once(':')` across three modules, five copies of `format!("{}:{}")`,
//! and two competing private helpers (`Task::derive_parent`, whose own doc
//! comment warned that "a fourth copy is how the rule drifts", and
//! `discovery::parent_task_name`). The copies had already drifted in return
//! type, in whether they allocated, and in what they did with a name carrying
//! more than one colon.
//!
//! Everything that mints or splits a subtask name goes through here, so the two
//! directions cannot disagree.

/// The separator between a foreach parent and its item identifier.
pub const SUBTASK_SEPARATOR: char = ':';

/// The name of the subtask `parent` spawns for `identifier`.
///
/// The inverse of [`split_subtask`]: `parent_of(&subtask_name(p, i)) == Some(p)`
/// for any `p` that contains no separator.
#[must_use]
pub fn subtask_name(parent: &str, identifier: &str) -> String {
    format!("{parent}{SUBTASK_SEPARATOR}{identifier}")
}

/// Split a subtask name into `(parent, identifier)`, or `None` if `name` is not
/// a subtask name.
///
/// Splits on the *first* separator, so an identifier may itself contain one and
/// the parent is never truncated by it.
#[must_use]
pub fn split_subtask(name: &str) -> Option<(&str, &str)> {
    name.split_once(SUBTASK_SEPARATOR)
}

/// The parent a subtask belongs to, or `None` if `name` is not a subtask name.
#[must_use]
pub fn parent_of(name: &str) -> Option<&str> {
    split_subtask(name).map(|(parent, _)| parent)
}

/// The item identifier a subtask carries, or `None` if `name` is not a subtask
/// name.
#[must_use]
pub fn identifier_of(name: &str) -> Option<&str> {
    split_subtask(name).map(|(_, identifier)| identifier)
}

/// The parent a subtask belongs to, or `name` itself when it is not a subtask.
///
/// The lookup form: callers resolving a dependency name against a table of
/// top-level tasks want the parent when there is one and the name as given when
/// there is not.
#[must_use]
pub fn parent_or_self(name: &str) -> &str {
    parent_of(name).unwrap_or(name)
}

/// Whether `name` is a foreach subtask name.
#[must_use]
pub fn is_subtask(name: &str) -> bool {
    split_subtask(name).is_some()
}

#[path = "naming_tests.rs"]
mod tests;
