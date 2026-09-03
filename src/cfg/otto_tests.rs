#![cfg(test)]

use super::*;

#[test]
fn test_check_api_version_accepts_the_current_version() {
    // Both spellings: YAML `1` is an int scalar, `'1'` is a string.
    for yaml in ["otto:\n  api: 1\n", "otto:\n  api: '1'\n"] {
        check_api_version(yaml).unwrap_or_else(|e| panic!("{yaml:?} should load: {e}"));
    }
}

#[test]
fn test_check_api_version_accepts_an_absent_api() {
    // No `otto:` block at all, and an `otto:` block declaring no `api:`.
    check_api_version("tasks:\n  up:\n    bash: echo hi\n").unwrap();
    check_api_version("otto:\n  name: demo\n").unwrap();
    check_api_version("otto:\n").unwrap();
}

#[test]
fn test_check_api_version_rejects_an_unsupported_version() {
    let err = check_api_version("otto:\n  api: 2\n").unwrap_err().to_string();
    assert!(
        err.contains("unsupported api version '2'"),
        "names the declared version: {err}"
    );
    assert!(err.contains("this otto supports: 1"), "names the supported set: {err}");
    assert!(err.contains("upgrade otto"), "names the remedy: {err}");
}

#[test]
fn test_check_api_version_defers_when_the_header_is_unreadable() {
    // The header is deliberately tolerant: a document it cannot make sense
    // of is passed through so the typed parse reports the real error.
    check_api_version("otto: [not, a, mapping]\n").unwrap();
    check_api_version("this: is: not: valid: yaml\n").unwrap();
}

#[test]
fn test_supported_api_versions_contains_the_default() {
    // The set and `default_api()` cannot drift: an ottofile with no `api:`
    // is treated as the default, which must itself be supported.
    assert!(SUPPORTED_API_VERSIONS.contains(&default_api().as_str()));
}

#[test]
fn test_retention_spec_defaults() {
    let spec = RetentionSpec::default();
    assert_eq!(spec.keep_days, 30);
    assert_eq!(spec.keep_last, 10);
    assert_eq!(spec.keep_failed, 60);
    assert!(spec.auto_prune);
    assert_eq!(spec.prune_interval_hours, 24);
}

#[test]
fn test_retention_spec_deserialize_empty() {
    let yaml = "{}";
    let spec: RetentionSpec = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(spec, RetentionSpec::default());
}

#[test]
fn test_retention_spec_deserialize_partial() {
    let yaml = "keep_days: 14\nkeep_last: 5";
    let spec: RetentionSpec = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(spec.keep_days, 14);
    assert_eq!(spec.keep_last, 5);
    assert_eq!(spec.keep_failed, 60); // default
    assert!(spec.auto_prune); // default
    assert_eq!(spec.prune_interval_hours, 24); // default
}

#[test]
fn test_retention_spec_deserialize_full() {
    let yaml = r#"
keep_days: 7
keep_last: 3
keep_failed: 14
auto_prune: false
prune_interval_hours: 12
"#;
    let spec: RetentionSpec = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(spec.keep_days, 7);
    assert_eq!(spec.keep_last, 3);
    assert_eq!(spec.keep_failed, 14);
    assert!(!spec.auto_prune);
    assert_eq!(spec.prune_interval_hours, 12);
}

#[test]
fn test_otto_spec_with_retention() {
    let yaml = r#"
name: test-project
retention:
  keep_days: 14
  keep_last: 5
"#;
    let spec: OttoSpec = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(spec.name, "test-project");
    assert_eq!(spec.retention.keep_days, 14);
    assert_eq!(spec.retention.keep_last, 5);
    assert_eq!(spec.retention.keep_failed, 60); // default
}

/// Phase 4 success criterion (a): an unknown key this binary predates
/// (the pre-2.1.0 upgrade cliff) fails with a message naming the key AND
/// naming upgrading.
#[test]
fn wrap_unknown_field_error_names_the_key_and_the_upgrade_fix() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "otto:\n  not-a-real-key: foo\ntasks:\n  up:\n    bash: echo hi\n";
    let raw_err = serde_yaml_ng::from_str::<ConfigSpec>(yaml).unwrap_err();
    let wrapped = wrap_unknown_field_error(raw_err).to_string();
    assert!(wrapped.contains("not-a-real-key"), "must still name the key: {wrapped}");
    assert!(wrapped.contains("otto Upgrade"), "must name the fix: {wrapped}");
}

/// Phase 4 success criterion (b): a genuinely misspelled key (`tsaks:`, not
/// a key any otto has ever shipped) gets the same wrapper, and the wrapper
/// does not assert being out of date as the ONLY explanation.
#[test]
fn wrap_unknown_field_error_hedges_a_genuine_misspelling() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tsaks:\n  up:\n    bash: echo hi\n";
    let raw_err = serde_yaml_ng::from_str::<ConfigSpec>(yaml).unwrap_err();
    let wrapped = wrap_unknown_field_error(raw_err).to_string();
    assert!(wrapped.contains("tsaks"), "must still name the key: {wrapped}");
    assert!(
        wrapped.contains("misspelled") && wrapped.contains("newer"),
        "must name both possibilities, not assert out-of-date alone: {wrapped}"
    );
}

/// A non-unknown-field serde failure (here, a wrong scalar type) is passed
/// through untouched: the trailing line only applies to "a key this binary
/// does not recognize", not to every load failure.
#[test]
fn wrap_unknown_field_error_is_a_noop_for_other_serde_failures() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "otto:\n  jobs: not-a-number\ntasks:\n  up:\n    bash: echo hi\n";
    let raw_err = serde_yaml_ng::from_str::<ConfigSpec>(yaml).unwrap_err();
    let raw_message = raw_err.to_string();
    let wrapped = wrap_unknown_field_error(raw_err).to_string();
    assert_eq!(
        wrapped, raw_message,
        "non-unknown-field errors must pass through unchanged"
    );
}

/// Phase 4 success criterion (c): the set stays exactly `["1"]`. Guards the
/// policy at `src/cfg/otto.rs:20-26` against a later change quietly growing
/// it for an additive key.
#[test]
fn supported_api_versions_stays_exactly_one() {
    assert_eq!(SUPPORTED_API_VERSIONS, &["1"]);
}

#[test]
fn test_otto_spec_without_retention() {
    let yaml = "name: test-project";
    let spec: OttoSpec = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(spec.retention, RetentionSpec::default());
}

/// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). Parsed
/// through `ConfigSpec`, not `OttoSpec` directly, so the error carries the
/// nesting path (`otto`) that a direct-struct parse would not have.
#[test]
fn deny_unknown_fields_names_a_misspelled_otto_task_key() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "otto:\n  task: foo\ntasks:\n  up:\n    bash: echo hi\n";
    let err = serde_yaml_ng::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("task"), "must name the field: {err}");
    assert!(err.contains("otto"), "must name the path: {err}");
}

/// Phase 4 negative test. `keep-days` (kebab) where the schema is snake
/// (`keep_days`): the schema mix means the kebab spelling is rejected, not
/// silently defaulted.
#[test]
fn deny_unknown_fields_names_a_kebab_retention_key_the_schema_keeps_snake() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "otto:\n  retention:\n    keep-days: 5\ntasks:\n  up:\n    bash: echo hi\n";
    let err = serde_yaml_ng::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("keep-days"), "must name the field: {err}");
    assert!(err.contains("otto.retention"), "must name the path: {err}");
}

#[test]
fn test_retention_spec_roundtrip() {
    let spec = RetentionSpec {
        keep_days: 7,
        keep_last: 3,
        keep_failed: 14,
        auto_prune: false,
        prune_interval_hours: 12,
    };
    let yaml = serde_yaml_ng::to_string(&spec).unwrap();
    let deserialized: RetentionSpec = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(spec, deserialized);
}

// No `#[cfg(test)]` here: the file already carries `#![cfg(test)]` at line 1,
// so the attribute was dead and only served to trip Phase 9's inline-test-module
// criterion.
mod jobs_range_tests {
    use crate::cfg::config::ConfigSpec;

    /// `otto.jobs: 0` is a config-load error, not a hang.
    ///
    /// The remediation plan originally DROPPED this validation, recording that
    /// "`otto.jobs` has zero consumers, so `jobs: 0` in an ottofile runs fine".
    /// Both halves later became false: `jobs` is consumed at
    /// `cli/parser.rs:777-779` whenever `-j` is absent, at which point `jobs: 0`
    /// reproduces the hot spin the `-j 0` fix removed (measured: exit 124 under
    /// a 12s timeout, 100% CPU, zero output).
    #[test]
    fn jobs_zero_is_rejected_at_config_load() {
        let err = serde_yaml_ng::from_str::<ConfigSpec>("otto:\n  jobs: 0\ntasks:\n  build:\n    bash: true\n")
            .expect_err("jobs: 0 must not load");
        let msg = err.to_string();
        assert!(
            msg.contains("not a valid job count"),
            "error must name the problem, got: {msg}"
        );
    }

    #[test]
    fn a_positive_jobs_value_still_loads() {
        for n in [1usize, 2, 64] {
            let yaml = format!("otto:\n  jobs: {n}\ntasks:\n  build:\n    bash: true\n");
            let spec: ConfigSpec = serde_yaml_ng::from_str(&yaml).expect("positive jobs must load");
            assert_eq!(spec.otto.jobs, Some(n));
        }
    }

    /// Omitting the key must not trip the guard, and must stay `None` rather
    /// than being pre-filled with the host's CPU count: the CLI owns that
    /// default, and `None` is what makes "the ottofile never wrote `jobs`"
    /// representable on the way back out.
    #[test]
    fn an_absent_jobs_key_stays_none_without_error() {
        let spec: ConfigSpec =
            serde_yaml_ng::from_str("tasks:\n  build:\n    bash: true\n").expect("absent jobs must load");
        assert_eq!(spec.otto.jobs, None, "an absent jobs key must not invent a count");
    }
}
