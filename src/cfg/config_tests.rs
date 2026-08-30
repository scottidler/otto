#![cfg(test)]

use super::*;

/// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). Root
/// `envs:` is not, and has never been, a `ConfigSpec` field; it belongs
/// under `otto:`. This is the root-level case, which has no parent path:
/// per the Phase 4 table, the assert is field + expected-set + location,
/// not a path prefix.
#[test]
fn deny_unknown_fields_names_an_invented_root_envs_key() {
    let yaml = "tasks:\n  up:\n    bash: echo hi\nenvs:\n  FOO: bar\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("envs"), "must name the field: {err}");
    assert!(err.contains("otto"), "must list otto in the expected set: {err}");
    assert!(err.contains("tasks"), "must list tasks in the expected set: {err}");
    assert!(
        err.contains("line") && err.contains("column"),
        "must name the location: {err}"
    );
}
