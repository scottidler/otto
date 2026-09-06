//#![allow(unused_imports, unused_variables, dead_code)]

use serde::{Deserialize, Serialize};

pub use crate::cfg::otto::{OttoSpec, RetentionSpec, default_otto};
pub use crate::cfg::param::{ParamSpec, ParamSpecs, Value};
pub use crate::cfg::task::{TaskSpec, TaskSpecs, deserialize_task_map};

/// True when `otto` is exactly what an absent `otto:` block deserializes to.
/// Serialization skips the field in that case, so a config that never wrote
/// `otto:` does not gain the full default block (every `OttoSpec` field plus
/// the nested `RetentionSpec` fields) on re-emit.
fn otto_is_default(otto: &OttoSpec) -> bool {
    *otto == OttoSpec::default()
}

/// The whole ottofile.
///
/// `deny_unknown_fields` turns a stale or misplaced top-level key (e.g. root
/// `envs:`, which is not a thing; it belongs under `otto:`) into a loud
/// config-load error naming the field, rather than a silently-ignored no-op.
/// Does not reach `tasks`' free-form task names: the attribute governs
/// `ConfigSpec`'s own field names, not the contents of
/// `deserialize_task_map`'s value type.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSpec {
    #[serde(default = "default_otto", skip_serializing_if = "otto_is_default")]
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

#[path = "config_tests.rs"]
mod tests;
