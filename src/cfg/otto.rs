//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::vec::Vec;

/// The `otto.api` version this otto writes, and the one it assumes when an
/// ottofile declares no `api:` at all.
pub const CURRENT_API_VERSION: &str = "1";

/// Every `otto.api` version this otto has reviewed and executes correctly. An
/// ottofile declaring anything else is a loud failure in [`check_api_version`],
/// checked BEFORE the typed parse so the operator is told the file is from a
/// newer otto instead of being handed a confusing complaint about one key.
///
/// A SET, not a floor: a future `"2"` must be read and added deliberately,
/// rather than being accepted because it is numerically larger.
///
/// **Policy for growing the set.** A new version is added when, and only when,
/// otto makes a change that a prior otto would MIS-EXECUTE rather than merely
/// fail to understand. Adding an optional field does NOT bump it: strict
/// parsing already rejects the unknown key with a truthful message. Renaming or
/// re-typing an existing key, or changing what an existing key means, DOES. Old
/// versions stay in the set as long as otto still executes them correctly,
/// which is why this is a set and not a floor.
pub const SUPPORTED_API_VERSIONS: &[&str] = &[CURRENT_API_VERSION];

/// Minimal, deliberately tolerant view of an ottofile: just `otto.api`. Parsed
/// BEFORE the typed [`crate::cfg::config::ConfigSpec`] parse so a version
/// mismatch surfaces as "upgrade otto" instead of a confusing complaint about
/// whichever key the newer schema added.
///
/// It carries no `deny_unknown_fields` and every field is `Option` with a
/// default: the whole point is that it survives a document it does not
/// understand. A file with no `otto:` block, or an `otto:` block with no
/// `api:`, parses to `None` and is treated as [`CURRENT_API_VERSION`].
///
/// Deliberate deviation from borg (`borg/src/harvest/contract.rs:205-212`),
/// whose `VersionHeader.schema_version` is a required `u32`: borg emits its own
/// contracts and can require the field, while otto's ottofiles are hand-written
/// and `api:` is optional today. Requiring it would break them for no gain.
#[derive(Deserialize)]
struct ApiHeader {
    #[serde(default)]
    otto: Option<ApiHeaderOtto>,
}

#[derive(Deserialize)]
struct ApiHeaderOtto {
    #[serde(default)]
    api: Option<String>,
}

/// Reject an ottofile whose declared `otto.api` this otto does not speak.
///
/// Tolerant by construction: a document that does not even yield an
/// [`ApiHeader`] (unparseable YAML, an `otto:` block of the wrong shape) is
/// passed through so the typed parse can report the real, specific error.
pub fn check_api_version(content: &str) -> Result<()> {
    let Ok(header) = serde_yaml::from_str::<ApiHeader>(content) else {
        log::debug!("cfg::check_api_version: no readable api header, deferring to the typed parse");
        return Ok(());
    };
    let declared = header.otto.and_then(|otto| otto.api);
    log::debug!("cfg::check_api_version: declared={declared:?} supported={SUPPORTED_API_VERSIONS:?}");
    if let Some(api) = declared
        && !SUPPORTED_API_VERSIONS.contains(&api.as_str())
    {
        bail!(
            "otto: unsupported api version '{}' (this otto supports: {}). upgrade otto.",
            api,
            SUPPORTED_API_VERSIONS.join(", ")
        );
    }
    Ok(())
}

fn default_name() -> String {
    "otto".to_string()
}

fn default_about() -> String {
    "A task runner".to_string()
}

fn default_api() -> String {
    CURRENT_API_VERSION.to_string()
}

fn default_jobs() -> usize {
    num_cpus::get()
}

fn default_home() -> String {
    "~/.otto".to_string()
}

fn default_tasks() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_verbosity() -> u8 {
    1
}

fn default_keep_days() -> u64 {
    30
}

fn default_keep_last() -> usize {
    10
}

fn default_keep_failed() -> u64 {
    60
}

fn default_auto_prune() -> bool {
    true
}

fn default_prune_interval_hours() -> u64 {
    24
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RetentionSpec {
    /// Delete runs older than this many days (default: 30)
    #[serde(default = "default_keep_days")]
    pub keep_days: u64,

    /// Always keep at least this many most recent runs (default: 10)
    #[serde(default = "default_keep_last")]
    pub keep_last: usize,

    /// Keep failed runs for this many days (default: 60)
    #[serde(default = "default_keep_failed")]
    pub keep_failed: u64,

    /// Enable automatic pruning after runs (default: true)
    #[serde(default = "default_auto_prune")]
    pub auto_prune: bool,

    /// Minimum hours between auto-prune runs (default: 24)
    #[serde(default = "default_prune_interval_hours")]
    pub prune_interval_hours: u64,
}

impl Default for RetentionSpec {
    fn default() -> Self {
        Self {
            keep_days: default_keep_days(),
            keep_last: default_keep_last(),
            keep_failed: default_keep_failed(),
            auto_prune: default_auto_prune(),
            prune_interval_hours: default_prune_interval_hours(),
        }
    }
}

#[must_use]
pub fn default_otto() -> OttoSpec {
    OttoSpec {
        name: default_name(),
        about: default_about(),
        api: default_api(),
        jobs: default_jobs(),
        home: default_home(),
        tasks: default_tasks(),
        verbosity: default_verbosity(),
        envs: HashMap::new(),
        retention: RetentionSpec::default(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OttoSpec {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_about")]
    pub about: String,

    #[serde(default = "default_api")]
    pub api: String,

    #[serde(default = "default_jobs")]
    pub jobs: usize,

    #[serde(default = "default_home")]
    pub home: String,

    #[serde(default = "default_tasks")]
    pub tasks: Vec<String>,

    #[serde(default = "default_verbosity")]
    pub verbosity: u8,

    #[serde(default)]
    pub envs: HashMap<String, String>,

    #[serde(default)]
    pub retention: RetentionSpec,
}

impl Default for OttoSpec {
    fn default() -> Self {
        Self {
            name: default_name(),
            about: default_about(),
            api: default_api(),
            jobs: default_jobs(),
            home: default_home(),
            tasks: default_tasks(),
            verbosity: default_verbosity(),
            envs: HashMap::new(),
            retention: RetentionSpec::default(),
        }
    }
}

#[cfg(test)]
mod tests {
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
        let spec: RetentionSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec, RetentionSpec::default());
    }

    #[test]
    fn test_retention_spec_deserialize_partial() {
        let yaml = "keep_days: 14\nkeep_last: 5";
        let spec: RetentionSpec = serde_yaml::from_str(yaml).unwrap();
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
        let spec: RetentionSpec = serde_yaml::from_str(yaml).unwrap();
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
        let spec: OttoSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.name, "test-project");
        assert_eq!(spec.retention.keep_days, 14);
        assert_eq!(spec.retention.keep_last, 5);
        assert_eq!(spec.retention.keep_failed, 60); // default
    }

    #[test]
    fn test_otto_spec_without_retention() {
        let yaml = "name: test-project";
        let spec: OttoSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.retention, RetentionSpec::default());
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
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let deserialized: RetentionSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(spec, deserialized);
    }
}
