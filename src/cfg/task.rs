use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

use crate::cfg::param::{deserialize_param_map, Params};

pub type Tasks = HashMap<String, Task>;

pub trait ITask {
    fn new(
        name: String,
        help: Option<String>,
        after: Vec<String>,
        before: Vec<String>,
        params: Params,
        action: Option<String>,
        selected: bool,
    ) -> Self;
    fn name(&self) -> &str;
    fn help(&self) -> Option<&str>;
    fn after(&self) -> &[String];
    fn before(&self) -> &[String];
    fn params(&self) -> &Params;
    fn action(&self) -> Option<&str>;
    fn selected(&self) -> bool;
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Task {
    #[serde(skip_deserializing)]
    name: String,

    #[serde(default)]
    help: Option<String>,

    #[serde(default)]
    after: Vec<String>,

    #[serde(default)]
    before: Vec<String>,

    #[serde(default, deserialize_with = "deserialize_param_map")]
    params: Params,

    action: Option<String>,

    #[serde(skip_deserializing)]
    selected: bool,
}

impl ITask for Task {
    #[must_use]
    fn new(
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

    fn name(&self) -> &str {
        &self.name
    }

    fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    fn after(&self) -> &[String] {
        &self.after
    }

    fn before(&self) -> &[String] {
        &self.before
    }

    fn params(&self) -> &Params {
        &self.params
    }

    fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }

    fn selected(&self) -> bool {
        self.selected
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
