//#![allow(unused_imports, unused_variables, dead_code)]

use serde::{Deserialize, Serialize};

pub use crate::cfg::otto::{OttoSpec, RetentionSpec, default_otto};
pub use crate::cfg::param::{ParamSpec, ParamSpecs, Value};
pub use crate::cfg::task::{TaskSpec, TaskSpecs, deserialize_task_map};

/// `deny_unknown_fields` turns a stale or misplaced top-level key (e.g. root
/// `envs:`, which is not a thing; it belongs under `otto:`) into a loud
/// config-load error naming the field, rather than a silently-ignored no-op.
/// Per `borg/src/config.rs:281-285`. Does not reach `tasks`' free-form task
/// names: the attribute governs `ConfigSpec`'s own field names, not the
/// contents of `deserialize_task_map`'s value type.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSpec {
    #[serde(default = "default_otto")]
    pub otto: OttoSpec,

    #[serde(default, deserialize_with = "deserialize_task_map")]
    pub tasks: TaskSpecs,
}

impl Default for ConfigSpec {
    fn default() -> Self {
        Self {
            otto: default_otto(),
            tasks: TaskSpecs::new(),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
