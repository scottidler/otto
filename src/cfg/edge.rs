use serde::de::{Deserializer, Error, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    #[default]
    Success,
    Failure,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSpec {
    pub task: String,
    pub when: When,
    /// Tracks whether this edge was authored as a bare string (sugar form).
    /// Used by Serialize to preserve round-trip style.
    pub from_sugar: bool,
    /// Set to true when this edge was injected into `after:` by the on-failure: desugar pass.
    /// Used by `TaskSpec::serialize` to filter out injected edges so they don't appear as
    /// duplicates alongside the host's `on-failure:` field.
    pub is_injected_sugar: bool,
}

impl EdgeSpec {
    /// Construct a sugared bare-string edge with default `when: Success`.
    /// Test fixtures and scaffold generation should use this helper to ensure
    /// round-trip serialization emits the bare-string form.
    pub fn sugar(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            when: When::Success,
            from_sugar: true,
            is_injected_sugar: false,
        }
    }
}

impl<'de> Deserialize<'de> for EdgeSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EdgeVisitor;
        impl<'de> Visitor<'de> for EdgeVisitor {
            type Value = EdgeSpec;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a task name string or a {task, when} object")
            }
            // A task keyed `2024:` or `true:` in an ottofile loads: YAML hands
            // the task map an integer or boolean scalar and the map key is
            // stringified. An edge naming that task is the same name written in
            // the same place, so it stringifies the same way - without these
            // three, `after: [2024]` failed with "invalid type: integer" while
            // the task it named loaded fine.
            fn visit_u64<E: Error>(self, value: u64) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec::sugar(value.to_string()))
            }
            fn visit_i64<E: Error>(self, value: i64) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec::sugar(value.to_string()))
            }
            fn visit_bool<E: Error>(self, value: bool) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec::sugar(value.to_string()))
            }
            fn visit_str<E: Error>(self, value: &str) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec {
                    task: value.to_string(),
                    when: When::default(),
                    from_sugar: true,
                    is_injected_sugar: false,
                })
            }
            fn visit_string<E: Error>(self, value: String) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec {
                    task: value,
                    when: When::default(),
                    from_sugar: true,
                    is_injected_sugar: false,
                })
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<EdgeSpec, M::Error> {
                let mut task: Option<String> = None;
                let mut when: When = When::default();
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "task" => task = Some(map.next_value()?),
                        "when" => when = map.next_value()?,
                        other => return Err(Error::unknown_field(other, &["task", "when"])),
                    }
                }
                let task = task.ok_or_else(|| Error::missing_field("task"))?;
                Ok(EdgeSpec {
                    task,
                    when,
                    from_sugar: false,
                    is_injected_sugar: false,
                })
            }
        }
        deserializer.deserialize_any(EdgeVisitor)
    }
}

impl Serialize for EdgeSpec {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        // Note: filtering of is_injected_sugar happens at TaskSpec::serialize
        // (around the seq element walk), not here - EdgeSpec serializes itself
        // unconditionally if asked.
        if self.from_sugar && self.when == When::Success {
            ser.serialize_str(&self.task)
        } else {
            let mut map = ser.serialize_map(Some(2))?;
            map.serialize_entry("task", &self.task)?;
            map.serialize_entry("when", &self.when)?;
            map.end()
        }
    }
}

#[path = "edge_tests.rs"]
mod tests;
