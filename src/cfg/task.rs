use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

use crate::cfg::param::{deserialize_param_map, Params};

pub type Tasks = HashMap<String, Task>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Task {
    #[serde(skip_deserializing)]
    pub name: String,

    #[serde(default)]
    pub help: Option<String>,

    #[serde(default)]
    pub after: Vec<String>,

    #[serde(default)]
    pub before: Vec<String>,

    #[serde(default, deserialize_with = "deserialize_param_map")]
    pub params: Params,

    pub action: Option<String>,

    #[serde(skip_deserializing)]
    pub selected: bool,
}

impl Task {
    #[must_use]
    pub fn new(
        name: String,
        help: Option<String>,
        after: Vec<String>,
        before: Vec<String>,
        params: Params,
        action: Option<String>,
        selected: bool,
    ) -> Self {
        Self {
            name,
            help,
            after,
            before,
            params,
            action,
            selected,
        }
    }
}

pub fn deserialize_task_map<'de, D>(deserializer: D) -> Result<Tasks, D::Error>
where
    D: Deserializer<'de>,
{
    struct TaskMap;

    impl<'de> Visitor<'de> for TaskMap {
        type Value = Tasks;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of name to Task")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut tasks = Tasks::new();
            while let Some((name, mut task)) = map.next_entry::<String, Task>()? {
                task.name = name.clone();
                tasks.insert(name.clone(), task);
            }
            Ok(tasks)
        }
    }
    deserializer.deserialize_map(TaskMap)
}
