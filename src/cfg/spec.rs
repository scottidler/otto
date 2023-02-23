#![allow(unused_imports, unused_variables, dead_code)]
use super::error::ConfigError;
//use anyhow::{anyhow, Result};
use eyre::Result;

use serde::de::{Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::num::ParseIntError;
use std::path::PathBuf;
use std::vec::Vec;

use crate::cfg::param::Param;
use crate::cfg::param::ParamType;
use crate::cfg::param::Params;
use crate::cfg::param::Value;

pub(crate) type Tasks = HashMap<String, Task>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Script(String),
    File(PathBuf),
    //URL(Url),
}

fn default_otto() -> String {
    "otto".to_string()
}

fn default_verbosity() -> i32 {
    1
}

fn default_api() -> i32 {
    1
}

fn default_jobs() -> i32 {
    12
}
fn default_defaults() -> Defaults {
    Defaults {
        api: default_api(),
        verbosity: default_verbosity(),
        jobs: default_jobs(),
        tasks: vec![],
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Spec {
    #[serde(default = "default_defaults")]
    pub defaults: Defaults,

    pub otto: Otto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Defaults {
    #[serde(default = "default_api")]
    pub api: i32,

    #[serde(default = "default_verbosity")]
    pub verbosity: i32,

    #[serde(default = "default_jobs")]
    pub jobs: i32,

    #[serde(default)]
    pub tasks: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Otto {
    #[serde(skip_deserializing, default = "default_otto")]
    pub name: String,

    #[serde(default)]
    pub help: Option<String>,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default)]
    pub about: Option<String>,

    #[serde(default)]
    pub version: Option<String>,

    #[serde(default, deserialize_with = "deserialize_param_map")]
    pub params: Params,

    #[serde(default, deserialize_with = "deserialize_task_map")]
    pub tasks: Tasks,

    pub action: Option<String>,
}

impl Otto {
    pub fn param_names(&self) -> Vec<&str> {
        self.params.keys().map(AsRef::as_ref).collect()
    }
    pub fn task_names(&self) -> Vec<&str> {
        self.tasks.keys().map(AsRef::as_ref).collect()
    }
}

impl Default for ParamType {
    fn default() -> Self {
        Self::OPT
    }
}

fn divine(title: &str) -> (String, Option<char>, Option<String>) {
    let flags: Vec<String> = title.split('|').map(std::string::ToString::to_string).collect();
    let short = flags
        .iter()
        .cloned()
        .filter(|i| i.starts_with('-') && i.len() == 2)
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .next();

    let long = Some(String::from(
        flags
            .iter()
            .cloned()
            .filter(|i| i.starts_with("--") && i.len() > 2)
            .collect::<String>()
            .trim_matches('-'),
    ))
    .filter(|s| !s.is_empty());

    let name = if let Some(ref long) = long {
        long.clone()
    } else {
        match short {
            Some(ref short) => short.to_string(),
            None => title.to_string(),
        }
    };
    (name, short, long)
}

fn deserialize_param_map<'de, D>(deserializer: D) -> Result<Params, D::Error>
where
    D: Deserializer<'de>,
{
    struct ParamMap;

    impl<'de> Visitor<'de> for ParamMap {
        type Value = Params;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of name to Param")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut params = Params::new();
            while let Some((title, mut param)) = map.next_entry::<String, Param>()? {
                (param.name, param.short, param.long) = divine(&title);
                if param.long.is_some() || param.short.is_some() {
                    if let Some(ref value) = param.default {
                        if value == "true" || value == "false" {
                            param.param_type = ParamType::FLG;
                        }
                    }
                } else {
                    param.param_type = ParamType::POS;
                }
                params.insert(title.clone(), param);
            }
            Ok(params)
        }
    }
    deserializer.deserialize_map(ParamMap)
}

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

fn deserialize_task_map<'de, D>(deserializer: D) -> Result<Tasks, D::Error>
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
