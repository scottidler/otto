#![allow(unused_imports, unused_variables, dead_code)]
use eyre::Result;

use serde::de::{Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::num::ParseIntError;
use std::path::PathBuf;
use std::vec::Vec;

use crate::cfg::error::ConfigError;
use crate::cfg::param::{deserialize_param_map, Param, Params, Value};
use crate::cfg::task::{deserialize_task_map, Task, Tasks};

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
